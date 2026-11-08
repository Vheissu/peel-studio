//! Mixer buffers and per-sample gain ramps.
//!
//! Every channel and track owns a [`Ramp`]: the gains actually applied so far.
//! Each block moves them toward the requested targets by at most `step` per
//! sample. Because the slew is per sample, the output does not depend on how
//! the host splits the stream into blocks, and mute/gain/pan changes never
//! produce a step discontinuity.

use peel_dsp::util::pan_gains;

use crate::MAX_TRACKS;

/// Gains applied to the left and right output of a channel or track.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Ramp {
    pub l: f32,
    pub r: f32,
    pub init: bool,
}

impl Ramp {
    /// Jumps to the targets without ramping.
    #[inline]
    pub fn snap(&mut self, tl: f32, tr: f32) {
        self.l = tl;
        self.r = tr;
        self.init = true;
    }

    /// True when the gains are settled at exactly zero.
    #[inline]
    pub fn is_silent(&self, tl: f32, tr: f32) -> bool {
        self.init && self.l == 0.0 && self.r == 0.0 && tl == 0.0 && tr == 0.0
    }
}

/// Left/right gain target for a gain, pan and mute state. Non-finite plan
/// values are treated as silence rather than propagated.
#[inline]
pub(crate) fn target_gains(gain: f32, pan: f32, muted: bool) -> (f32, f32) {
    if muted || !gain.is_finite() || !pan.is_finite() {
        return (0.0, 0.0);
    }
    if pan == 0.0 {
        return (gain, gain);
    }
    let (pl, pr) = pan_gains(pan);
    (gain * pl, gain * pr)
}

#[inline]
pub(crate) fn slew(cur: f32, target: f32, step: f32) -> f32 {
    let d = target - cur;
    if d > step {
        cur + step
    } else if d < -step {
        cur - step
    } else {
        target
    }
}

/// `dst (+)= src * ramped gain`. With `ACC = false` the destination is
/// overwritten (first writer into a track), otherwise summed into.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mix_ramped<const ACC: bool>(
    dl: &mut [f32],
    dr: &mut [f32],
    sl: &[f32],
    sr: &[f32],
    ramp: &mut Ramp,
    tl: f32,
    tr: f32,
    step: f32,
) {
    if !ramp.init {
        ramp.snap(tl, tr);
    }
    let n = dl.len().min(dr.len()).min(sl.len()).min(sr.len());
    if ramp.l == tl && ramp.r == tr {
        let (gl, gr) = (tl, tr);
        for i in 0..n {
            if ACC {
                dl[i] += sl[i] * gl;
                dr[i] += sr[i] * gr;
            } else {
                dl[i] = sl[i] * gl;
                dr[i] = sr[i] * gr;
            }
        }
        return;
    }
    let (mut gl, mut gr) = (ramp.l, ramp.r);
    for i in 0..n {
        gl = slew(gl, tl, step);
        gr = slew(gr, tr, step);
        if ACC {
            dl[i] += sl[i] * gl;
            dr[i] += sr[i] * gr;
        } else {
            dl[i] = sl[i] * gl;
            dr[i] = sr[i] * gr;
        }
    }
    ramp.l = gl;
    ramp.r = gr;
}

/// In-place `buf *= ramped gain`.
pub(crate) fn scale_ramped(
    l: &mut [f32],
    r: &mut [f32],
    ramp: &mut Ramp,
    tl: f32,
    tr: f32,
    step: f32,
) {
    if !ramp.init {
        ramp.snap(tl, tr);
    }
    let n = l.len().min(r.len());
    if ramp.l == tl && ramp.r == tr {
        if tl != 1.0 || tr != 1.0 {
            for i in 0..n {
                l[i] *= tl;
                r[i] *= tr;
            }
        }
        return;
    }
    let (mut gl, mut gr) = (ramp.l, ramp.r);
    for i in 0..n {
        gl = slew(gl, tl, step);
        gr = slew(gr, tr, step);
        l[i] *= gl;
        r[i] *= gr;
    }
    ramp.l = gl;
    ramp.r = gr;
}

/// Replaces non-finite samples with zero. Returns the peaks of the cleaned
/// buffers and the number of samples replaced.
pub(crate) fn sanitize_peak(l: &mut [f32], r: &mut [f32]) -> (f32, f32, u32) {
    let (mut pl, mut pr, mut bad) = (0.0f32, 0.0f32, 0u32);
    for x in l.iter_mut() {
        if x.is_finite() {
            pl = pl.max(x.abs());
        } else {
            *x = 0.0;
            bad += 1;
        }
    }
    for x in r.iter_mut() {
        if x.is_finite() {
            pr = pr.max(x.abs());
        } else {
            *x = 0.0;
            bad += 1;
        }
    }
    (pl, pr, bad)
}

#[inline]
pub(crate) fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |p, x| p.max(x.abs()))
}

