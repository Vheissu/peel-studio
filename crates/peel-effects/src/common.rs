//! Shared plumbing for every effect: parameter storage, sample-accurate
//! event splitting, and a few small DSP helpers.

use peel_dsp::delay::Lfo;
use peel_dsp::device::{DeviceDescriptor, DeviceEvent, EventKind, ProcessContext};
use peel_dsp::param::ParamId;

/// Plain parameter values for one device, indexed by descriptor order.
///
/// Allocated once when the device is created (a control-thread operation).
/// Lookups by id are a short linear scan, which only happens when a
/// parameter actually changes.
pub(crate) struct ParamStore {
    desc: &'static DeviceDescriptor,
    values: Box<[f32]>,
}

impl ParamStore {
    pub fn new(desc: &'static DeviceDescriptor) -> ParamStore {
        ParamStore {
            desc,
            values: desc.params.iter().map(|p| p.default).collect(),
        }
    }

    pub fn descriptor(&self) -> &'static DeviceDescriptor {
        self.desc
    }

    fn index(&self, id: ParamId) -> Option<usize> {
        self.desc.params.iter().position(|p| p.id == id)
    }

    /// Clamps and stores a value. Returns the stored value, or `None` for an
    /// unknown id.
    pub fn set(&mut self, id: ParamId, value: f32) -> Option<f32> {
        let i = self.index(id)?;
        let v = self.desc.params[i].clamp(value);
        self.values[i] = v;
        Some(v)
    }

    /// Current plain value, or 0 for an unknown id.
    pub fn get(&self, id: ParamId) -> f32 {
        self.index(id).map_or(0.0, |i| self.values[i])
    }

    /// Values in descriptor order.
    pub fn values(&self) -> &[f32] {
        &self.values
    }
}

/// What the generic device plumbing needs from an effect.
pub(crate) trait Fx {
    fn store(&self) -> &ParamStore;
    fn store_mut(&mut self) -> &mut ParamStore;
    /// Allocates buffers and recomputes everything that depends on the
    /// sample rate. Must finish by calling [`refresh_all`] and snapping its
    /// smoothers to the stored values.
    fn setup(&mut self, sample_rate: f32, max_block: usize);
    /// A stored parameter changed; update derived DSP state (and start any
    /// smoothing). `value` is already clamped.
    fn param_changed(&mut self, id: ParamId, value: f32);
    /// Clears all audio state without allocating.
    fn clear(&mut self);
    /// Processes one contiguous run of frames in place.
    fn render(&mut self, ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]);
    /// Like `render`, with a sidechain key of the same length. Effects
    /// without a sidechain input ignore it.
    fn render_keyed(
        &mut self,
        ctx: &ProcessContext,
        left: &mut [f32],
        right: &mut [f32],
        key: (&[f32], &[f32]),
    ) {
        let _ = key;
        self.render(ctx, left, right);
    }
    /// Gain reduction in dB for a meter, if the effect has one.
    fn reduction(&self) -> Option<f32> {
        None
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail(&self) -> Option<f32> {
        Some(0.0)
    }
    fn silent(&self) -> bool {
        false
    }
}

/// Stores a value and tells the DSP about it.
pub(crate) fn set_value<F: Fx>(fx: &mut F, id: ParamId, value: f32) {
    if let Some(v) = fx.store_mut().set(id, value) {
        fx.param_changed(id, v);
    }
}

/// Replays every stored value into [`Fx::param_changed`].
pub(crate) fn refresh_all<F: Fx>(fx: &mut F) {
    let desc = fx.store().descriptor();
    for (i, p) in desc.params.iter().enumerate() {
        let v = fx.store().values()[i];
        fx.param_changed(p.id, v);
    }
}

