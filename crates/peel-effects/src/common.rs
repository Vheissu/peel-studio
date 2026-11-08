//! Shared plumbing for every effect: parameter storage, sample-accurate
//! event splitting, and a few small DSP helpers.

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
) {
    let frames = left.len().min(right.len());
    let (left, right) = (&mut left[..frames], &mut right[..frames]);
    // A NaN or infinity must never enter filter or feedback state, where it
    // would circulate forever. Treat it as silence at the door.
    for s in left.iter_mut().chain(right.iter_mut()) {
        if !s.is_finite() {
            *s = 0.0;
        }
    }
    let beats_per_frame = ctx.tempo_bpm / (60.0 * f64::from(ctx.sample_rate.max(1.0)));
    let mut pos = 0usize;

    let segment = |fx: &mut F, from: usize, to: usize, left: &mut [f32], right: &mut [f32]| {
        if to > from {
            let mut c = *ctx;
            c.beat_position += from as f64 * beats_per_frame;
            fx.render(&c, &mut left[from..to], &mut right[from..to]);
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
                $crate::common::run_events(self, ctx, events, left, right);
            }
            fn latency(&self) -> u32 {
                $crate::common::Fx::latency_samples(self)
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
