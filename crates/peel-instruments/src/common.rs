//! Shared plumbing for the native instruments.
//!
//! Every instrument implements [`Synth`], a small event-driven interface, and
//! is wrapped in [`Instrument`], which implements `peel_dsp::Device` once for
//! all of them: parameter storage and clamping, sample-accurate event
//! splitting, and finite-output protection.
//!
//! Block-size independence: instruments process sample by sample and keep
//! their control-rate counters across calls, so splitting the same timeline
//! into different block sizes gives the same output.

use std::sync::Arc;

use peel_dsp::filter::tanh_approx;
use peel_dsp::voice::{StealPolicy, VoiceAllocator, VoiceState};
use peel_dsp::{
    Device, DeviceDescriptor, DeviceEvent, EventKind, ParamId, ProcessContext, SampleBuffer,
};

/// Control-rate interval in samples. Filter coefficients, parameter smoothing
/// and other expensive per-voice updates run once per interval.
pub(crate) const CTRL: usize = 16;

/// Length of the anti-click fade applied to stolen or choked voices.
pub(crate) const STEAL_FADE_MS: f32 = 3.0;

/// Hard safety rail on instrument output; only pathological settings ever
/// reach it (it is +12 dBFS).
const OUTPUT_RAIL: f32 = 4.0;

/// The event-driven core of an instrument.
pub(crate) trait Synth: Send + 'static {
    /// Allocates voices and recomputes sample-rate dependent state. May
    /// allocate; the caller applies every parameter afterwards.
    fn prepare(&mut self, sample_rate: f32, max_block: usize);

    /// Silences everything without allocating.
    fn reset(&mut self);

    /// A parameter changed. `immediate` skips smoothing (initial setup and
    /// changes while the instrument is silent).
    fn param_changed(&mut self, id: ParamId, value: f32, immediate: bool);

    fn note_on(&mut self, key: u8, velocity: f32, id: u32);
    fn note_off(&mut self, key: u8, velocity: f32, id: u32);
    fn all_notes_off(&mut self);
    fn all_sound_off(&mut self);

    /// Overwrites `left`/`right` with the instrument output.
    fn render(&mut self, ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]);

    fn is_silent(&self) -> bool;

    fn set_sample(&mut self, sample: Option<Arc<SampleBuffer>>) -> Option<Arc<SampleBuffer>> {
        sample
    }
}

/// Adapts a [`Synth`] to the `Device` trait.
pub(crate) struct Instrument<S: Synth> {
    desc: &'static DeviceDescriptor,
    values: Vec<f32>,
    synth: S,
}

impl<S: Synth> Instrument<S> {
    pub(crate) fn new(desc: &'static DeviceDescriptor, synth: S) -> Instrument<S> {
        let mut inst = Instrument {
            desc,
            values: desc.params.iter().map(|p| p.default).collect(),
            synth,
        };
        // Usable even if the host never calls `prepare`.
        inst.prepare(48_000.0, 512);
        inst
    }

    fn apply(&mut self, id: ParamId, value: f32) {
        let Some(idx) = self.desc.params.iter().position(|p| p.id == id) else {
            return;
        };
        let v = self.desc.params[idx].clamp(value);
        self.values[idx] = v;
        // Changes while nothing sounds cannot be heard as zipper noise, so
        // snap to them; this also makes a preset fully applied before the
        // first note.
        let immediate = self.synth.is_silent();
        self.synth.param_changed(id, v, immediate);
    }