/// Runs a block, splitting it at every parameter event so changes land on
/// the exact frame. The context's `beat_position` is advanced for each
/// segment so tempo-synced effects stay aligned.
pub(crate) fn run_events<F: Fx>(
    fx: &mut F,
    ctx: &ProcessContext,
    events: &[DeviceEvent],
    left: &mut [f32],
    right: &mut [f32],
    key: Option<(&[f32], &[f32])>,
) {
    let frames = left.len().min(right.len());
    let key = key.filter(|(kl, kr)| kl.len() >= frames && kr.len() >= frames);
    let (left, right) = (&mut left[..frames], &mut right[..frames]);
    // A NaN or infinity must never enter filter or feedback state, where it
    // would circulate forever. Treat it as silence at the door.
    for s in left.iter_mut().chain(right.iter_mut()) {
        if !s.is_finite() {
            *s = 0.0;
        }
    }
    // Garbage transport values must not reach tempo-synced oscillators.
    let mut clean = *ctx;
    if !(clean.tempo_bpm.is_finite() && clean.tempo_bpm >= 1.0) {
        clean.tempo_bpm = 120.0;
    }
    if !clean.beat_position.is_finite() {
        clean.beat_position = 0.0;
    }
    if !(clean.sample_rate.is_finite() && clean.sample_rate >= 1_000.0) {
        clean.sample_rate = 48_000.0;
    }
    let ctx = &clean;
    let beats_per_frame = ctx.tempo_bpm / (60.0 * f64::from(ctx.sample_rate));
    let mut pos = 0usize;

    let segment = |fx: &mut F, from: usize, to: usize, left: &mut [f32], right: &mut [f32]| {
        if to > from {
            let mut c = *ctx;
            c.beat_position += from as f64 * beats_per_frame;
            match key {
                Some((kl, kr)) => fx.render_keyed(
                    &c,
                    &mut left[from..to],
                    &mut right[from..to],
                    (&kl[from..to], &kr[from..to]),
                ),
                None => fx.render(&c, &mut left[from..to], &mut right[from..to]),
            }
        }
    };

    for ev in events {
        let at = (ev.offset as usize).min(frames);
        segment(fx, pos, at, left, right);
        pos = pos.max(at);
        match ev.kind {
            EventKind::Param { id, value } => set_value(fx, id, value),
            EventKind::AllSoundOff => fx.clear(),
            _ => {}
        }
    }
    segment(fx, pos, frames, left, right);
}

/// Implements `peel_dsp::Device` for an effect type that implements
/// [`Fx`](crate::common::Fx).
macro_rules! impl_device {
    ($ty:ty, $desc:expr) => {
        impl ::peel_dsp::device::Device for $ty {
            fn descriptor(&self) -> &'static ::peel_dsp::device::DeviceDescriptor {
                &$desc
            }
            fn prepare(&mut self, sample_rate: f32, max_block: usize) {
                $crate::common::Fx::setup(self, sample_rate, max_block);
            }
            fn reset(&mut self) {
                $crate::common::Fx::clear(self);
            }
            fn set_param(&mut self, id: ::peel_dsp::param::ParamId, value: f32) {
                $crate::common::set_value(self, id, value);
            }
            fn param(&self, id: ::peel_dsp::param::ParamId) -> f32 {
                $crate::common::Fx::store(self).get(id)
            }
            fn process(
                &mut self,
                ctx: &::peel_dsp::device::ProcessContext,
                events: &[::peel_dsp::device::DeviceEvent],
                left: &mut [f32],
                right: &mut [f32],
            ) {
                $crate::common::run_events(self, ctx, events, left, right, None);
            }
            fn process_keyed(
                &mut self,
                ctx: &::peel_dsp::device::ProcessContext,
                events: &[::peel_dsp::device::DeviceEvent],
                left: &mut [f32],
                right: &mut [f32],
                key_left: &[f32],
                key_right: &[f32],
            ) {
                $crate::common::run_events(
                    self,
                    ctx,
                    events,
                    left,
                    right,
                    Some((key_left, key_right)),
                );
            }
            fn latency(&self) -> u32 {
                $crate::common::Fx::latency_samples(self)
            }
            fn reduction_db(&self) -> Option<f32> {
                $crate::common::Fx::reduction(self)
            }
            fn tail_seconds(&self) -> Option<f32> {
                $crate::common::Fx::tail(self)
            }
            fn is_silent(&self) -> bool {
                $crate::common::Fx::silent(self)
            }
        }
    };
}
pub(crate) use impl_device;