/// Preallocated mixing storage. Sized for [`MAX_TRACKS`] tracks of
/// `max_block` frames, so the audio thread never allocates when a plan
/// arrives (see the crate docs for the rationale).
pub(crate) struct Mixer {
    max_block: usize,
    /// `MAX_TRACKS * 2 * max_block`: per track a left then a right block.
    tracks: Vec<f32>,
    /// True when a track's buffer holds this block's signal. Tracks that stay
    /// false are silent and are neither cleared nor read.
    pub dirty: Vec<bool>,
    /// Channel render target (left then right).
    scratch: Vec<f32>,
    /// Dry copy used while crossfading an insert's bypass.
    dry: Vec<f32>,
    pub chan_ramp: Vec<Ramp>,
    pub track_ramp: Vec<Ramp>,
    /// Sidechain keys: each key track's signal after its inserts and before
    /// its fader, so muting, soloing or mixing it does not change what it
    /// keys. Same layout as `tracks`.
    keys: Vec<f32>,
    /// True when a key holds this block's signal (otherwise it is silence).
    pub key_ready: Vec<bool>,
}

impl Mixer {
    pub fn new(max_block: usize, channel_slots: usize) -> Mixer {
        Mixer {
            max_block,
            tracks: vec![0.0; MAX_TRACKS * 2 * max_block],
            keys: vec![0.0; MAX_TRACKS * 2 * max_block],
            key_ready: vec![false; MAX_TRACKS],
            dirty: vec![false; MAX_TRACKS],
            scratch: vec![0.0; 2 * max_block],
            dry: vec![0.0; 2 * max_block],
            chan_ramp: vec![Ramp::default(); channel_slots],
            track_ramp: vec![Ramp::default(); MAX_TRACKS],
        }
    }

    /// The left and right blocks of a track.
    #[inline]
    pub fn track(&mut self, i: usize, frames: usize) -> (&mut [f32], &mut [f32]) {
        let mb = self.max_block;
        let base = i * 2 * mb;
        let (l, r) = self.tracks[base..base + 2 * mb].split_at_mut(mb);
        (&mut l[..frames], &mut r[..frames])
    }

    /// Copies a track's current signal into its key.
    pub fn store_key(&mut self, i: usize, frames: usize) {
        let mb = self.max_block;
        let base = i * 2 * mb;
        let (src_l, src_r) = self.tracks[base..base + 2 * mb].split_at(mb);
        let (dst_l, dst_r) = self.keys[base..base + 2 * mb].split_at_mut(mb);
        dst_l[..frames].copy_from_slice(&src_l[..frames]);
        dst_r[..frames].copy_from_slice(&src_r[..frames]);
        self.key_ready[i] = true;
    }

    /// Key `k` (silence when it holds nothing this block) and track `ti`.
    pub fn key_and_track(
        &mut self,
        k: usize,
        ti: usize,
        frames: usize,
    ) -> (&[f32], &[f32], &mut [f32], &mut [f32]) {
        let mb = self.max_block;
        let kb = k * 2 * mb;
        if !self.key_ready[k] {
            self.keys[kb..kb + 2 * mb].fill(0.0);
            self.key_ready[k] = true;
        }
        let (kl, kr) = self.keys[kb..kb + 2 * mb].split_at(mb);
        let tb = ti * 2 * mb;
        let (tl, tr) = self.tracks[tb..tb + 2 * mb].split_at_mut(mb);
        (
            &kl[..frames],
            &kr[..frames],
            &mut tl[..frames],
            &mut tr[..frames],
        )
    }

    /// Zeroes a track the first time it is touched in a block.
    #[inline]
    pub fn ensure_clean(&mut self, i: usize, frames: usize) {
        if !self.dirty[i] {
            let (l, r) = self.track(i, frames);
            l.fill(0.0);
            r.fill(0.0);
            self.dirty[i] = true;
        }
    }

    /// Disjoint access to the channel scratch buffers and the track storage,
    /// for rendering a channel straight into its track.
    #[inline]
    pub fn scratch_and_track(
        &mut self,
        track: usize,
        frames: usize,
    ) -> (&mut [f32], &mut [f32], &mut [f32], &mut [f32]) {
        let mb = self.max_block;
        let (sl, sr) = self.scratch.split_at_mut(mb);
        let base = track * 2 * mb;
        let (tl, tr) = self.tracks[base..base + 2 * mb].split_at_mut(mb);
        (
            &mut sl[..frames],
            &mut sr[..frames],
            &mut tl[..frames],
            &mut tr[..frames],
        )
    }

    /// Track block plus the dry-copy pair, for crossfaded bypass.
    #[inline]
    pub fn track_and_dry(
        &mut self,
        track: usize,
        frames: usize,
    ) -> (&mut [f32], &mut [f32], &mut [f32], &mut [f32]) {
        let mb = self.max_block;
        let (dl, dr) = self.dry.split_at_mut(mb);
        let base = track * 2 * mb;
        let (tl, tr) = self.tracks[base..base + 2 * mb].split_at_mut(mb);
        (
            &mut tl[..frames],
            &mut tr[..frames],
            &mut dl[..frames],
            &mut dr[..frames],
        )
    }

    /// Source and destination blocks of two distinct tracks (`src < dst`).
    #[inline]
    pub fn track_pair(
        &mut self,
        src: usize,
        dst: usize,
        frames: usize,
    ) -> (&[f32], &[f32], &mut [f32], &mut [f32]) {
        debug_assert!(src < dst);
        let mb = self.max_block;
        let (head, tail) = self.tracks.split_at_mut(dst * 2 * mb);
        let s = &head[src * 2 * mb..src * 2 * mb + 2 * mb];
        let (sl, sr) = s.split_at(mb);
        let (dl, dr) = tail[..2 * mb].split_at_mut(mb);
        (
            &sl[..frames],
            &sr[..frames],
            &mut dl[..frames],
            &mut dr[..frames],
        )
    }
}
