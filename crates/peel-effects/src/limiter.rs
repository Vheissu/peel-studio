//! Lookahead brickwall limiter.
//!
//! A thin wrapper around [`LookaheadLimiter`] with an input gain stage. The
//! lookahead is fixed at 4 ms and reported through `latency()`, so the host
//! can compensate; it does not change with any parameter. Output sample
//! peaks never exceed the ceiling. With *True Peak* on, the detector also
//! looks at the midpoint between samples, which catches most inter-sample
//! overs (typically within 1 dB of a 4x oversampled meter).

use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::dynamics::LookaheadLimiter;
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

pub const ID_INPUT: ParamId = 0;
pub const ID_CEILING: ParamId = 1;
pub const ID_RELEASE: ParamId = 2;
pub const ID_TRUE_PEAK: ParamId = 3;

/// Lookahead in seconds.
const LOOKAHEAD_SECONDS: f32 = 0.004;

static PARAMS: [ParamDescriptor; 4] = [
    ParamDescriptor::linear(
        ID_INPUT,
        "input",
        "Input",
        "Level",
        -12.0,
        24.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_CEILING,
        "ceiling",
        "Ceiling",
        "Level",
        -12.0,
        0.0,
        -1.0,
        Unit::Decibels,
    ),
    ParamDescriptor::log(
        ID_RELEASE,
        "release",
        "Release",
        "Timing",
        1.0,
        1_000.0,
        100.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::toggle(ID_TRUE_PEAK, "true_peak", "True Peak", "Detector", true),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.limiter",
    name: "Limiter",
    description: "Lookahead brickwall limiter with true-peak detection",
    category: "Dynamics",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Transparent",
        params: &[],
    },
    Preset {
        name: "Master Safety",
        params: &[("ceiling", -0.3), ("release", 250.0)],
    },
    Preset {
        name: "Loud Master",
        params: &[("input", 6.0), ("ceiling", -0.5), ("release", 60.0)],
    },
    Preset {
        name: "Drum Bus Catch",
        params: &[("input", 3.0), ("ceiling", -2.0), ("release", 30.0)],
    },
    Preset {
        name: "Soft Peaks",
        params: &[("input", 2.0), ("ceiling", -1.0), ("release", 400.0)],
    },
];

/// Brickwall limiter. Read the current gain reduction with
/// [`Limiter::gain_reduction_db`].
pub struct Limiter {
    store: ParamStore,
    sample_rate: f32,
    core: LookaheadLimiter,
    input: Smoothed,
    silence: SilenceTracker,
}

impl Default for Limiter {
    fn default() -> Self {
        Limiter::new()
    }
}

impl Limiter {
    pub fn new() -> Limiter {
        let mut l = Limiter {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            core: LookaheadLimiter::new(192),
            input: Smoothed::new(1.0),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut l, 48_000.0, 512);
        l
    }

    /// Gain reduction in dB (zero or positive) applied to the latest sample.
    pub fn gain_reduction_db(&self) -> f32 {
        self.core.gain_reduction_db()
    }
}

impl Fx for Limiter {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn reduction(&self) -> Option<f32> {
        Some(self.core.gain_reduction_db())
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let look = (self.sample_rate * LOOKAHEAD_SECONDS).round() as usize;
        self.core = LookaheadLimiter::new(look);
        self.input.set_ramp(self.sample_rate, RAMP_SECONDS);
        refresh_all(self);
        let t = self.input.target();
        self.input.set_immediate(t);
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_INPUT => self.input.set_target(db_to_gain(value)),
            ID_CEILING => self.core.set_ceiling_db(value),
            ID_RELEASE => self.core.set_release(value * 0.001, self.sample_rate),
            ID_TRUE_PEAK => self.core.set_true_peak(value >= 0.5),
            _ => {}
        }
    }

    fn clear(&mut self) {
        self.core.reset();
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = self.input.next();
            let (a, b) = self.core.process(*l * g, *r * g);
            *l = a;
            *r = b;
        }
    }

    fn latency_samples(&self) -> u32 {
        self.core.lookahead() as u32
    }

    fn tail(&self) -> Option<f32> {
        Some(self.core.lookahead() as f32 / self.sample_rate)
    }

    fn silent(&self) -> bool {
        self.silence.quiet_for(self.core.lookahead() as u32 + 8)
    }
}

impl_device!(Limiter, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;

    #[test]
    fn never_exceeds_ceiling_on_hot_program() {
        for ceiling in [-0.3f32, -3.0, -9.0] {
            let mut lim = Limiter::new();
            lim.set_param(ID_INPUT, 18.0);
            lim.set_param(ID_CEILING, ceiling);
            lim.prepare(SR, 256);
            let limit = db_to_gain(ceiling);
            let ctx = ProcessContext::new(SR);
            let mut seed = 3u32;
            let mut worst = 0.0f32;
            for block in 0..400 {
                let mut l: Vec<f32> = (0..256)
                    .map(|i| {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        let n = ((seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0;
                        0.9 * (((block * 256 + i) as f32) * 0.03).sin() + 0.6 * n
                    })
                    .collect();
                let mut r: Vec<f32> = l.iter().map(|v| -v * 0.8).collect();
                lim.process(&ctx, &[], &mut l, &mut r);
                worst = l.iter().chain(r.iter()).fold(worst, |m, v| m.max(v.abs()));
            }
            assert!(
                worst <= limit + 1e-6,
                "ceiling {ceiling}: peak {worst} > {limit}"
            );
            assert!(worst > limit * 0.95);
        }
    }

    #[test]
    fn reports_constant_latency_and_aligns_output() {
        let mut lim = Limiter::new();
        lim.prepare(SR, 256);
        let lat = lim.latency() as usize;
        assert_eq!(lat, (SR * 0.004) as usize);
        let ctx = ProcessContext::new(SR);
        let mut l = vec![0.0; 512];
        l[10] = 0.5;
        let mut r = l.clone();
        lim.process(&ctx, &[], &mut l, &mut r);
        assert!((l[10 + lat] - 0.5).abs() < 1e-6);
        assert!(l[10].abs() < 1e-6);
        // Latency does not depend on parameters.
        lim.set_param(ID_RELEASE, 5.0);
        lim.set_param(ID_TRUE_PEAK, 0.0);
        assert_eq!(lim.latency() as usize, lat);
    }

    #[test]
    fn sample_rate_change_updates_latency() {
        let mut lim = Limiter::new();
        lim.prepare(44_100.0, 256);
        let a = lim.latency();
        lim.prepare(96_000.0, 256);
        assert!(lim.latency() > a);
    }

    #[test]
    fn tail_covers_the_delayed_signal() {
        let mut lim = Limiter::new();
        lim.prepare(SR, 128);
        assert!((lim.tail_seconds().unwrap() - 0.004).abs() < 1e-4);
    }
}