/// Counts how long the input has been quiet, for `is_silent`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SilenceTracker {
    quiet: u32,
}

impl SilenceTracker {
    /// Level below which input counts as silence (about -140 dBFS).
    const FLOOR: f32 = 1e-7;

    pub fn reset(&mut self) {
        self.quiet = 0;
    }

    /// Call with the *input* before processing it.
    pub fn feed(&mut self, left: &[f32], right: &[f32]) {
        let loud = left
            .iter()
            .chain(right.iter())
            .any(|x| x.abs() > Self::FLOOR);
        if loud {
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(left.len() as u32);
        }
    }

    pub fn quiet_for(&self, samples: u32) -> bool {
        self.quiet >= samples
    }
}

/// Equal-power crossfade gains `(dry, wet)` for `mix` in 0..=1.
#[inline]
pub(crate) fn equal_power(mix: f32) -> (f32, f32) {
    let a = mix.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    (a.cos(), a.sin())
}

/// Transparent below 1.0, then a smooth saturation toward 2.0. Used to
/// bound feedback paths without coloring normal levels.
#[inline]
pub(crate) fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= 1.0 {
        x
    } else {
        x.signum() * (1.0 + (a - 1.0).tanh())
    }
}

/// Smoothing ramp used for gains and mixes, in seconds.
pub(crate) const RAMP_SECONDS: f32 = 0.015;

/// Musical note values (in quarter notes) a tempo-synced delay snaps to:
/// 1/32 through 2 bars, straight, triplet and dotted.
pub(crate) const NOTE_VALUES: [f32; 19] = [
    0.125,
    1.0 / 6.0,
    0.1875,
    0.25,
    1.0 / 3.0,
    0.375,
    0.5,
    2.0 / 3.0,
    0.75,
    1.0,
    4.0 / 3.0,
    1.5,
    2.0,
    8.0 / 3.0,
    3.0,
    4.0,
    16.0 / 3.0,
    6.0,
    8.0,
];

/// Snaps a beat length to the nearest entry of [`NOTE_VALUES`] (ratio-wise).
pub(crate) fn snap_beats(beats: f32) -> f32 {
    let b = beats.max(1e-3);
    let mut best = NOTE_VALUES[0];
    let mut best_err = f32::MAX;
    for &v in &NOTE_VALUES {
        let err = (b / v).ln().abs();
        if err < best_err {
            best_err = err;
            best = v;
        }
    }
    best
}

/// Points an LFO at a tempo-synced period of `beats` quarter notes
/// (snapped to a musical value). The LFO free-runs at the synced rate; while
/// the transport plays its phase is locked to the song position.
pub(crate) fn lfo_follow_tempo(lfo: &mut Lfo, ctx: &ProcessContext, beats: f32) {
    let period = f64::from(snap_beats(beats));
    let inc = ctx.tempo_bpm / (60.0 * f64::from(ctx.sample_rate) * period);
    lfo.set_increment(inc as f32);
    if ctx.playing {
        let p = ctx.beat_position / period;
        let frac = (p - p.floor()) as f32;
        lfo.phase = if frac >= 1.0 { 0.0 } else { frac };
    }
}

