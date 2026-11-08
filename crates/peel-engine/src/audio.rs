//! Audio region playback.
//!
//! A [`RenderPlan`](crate::RenderPlan) carries [`AudioRegion`]s. Every block
//! the engine resolves each region that overlaps the block into engine
//! samples with the transport's absolute [`Origin`], then adds the audio to
//! the region's mixer track. Nothing about a region is accumulated from block
//! to block, so
//!
//! * the output does not depend on how the host splits the stream,
//! * a seek or loop wrap into the middle of a region starts at the right
//!   source frame (chase), and
//! * the only state is the declick tails below.
//!
//! # Envelope
//!
//! The gain applied to a frame is the product of
//!
//! * the region gain,
//! * a short linear fade at both edges ([`EDGE_FADE_SECONDS`]) so a region
//!   that starts or ends on a non-zero sample does not click,
//! * the clip's own fade-in and fade-out,
//! * an entry fade of [`DECLICK_SECONDS`] for a region that was already
//!   running when the transport (re)started or was re-anchored (play, seek,
//!   loop wrap, plan swap), and
//! * for a tail, the fade-out that replaces the cut.
//!
//! Every linear fade over `n` frames uses the gains `(k + 1) / (n + 1)`, so
//! the first audible frame is never exactly silent and a region's first and
//! last frame land exactly on their sample positions.
//!
//! # Cuts and tails
//!
//! When the transport jumps (stop, pause, seek, loop wrap, end of material,
//! plan swap) the regions sounding at that instant continue for
//! [`DECLICK_SECONDS`] with a linear fade-out ("tail") instead of stopping
//! dead. The new context fades its in-progress regions in over the same
//! number of frames, so a region that continues unchanged across a plan swap
//! comes out exactly as it would have without the swap (the two ramps sum to
//! one). Tails live in a fixed pool; each holds a reference to the sample and
//! returns it through the retirement queue when it finishes, so the audio
//! thread never frees a buffer.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use peel_dsp::SampleBuffer;
use peel_time::TempoMap;

use crate::clock::Origin;
use crate::command::Retired;
use crate::mixer::Mixer;
use crate::plan::{AudioRegion, RenderPlan};
use crate::retire::Retirer;
use crate::telemetry::Telemetry;

/// Fade at both edges of every region.
pub(crate) const EDGE_FADE_SECONDS: f64 = 0.001;
/// Fade used when the transport cuts into or out of a region.
pub(crate) const DECLICK_SECONDS: f64 = 0.004;
/// Regions that can fade out after a cut at the same time. Further cuts end
/// abruptly and are counted in `Telemetry::audio_tail_overflows`.
pub(crate) const MAX_TAILS: usize = 256;
/// Transport segments recorded per block (one per loop wrap). A block with
/// more wraps than this plays audio only for the first segments.
pub(crate) const MAX_SEGMENTS: usize = 16;
/// Upper bound for a region's linear gain (+30 dB), as for mixer gains.
const MAX_GAIN: f32 = 32.0;
/// Slack in quarter notes for the cheap overlap test before a region is
/// resolved exactly in samples.
const BEAT_SLACK: f64 = 0.01;

/// A run of frames within a block played from one transport anchor.
#[derive(Clone, Copy)]
struct Segment {
    start: i64,
    end: i64,
    origin: Origin,
    /// Engine sample of the last discontinuity before this segment.
    entry: i64,
}

/// An [`AudioRegion`] resolved to engine samples under one anchor.
#[derive(Clone, Copy)]
struct Placement {
    /// First engine sample of the region.
    start: i64,
    /// One past its last.
    end: i64,
    /// Source frame sounding at `start`.
    src0: f64,
    /// Source frames consumed per output frame.
    ratio: f64,
    /// True when `ratio` is exactly one: frames are copied, not interpolated.
    unit: bool,
    reversed: bool,
    gain: f32,
    /// Clip fade-in ends here (`start..=end`).
    fade_in_end: i64,
    /// Clip fade-out starts here (`start..=end`).
    fade_out_start: i64,
    /// Edge fade length in frames.
    edge: i64,
    /// Engine sample of the discontinuity this placement was resolved under.
    /// A region that started before it fades in over `declick` frames.
    entry: i64,
}