    fn handle(&mut self, kind: EventKind) {
        match kind {
            EventKind::NoteOn {
                key,
                velocity,
                voice,
            } => {
                let vel = if velocity.is_finite() {
                    velocity.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                self.synth.note_on(key.min(127), vel, voice);
            }
            EventKind::NoteOff {
                key,
                velocity,
                voice,
            } => {
                let vel = if velocity.is_finite() {
                    velocity.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                self.synth.note_off(key.min(127), vel, voice);
            }
            EventKind::Param { id, value } => self.apply(id, value),
            EventKind::AllNotesOff => self.synth.all_notes_off(),
            EventKind::AllSoundOff => self.synth.all_sound_off(),
        }
    }
}

impl<S: Synth> Device for Instrument<S> {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        self.desc
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize) {
        let sr = if sample_rate.is_finite() {
            sample_rate.clamp(8_000.0, 384_000.0)
        } else {
            48_000.0
        };
        self.synth.prepare(sr, max_block.max(1));
        for (p, &v) in self.desc.params.iter().zip(self.values.iter()) {
            self.synth.param_changed(p.id, v, true);
        }
    }

    fn reset(&mut self) {
        self.synth.reset();
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        self.apply(id, value);
    }

    fn param(&self, id: ParamId) -> f32 {
        self.desc
            .params
            .iter()
            .position(|p| p.id == id)
            .map_or(0.0, |i| self.values[i])
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        events: &[DeviceEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let frames = left.len().min(right.len());
        let mut pos = 0usize;
        let mut next_event = 0usize;
        let beats_per_sample = if ctx.sample_rate > 0.0 {
            ctx.tempo_bpm.clamp(1.0, 999.0) / 60.0 / ctx.sample_rate as f64
        } else {
            0.0
        };
        while pos < frames {
            while next_event < events.len() && (events[next_event].offset as usize) <= pos {
                self.handle(events[next_event].kind);
                next_event += 1;
            }
            let end = match events.get(next_event) {
                Some(e) => (e.offset as usize).min(frames),
                None => frames,
            };
            let mut seg_ctx = *ctx;
            seg_ctx.beat_position = ctx.beat_position + pos as f64 * beats_per_sample;
            self.synth
                .render(&seg_ctx, &mut left[pos..end], &mut right[pos..end]);
            pos = end;
        }
        // Events at or past the end of the block (not expected) still apply so
        // note-offs are never lost.
        while next_event < events.len() {
            self.handle(events[next_event].kind);
            next_event += 1;
        }
        for s in left[..frames].iter_mut().chain(right[..frames].iter_mut()) {
            *s = if s.is_finite() {
                s.clamp(-OUTPUT_RAIL, OUTPUT_RAIL)
            } else {
                0.0
            };
        }
        // Any unmatched tail of a longer buffer is silenced.
        for s in left[frames..].iter_mut().chain(right[frames..].iter_mut()) {
            *s = 0.0;
        }
    }

    fn is_silent(&self) -> bool {
        self.synth.is_silent()
    }

    fn set_sample(&mut self, sample: Option<Arc<SampleBuffer>>) -> Option<Arc<SampleBuffer>> {
        self.synth.set_sample(sample)
    }
}

/// A parameter value that approaches its target once per control interval.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ramp {
    cur: f32,
    tgt: f32,
}

impl Ramp {
    pub(crate) const fn new(v: f32) -> Ramp {
        Ramp { cur: v, tgt: v }
    }

    pub(crate) fn set(&mut self, v: f32, immediate: bool) {
        self.tgt = v;
        if immediate {
            self.cur = v;
        }
    }

    /// Moves towards the target; `k` comes from [`ramp_coef`].
    #[inline]
    pub(crate) fn tick(&mut self, k: f32) -> f32 {
        self.cur += (self.tgt - self.cur) * k;
        if (self.tgt - self.cur).abs() < 1.0e-6 * (1.0 + self.tgt.abs()) {
            self.cur = self.tgt;
        }
        self.cur
    }