/// Triangle wave in `-1..=1` for a phase in turns (starts at 0, rising).
#[inline]
pub(crate) fn tri_turns(phase: f32) -> f32 {
    let p = phase - phase.floor();
    if p < 0.25 {
        4.0 * p
    } else if p < 0.75 {
        2.0 - 4.0 * p
    } else {
        4.0 * p - 4.0
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Helpers shared by the unit tests of the modulation effects.
    use peel_dsp::device::{Device, ProcessContext};

    pub const SR: f32 = 48_000.0;

    pub fn sine(freq: f64, i: usize, amp: f32) -> f32 {
        amp * (std::f64::consts::TAU * (i as f64 * freq / f64::from(SR)).fract()).sin() as f32
    }

    /// Deterministic noise in -1..1.
    pub struct Rng(pub u32);
    impl Rng {
        pub fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        }
    }

    /// Applies a preset by key to a freshly defaulted, prepared device.
    pub fn with_preset(type_id: &str, preset: &peel_dsp::Preset) -> Box<dyn Device> {
        let mut d = crate::create(type_id).unwrap();
        peel_dsp::device::apply_defaults(d.as_mut());
        for (key, v) in preset.params {
            let id = d.descriptor().param_by_key(key).unwrap().id;
            d.set_param(id, *v);
        }
        d.prepare(SR, 256);
        d
    }

    /// Every preset yields finite, bounded audio on a noisy stereo signal.
    pub fn presets_are_finite(type_id: &str) {
        for preset in crate::presets(type_id) {
            let mut d = with_preset(type_id, preset);
            let mut rng = Rng(7);
            let ctx = ProcessContext::new(SR);
            for block in 0..120 {
                let mut l: Vec<f32> = (0..256)
                    .map(|i| sine(110.0, block * 256 + i, 0.5) + 0.2 * rng.next())
                    .collect();
                let mut r: Vec<f32> = l.iter().map(|v| v * 0.8).collect();
                d.process(&ctx, &[], &mut l, &mut r);
                assert!(
                    l.iter()
                        .chain(r.iter())
                        .all(|v| v.is_finite() && v.abs() < 8.0),
                    "{type_id}/{}",
                    preset.name
                );
            }
        }
    }

    /// Sweeps every parameter between 10% and 90% of its range every 480
    /// samples while a sine plays; no sample-to-sample step may be large.
    pub fn automation_is_click_free(type_id: &str, limit: f32) {
        let desc = crate::descriptors()
            .iter()
            .find(|d| d.type_id == type_id)
            .unwrap();
        for p in desc.params {
            let mut d = crate::create(type_id).unwrap();
            peel_dsp::device::apply_defaults(d.as_mut());
            d.prepare(SR, 480);
            let ctx = ProcessContext::new(SR);
            let mut prev = 0.0f32;
            let mut worst = 0.0f32;
            for block in 0..80 {
                let norm = if block % 2 == 0 { 0.1 } else { 0.9 };
                d.set_param(p.id, p.from_normalized(norm));
                let mut l: Vec<f32> = (0..480)
                    .map(|i| sine(220.0, block * 480 + i, 0.5))
                    .collect();
                let mut r = l.clone();
                d.process(&ctx, &[], &mut l, &mut r);
                if block >= 10 {
                    for v in &l {
                        worst = worst.max((v - prev).abs());
                        prev = *v;
                    }
                } else {
                    prev = *l.last().unwrap();
                }
            }
            assert!(worst < limit, "{type_id}.{}: step {worst}", p.key);
        }
    }

    /// Goertzel magnitude of `x` at `freq`.
    pub fn tone_level(x: &[f32], freq: f64) -> f64 {
        let w = std::f64::consts::TAU * freq / f64::from(SR);
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            re += f64::from(*v) * (w * i as f64).cos();
            im += f64::from(*v) * (w * i as f64).sin();
        }
        (re * re + im * im).sqrt() * 2.0 / x.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_musical_values() {
        assert_eq!(snap_beats(0.5), 0.5);
        assert_eq!(snap_beats(0.52), 0.5);
        assert!((snap_beats(0.72) - 0.75).abs() < 1e-6);
        assert!((snap_beats(0.68) - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(snap_beats(100.0), 8.0);
        assert_eq!(snap_beats(0.0), 0.125);
    }

    #[test]
    fn soft_limit_is_transparent_then_bounded() {
        assert_eq!(soft_limit(0.7), 0.7);
        assert_eq!(soft_limit(-1.0), -1.0);
        assert!(soft_limit(50.0) <= 2.0);
        assert!(soft_limit(1.1) > 1.0 && soft_limit(1.1) < 1.11);
    }

    #[test]
    fn equal_power_extremes() {
        let (d, w) = equal_power(0.0);
        assert!((d - 1.0).abs() < 1e-6 && w.abs() < 1e-6);
        let (d, w) = equal_power(1.0);
        assert!(d.abs() < 1e-6 && (w - 1.0).abs() < 1e-6);
    }
}