impl Placement {
    /// Resolves a region under `origin`. `None` for regions that cannot
    /// sound (non-finite or empty ranges, silent or empty samples).
    fn resolve(
        r: &AudioRegion,
        tempo: &TempoMap,
        origin: &Origin,
        sr: f64,
        entry: i64,
        edge: i64,
    ) -> Option<Placement> {
        if !(r.start_beat.is_finite() && r.end_beat.is_finite() && r.end_beat > r.start_beat) {
            return None;
        }
        let rate = f64::from(r.sample.sample_rate);
        if !(rate.is_finite() && rate > 0.0) || r.sample.frames() == 0 {
            return None;
        }
        let source_start = r.source_start_seconds * rate;
        if !source_start.is_finite() {
            return None;
        }
        let start = origin.sample_of(tempo, r.start_beat, sr);
        let end = origin.sample_of(tempo, r.end_beat, sr);
        if end <= start {
            return None;
        }
        let len_beats = r.end_beat - r.start_beat;
        let fade_in = clean_fade(r.fade_in_beats, len_beats);
        let fade_out = clean_fade(r.fade_out_beats, len_beats);
        let fade_in_end = if fade_in > 0.0 {
            origin
                .sample_of(tempo, r.start_beat + fade_in, sr)
                .clamp(start, end)
        } else {
            start
        };
        let fade_out_start = if fade_out > 0.0 {
            origin
                .sample_of(tempo, r.end_beat - fade_out, sr)
                .clamp(start, end)
        } else {
            end
        };
        let ratio = rate / sr;
        let unit = (ratio - 1.0).abs() < 1e-9;
        Some(Placement {
            start,
            end,
            src0: if unit {
                source_start.round_ties_even()
            } else {
                source_start
            },
            ratio: if unit { 1.0 } else { ratio },
            unit,
            reversed: r.reversed,
            gain: if r.gain.is_finite() {
                r.gain.clamp(0.0, MAX_GAIN)
            } else {
                0.0
            },
            fade_in_end,
            fade_out_start,
            edge: edge.min((end - start) / 2),
            entry,
        })
    }

    /// Adds engine samples `from..to` (which must lie inside `start..end`)
    /// to the track block that begins at engine sample `block_start`.
    ///
    /// `declick` is the entry fade length; `tail` is `(cut, length)` for a
    /// tail, which fades out over `length` frames from sample `cut`.
    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        buf: &SampleBuffer,
        from: i64,
        to: i64,
        block_start: i64,
        out_l: &mut [f32],
        out_r: &mut [f32],
        declick: i64,
        tail: Option<(i64, i64)>,
    ) {
        let src_l = buf.left();
        let src_r = buf.right();
        let n = src_l.len().min(src_r.len()) as i64;
        let len = self.end - self.start;
        let inv_edge = 1.0 / (self.edge + 1) as f32;
        let fade_in_len = self.fade_in_end - self.start;
        let inv_fade_in = 1.0 / (fade_in_len + 1) as f32;
        let fade_out_len = self.end - self.fade_out_start;
        let inv_fade_out = 1.0 / (fade_out_len + 1) as f32;
        let inv_entry = 1.0 / (declick + 1) as f32;
        let inv_tail = tail.map_or(0.0, |(_, length)| 1.0 / (length + 1) as f32);
        let entering = self.start < self.entry;
        let src0_int = self.src0 as i64;

        for s in from..to {
            let k = s - self.start;
            let m = self.end - 1 - s;

            let mut g = self.gain;
            if k < self.edge {
                g *= (k + 1) as f32 * inv_edge;
            }
            if m < self.edge {
                g *= (m + 1) as f32 * inv_edge;
            }
            if k < fade_in_len {
                g *= (k + 1) as f32 * inv_fade_in;
            }
            if s >= self.fade_out_start {
                g *= (m + 1) as f32 * inv_fade_out;
            }
            if entering {
                let e = s - self.entry;
                if (0..declick).contains(&e) {
                    g *= (e + 1) as f32 * inv_entry;
                }
            }
            if let Some((cut, length)) = tail {
                let t = s - cut;
                g *= (length - t).max(0) as f32 * inv_tail;
            }

            let (l, r) = if self.unit {
                let idx = if self.reversed {
                    src0_int + (len - 1 - k)
                } else {
                    src0_int + k
                };
                if idx >= 0 && idx < n {
                    (src_l[idx as usize], src_r[idx as usize])
                } else {
                    (0.0, 0.0)
                }
            } else {
                let pos = if self.reversed {
                    self.src0 + (len - 1 - k) as f64 * self.ratio
                } else {
                    self.src0 + k as f64 * self.ratio
                };
                lerp(src_l, src_r, pos)
            };
            // Decoders only produce finite samples; a damaged buffer must
            // not poison the track.
            let l = if l.is_finite() { l } else { 0.0 };
            let r = if r.is_finite() { r } else { 0.0 };

            let i = (s - block_start) as usize;
            out_l[i] += l * g;
            out_r[i] += r * g;
        }
    }
}

/// Linear interpolation of both channels at a fractional frame position.
/// Frames outside the buffer are silent.
#[inline]
fn lerp(l: &[f32], r: &[f32], pos: f64) -> (f32, f32) {
    let floor = pos.floor();
    let frac = (pos - floor) as f32;
    let i = floor as i64;
    let at = |buf: &[f32], i: i64| -> f32 {
        if i >= 0 && (i as usize) < buf.len() {
            buf[i as usize]
        } else {
            0.0
        }
    };
    let (a0, a1) = (at(l, i), at(l, i + 1));
    let (b0, b1) = (at(r, i), at(r, i + 1));
    (a0 + (a1 - a0) * frac, b0 + (b1 - b0) * frac)
}

/// A fade length in quarter notes, limited to the region.
fn clean_fade(fade: f64, len: f64) -> f64 {
    if fade.is_finite() {
        fade.clamp(0.0, len)
    } else {
        0.0
    }
}