    #[inline]
    pub(crate) fn get(&self) -> f32 {
        self.cur
    }
}

/// One-pole coefficient for [`Ramp::tick`] with a ~10 ms time constant at the
/// control rate.
pub(crate) fn ramp_coef(sample_rate: f32) -> f32 {
    1.0 - (-(CTRL as f32) / (0.010 * sample_rate)).exp()
}

pub(crate) use peel_dsp::voice::StealFade as Fade;

/// Behaviour the voice pool needs from a voice.
pub(crate) trait PoolVoice {
    fn is_active(&self) -> bool;
    /// Rough loudness used for quietest-voice stealing.
    fn level(&self) -> f32;
    fn fade_out(&mut self, sample_rate: f32);
    /// Stops immediately.
    fn kill(&mut self);
}

/// Preallocated voices plus the allocator that manages them.
pub(crate) struct Pool<V: PoolVoice> {
    pub voices: Vec<V>,
    pub alloc: VoiceAllocator,
    sample_rate: f32,
}

impl<V: PoolVoice> Pool<V> {
    pub(crate) fn new(
        capacity: usize,
        polyphony: usize,
        policy: StealPolicy,
        sample_rate: f32,
        make: impl Fn() -> V,
    ) -> Pool<V> {
        let alloc = VoiceAllocator::new(capacity, polyphony, policy);
        let voices = (0..alloc.capacity()).map(|_| make()).collect();
        Pool {
            voices,
            alloc,
            sample_rate,
        }
    }

    /// Releases bookkeeping for voices that have finished and refreshes the
    /// levels used for stealing.
    pub(crate) fn sync(&mut self) {
        for (i, v) in self.voices.iter().enumerate() {
            if v.is_active() {
                self.alloc.set_level(i, v.level());
            } else if self.alloc.slot(i).state != VoiceState::Free {
                self.alloc.free(i);
            }
        }
    }

    /// Picks the slot for a new note. The caller initializes `voices[slot]`.
    pub(crate) fn begin(&mut self, key: u8, id: u32) -> usize {
        self.sync();
        let a = self.alloc.allocate(key, id);
        if let Some(s) = a.stolen {
            self.voices[s].fade_out(self.sample_rate);
        }
        if let Some(c) = a.cut {
            self.voices[c].kill();
        }
        a.slot
    }

    /// Marks the voices started with `id` as releasing and calls `f` on each.
    pub(crate) fn release_id(&mut self, id: u32, mut f: impl FnMut(&mut V)) {
        let mut found = [0usize; 8];
        let n = self.alloc.find_held(id, &mut found);
        for &slot in &found[..n] {
            self.alloc.release(slot);
            f(&mut self.voices[slot]);
        }
    }

    /// Marks every held voice releasing and calls `f` on each.
    pub(crate) fn release_all(&mut self, mut f: impl FnMut(&mut V)) {
        for i in 0..self.voices.len() {
            if self.alloc.slot(i).state == VoiceState::Held {
                self.alloc.release(i);
                f(&mut self.voices[i]);
            }
        }
    }

    pub(crate) fn kill_all(&mut self) {
        for v in &mut self.voices {
            v.kill();
        }
        self.alloc.reset();
    }

    pub(crate) fn is_silent(&self) -> bool {
        !self.voices.iter().any(PoolVoice::is_active)
    }
}

/// Velocity (0..1) to a gain that follows a gentle curve; `sens` blends from
/// fixed level (0) to fully velocity controlled (1).
#[inline]
pub(crate) fn vel_gain(velocity: f32, sens: f32) -> f32 {
    (1.0 - sens) + sens * velocity.powf(1.4)
}

/// Waveshaper settings for a 0..1 drive amount: `(gain, normalization)`.
#[inline]
pub(crate) fn drive_curve(drive: f32) -> (f32, f32) {
    let g = 1.0 + 8.0 * drive * drive;
    (g, 1.0 / tanh_approx(g))
}

/// Soft saturation that keeps a full-scale input at full scale.
#[inline]
pub(crate) fn shape(x: f32, gain: f32, norm: f32) -> f32 {
    tanh_approx(x * gain) * norm
}

#[inline]
pub(crate) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Semitone offset of `key` from middle C (key 60), the drum transpose.
#[inline]
pub(crate) fn key_offset(key: u8) -> f32 {
    key as f32 - 60.0
}

/// Pitch ratio for a semitone offset.
#[inline]
pub(crate) fn st_ratio(st: f32) -> f32 {
    (st * (std::f32::consts::LN_2 / 12.0)).exp()
}