/// A region that was sounding when the transport was cut, fading out.
struct Tail {
    place: Placement,
    /// Extra reference to the region's sample; returned through the
    /// retirement queue, never dropped here.
    sample: Arc<SampleBuffer>,
    track: usize,
    /// Engine sample of the cut.
    cut: i64,
}

/// Audio-thread state for region playback: the transport segments of the
/// current block and the declick tails.
pub(crate) struct AudioState {
    segments: [Segment; MAX_SEGMENTS],
    segment_count: usize,
    tails: Vec<Option<Tail>>,
    /// Edge fade length in frames.
    edge: i64,
    /// Entry and tail fade length in frames.
    declick: i64,
}

impl AudioState {
    pub fn new(sample_rate: f32) -> AudioState {
        let sr = f64::from(sample_rate);
        AudioState {
            segments: [Segment {
                start: 0,
                end: 0,
                origin: Origin::ZERO,
                entry: 0,
            }; MAX_SEGMENTS],
            segment_count: 0,
            tails: (0..MAX_TAILS).map(|_| None).collect(),
            edge: (EDGE_FADE_SECONDS * sr).round().max(1.0) as i64,
            declick: (DECLICK_SECONDS * sr).round().max(2.0) as i64,
        }
    }

    /// Forgets the previous block's segments.
    #[inline]
    pub fn begin_chunk(&mut self) {
        self.segment_count = 0;
    }

    /// Records that engine samples `start..end` play under `origin`.
    #[inline]
    pub fn push_segment(&mut self, start: i64, end: i64, origin: Origin, entry: i64) {
        if end > start && self.segment_count < MAX_SEGMENTS {
            self.segments[self.segment_count] = Segment {
                start,
                end,
                origin,
                entry,
            };
            self.segment_count += 1;
        }
    }

    /// The transport is cut at engine sample `at`: every region sounding
    /// there (started before it, ends after it) continues as a tail.
    pub fn cut(
        &mut self,
        plan: &RenderPlan,
        origin: &Origin,
        entry: i64,
        at: i64,
        sr: f64,
        telemetry: &Telemetry,
    ) {
        if plan.audio.is_empty() {
            return;
        }
        let beat = origin.beat_at(&plan.tempo, at, sr);
        for region in &plan.audio {
            if region.end_beat < beat - BEAT_SLACK || region.start_beat > beat + BEAT_SLACK {
                continue;
            }
            let Some(place) = Placement::resolve(region, &plan.tempo, origin, sr, entry, self.edge)
            else {
                continue;
            };
            if !(place.start < at && at < place.end) {
                continue;
            }
            match self.tails.iter_mut().find(|t| t.is_none()) {
                Some(slot) => {
                    *slot = Some(Tail {
                        place,
                        sample: region.sample.clone(),
                        track: usize::from(region.track),
                        cut: at,
                    });
                }
                None => {
                    telemetry
                        .audio_tail_overflows
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// Adds this block's tails and regions to their mixer tracks.
    pub fn render(
        &mut self,
        plan: &RenderPlan,
        mixer: &mut Mixer,
        retirer: &mut Retirer,
        block_start: i64,
        frames: usize,
        sr: f64,
    ) {
        let block_end = block_start + frames as i64;
        let n_tracks = plan.tracks.len();
        let declick = self.declick;

        for slot in self.tails.iter_mut() {
            let Some(tail) = slot else { continue };
            let last = (tail.cut + declick).min(tail.place.end);
            let lo = block_start.max(tail.cut);
            let hi = block_end.min(last);
            if lo < hi && tail.track < n_tracks {
                mixer.ensure_clean(tail.track, frames);
                let (l, r) = mixer.track(tail.track, frames);
                tail.place.render(
                    &tail.sample,
                    lo,
                    hi,
                    block_start,
                    l,
                    r,
                    declick,
                    Some((tail.cut, declick)),
                );
            }
            if block_end >= last
                && let Some(done) = slot.take()
            {
                retirer.retire(Retired::Sample(done.sample));
            }
        }

        for seg in &self.segments[..self.segment_count] {
            let beat_a = seg.origin.beat_at(&plan.tempo, seg.start, sr);
            let beat_b = seg.origin.beat_at(&plan.tempo, seg.end, sr);
            for region in &plan.audio {
                let track = usize::from(region.track);
                if track >= n_tracks
                    || region.end_beat < beat_a - BEAT_SLACK
                    || region.start_beat > beat_b + BEAT_SLACK
                {
                    continue;
                }
                let Some(place) =
                    Placement::resolve(region, &plan.tempo, &seg.origin, sr, seg.entry, self.edge)
                else {
                    continue;
                };
                let lo = seg.start.max(place.start);
                let hi = seg.end.min(place.end);
                if lo >= hi {
                    continue;
                }
                mixer.ensure_clean(track, frames);
                let (l, r) = mixer.track(track, frames);
                place.render(&region.sample, lo, hi, block_start, l, r, declick, None);
            }
        }
    }
}
