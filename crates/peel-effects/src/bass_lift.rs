//! Bass Lift: a low-end enhancer for clarity on small speakers.
//!
//! The signal is split at the crossover with a Linkwitz-Riley (4th-order)
//! pair, so the bands sum back flat. The low band goes through four stages:
//!
//! * **Punch** reshapes its dynamics. A fast and a slow power follower are
//!   compared (stereo-linked); where the fast one is higher (an onset) the
//!   band is lifted, where it is lower (the sustain) it is pulled down.
//! * **Harmonics** exploit the missing-fundamental effect. A saturated plus
//!   full-wave rectified copy of the low band is band-passed to roughly
//!   100-400 Hz (the pass band starts above the crossover so the
//!   fundamental is left out) and blended back in. Small speakers cannot
//!   play the fundamental, but the ear infers it from these overtones.
//! * **Sub** sets the gain of the low band itself.
//! * **Ceiling** soft-clips the low band (fundamental plus harmonics) with a
//!   knee, so boosting adds weight instead of peaks.
//!
//! Finally the sum passes a safety soft clip that is transparent below 0.97
//! and never exceeds 0 dBFS, so no combination of settings can overload the
//! output.

use peel_dsp::biquad::{Coeffs, FilterKind, StereoBiquad};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain, one_pole_coeff, sanitize};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

pub const ID_FREQ: ParamId = 0;
pub const ID_HARMONICS: ParamId = 1;
pub const ID_PUNCH: ParamId = 2;
pub const ID_SUB: ParamId = 3;
pub const ID_CEILING: ParamId = 4;
pub const ID_OUTPUT: ParamId = 5;

/// Gain applied to the harmonic band at `harmonics = 1`.
const HARMONIC_GAIN: f32 = 1.4;
/// Output level above which the safety clip starts to round the signal.
const SAFETY_KNEE: f32 = 0.97;

static PARAMS: [ParamDescriptor; 6] = [
    ParamDescriptor::log(
        ID_FREQ,
        "freq",
        "Frequency",
        "Crossover",
        40.0,
        200.0,
        90.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_HARMONICS,
        "harmonics",
        "Harmonics",
        "Harmonics",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_PUNCH,
        "punch",
        "Punch",
        "Low Band",
        0.0,
        1.0,
        0.25,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_SUB,
        "sub",
        "Sub",
        "Low Band",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_CEILING,
        "ceiling",
        "Ceiling",
        "Low Band",
        -12.0,
        0.0,
        -1.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_OUTPUT,
        "output",
        "Output",
        "Output",
        -24.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.bass_lift",
    name: "Bass Lift",
    description: "Low-end enhancer with harmonics for small speakers",
    category: "EQ",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Subtle",
        params: &[],
    },
    Preset {
        name: "Small Speakers",
        params: &[
            ("freq", 120.0),
            ("harmonics", 0.75),
            ("punch", 0.3),
            ("sub", -3.0),
            ("ceiling", -2.0),
        ],
    },
    Preset {
        name: "Punchy Kick",
        params: &[
            ("freq", 70.0),
            ("harmonics", 0.4),
            ("punch", 0.8),
            ("sub", 2.0),
            ("ceiling", -1.5),
        ],
    },
    Preset {
        name: "Warm Bass",
        params: &[
            ("freq", 100.0),
            ("harmonics", 0.6),
            ("punch", 0.1),
            ("sub", 1.5),
            ("ceiling", -2.0),
        ],
    },
    Preset {
        name: "Sub Focus",
        params: &[
            ("freq", 55.0),
            ("harmonics", 0.2),
            ("punch", 0.4),
            ("sub", 5.0),
            ("ceiling", -2.5),
        ],
    },
];

/// Passes everything below `0.7 * ceiling` untouched, then bends smoothly
/// toward `ceiling`.
#[inline]
fn knee_clip(x: f32, ceiling: f32) -> f32 {
    let a = x.abs();
    let k = 0.7 * ceiling;
    if a <= k {
        x
    } else {
        let w = ceiling - k;
        (k + w * ((a - k) / w).tanh()).copysign(x)
    }
}

/// Final guard: transparent up to the knee, never above 1.0.
#[inline]
fn safety_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= SAFETY_KNEE {
        x
    } else {
        let w = 1.0 - SAFETY_KNEE;
        (SAFETY_KNEE + w * ((a - SAFETY_KNEE) / w).tanh())
            .min(1.0)
            .copysign(x)
    }
}

/// Bass Lift effect. See the module docs.
pub struct BassLift {
    store: ParamStore,
    sample_rate: f32,
    low: [StereoBiquad; 2],
    high: [StereoBiquad; 2],
    gen_high: [StereoBiquad; 2],
    gen_low: StereoBiquad,
    harmonics: Smoothed,
    punch: Smoothed,
    sub: Smoothed,
    ceiling: Smoothed,
    output: Smoothed,
    fast_power: f32,
    slow_power: f32,
    punch_gain: f32,
    fast_coeff: f32,
    slow_coeff: f32,
    gain_coeff: f32,
    silence: SilenceTracker,
}

impl Default for BassLift {
    fn default() -> Self {
        BassLift::new()
    }
}

impl BassLift {
    pub fn new() -> BassLift {
        let mut b = BassLift {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            low: Default::default(),
            high: Default::default(),
            gen_high: Default::default(),
            gen_low: StereoBiquad::default(),
            harmonics: Smoothed::new(0.0),
            punch: Smoothed::new(0.0),
            sub: Smoothed::new(1.0),
            ceiling: Smoothed::new(1.0),
            output: Smoothed::new(1.0),
            fast_power: 0.0,
            slow_power: 0.0,
            punch_gain: 1.0,
            fast_coeff: 0.0,
            slow_coeff: 0.0,
            gain_coeff: 0.0,
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut b, 48_000.0, 512);
        b
    }

    fn filters(&mut self) -> impl Iterator<Item = &mut StereoBiquad> {
        self.low
            .iter_mut()
            .chain(self.high.iter_mut())
            .chain(self.gen_high.iter_mut())
            .chain(std::iter::once(&mut self.gen_low))
    }
}

impl Fx for BassLift {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        for s in [
            &mut self.harmonics,
            &mut self.punch,
            &mut self.sub,
            &mut self.ceiling,
            &mut self.output,
        ] {
            s.set_ramp(sr, RAMP_SECONDS);
        }
        self.fast_coeff = one_pole_coeff(0.010, sr);
        self.slow_coeff = one_pole_coeff(0.100, sr);
        self.gain_coeff = one_pole_coeff(0.003, sr);
        for f in self.filters() {
            f.set_ramp_time(sr, 0.02);
        }
        refresh_all(self);
        for s in [
            &mut self.harmonics,
            &mut self.punch,
            &mut self.sub,
            &mut self.ceiling,
            &mut self.output,
        ] {
            let t = s.target();
            s.set_immediate(t);
        }
        for f in self.filters() {
            let t = f.target();
            f.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        let sr = self.sample_rate;
        match id {
            ID_FREQ => {
                let lp = Coeffs::design(
                    FilterKind::LowPass,
                    sr,
                    value,
                    std::f32::consts::FRAC_1_SQRT_2,
                    0.0,
                );
                let hp = Coeffs::design(
                    FilterKind::HighPass,
                    sr,
                    value,
                    std::f32::consts::FRAC_1_SQRT_2,
                    0.0,
                );
                for f in &mut self.low {
                    f.set_target(lp);
                }
                for f in &mut self.high {
                    f.set_target(hp);
                }
                // Harmonic band: starts above the crossover so the
                // fundamental stays out, spans about two octaves.
                let lo = (value * 1.5).clamp(100.0, 320.0);
                let hi = (lo * 3.0).max(400.0);
                self.gen_high[0].set_target(Coeffs::design(
                    FilterKind::HighPass,
                    sr,
                    lo,
                    0.541_196,
                    0.0,
                ));
                self.gen_high[1].set_target(Coeffs::design(
                    FilterKind::HighPass,
                    sr,
                    lo,
                    1.306_563,
                    0.0,
                ));
                self.gen_low.set_target(Coeffs::design(
                    FilterKind::LowPass,
                    sr,
                    hi,
                    std::f32::consts::FRAC_1_SQRT_2,
                    0.0,
                ));
            }
            ID_HARMONICS => self.harmonics.set_target(value * HARMONIC_GAIN),
            ID_PUNCH => self.punch.set_target(value),
            ID_SUB => self.sub.set_target(db_to_gain(value)),
            ID_CEILING => self.ceiling.set_target(db_to_gain(value)),
            ID_OUTPUT => self.output.set_target(db_to_gain(value)),
            _ => {}
        }
    }

    fn clear(&mut self) {
        for f in self.filters() {
            f.reset();
        }
        self.fast_power = 0.0;
        self.slow_power = 0.0;
        self.punch_gain = 1.0;
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let harmonics = self.harmonics.next();
            let punch = self.punch.next();
            let sub = self.sub.next();
            let ceiling = self.ceiling.next();
            let out = self.output.next();

            let (in_l, in_r) = (*l, *r);
            let (mut lo_l, mut lo_r) = (in_l, in_r);
            let (mut hi_l, mut hi_r) = (in_l, in_r);
            for i in 0..2 {
                (lo_l, lo_r) = self.low[i].process(lo_l, lo_r);
                (hi_l, hi_r) = self.high[i].process(hi_l, hi_r);
            }

            // Punch: onset lift / sustain tightening from a fast-versus-slow
            // power comparison, linked across channels.
            let power = (lo_l * lo_l).max(lo_r * lo_r);
            self.fast_power += self.fast_coeff * (power - self.fast_power);
            self.slow_power += self.slow_coeff * (power - self.slow_power);
            let target = if punch > 1e-3 {
                let ratio = ((self.fast_power + 1e-8) / (self.slow_power + 1e-8)).sqrt();
                ratio.clamp(0.35, 2.8).powf(punch * 0.7)
            } else {
                1.0
            };
            self.punch_gain += self.gain_coeff * (target - self.punch_gain);
            let (lo_l, lo_r) = (lo_l * self.punch_gain, lo_r * self.punch_gain);

            // Harmonics: saturated plus rectified low band, band-passed.
            let harmonic = |s: f32| 0.5 * (2.5 * s).tanh() + 0.8 * s.abs();
            let (mut h_l, mut h_r) = (harmonic(lo_l), harmonic(lo_r));
            for f in &mut self.gen_high {
                (h_l, h_r) = f.process(h_l, h_r);
            }
            let (h_l, h_r) = self.gen_low.process(h_l, h_r);

            let bass_l = knee_clip(lo_l * sub + h_l * harmonics, ceiling);
            let bass_r = knee_clip(lo_r * sub + h_r * harmonics, ceiling);
            *l = safety_clip(sanitize((bass_l + hi_l) * out));
            *r = safety_clip(sanitize((bass_r + hi_r) * out));
        }
    }

    fn tail(&self) -> Option<f32> {
        Some(0.5)
    }

    fn silent(&self) -> bool {
        self.silence.quiet_for((0.5 * self.sample_rate) as u32)
    }
}

impl_device!(BassLift, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;
    use peel_dsp::device::Device;

    fn run_sine(d: &mut dyn Device, freq: f64, amp: f32, frames: usize) -> Vec<f32> {
        let ctx = ProcessContext::new(SR);
        let mut out = Vec::with_capacity(frames);
        for block in 0..frames / 256 {
            let mut l: Vec<f32> = (0..256).map(|i| sine(freq, block * 256 + i, amp)).collect();
            let mut r = l.clone();
            d.process(&ctx, &[], &mut l, &mut r);
            out.extend_from_slice(&l);
        }
        out
    }

    fn set(d: &mut dyn Device, key: &str, v: f32) {
        let id = d.descriptor().param_by_key(key).unwrap().id;
        d.set_param(id, v);
    }

    #[test]
    fn every_preset_is_finite() {
        presets_are_finite("peel.fx.bass_lift");
    }

    #[test]
    fn never_exceeds_full_scale_at_max_settings() {
        for freq in [40.0f64, 50.0, 80.0, 120.0] {
            let mut d = BassLift::new();
            for p in &PARAMS {
                d.set_param(p.id, p.max);
            }
            d.prepare(SR, 256);
            let out = run_sine(&mut d, freq, 1.0, 48_000);
            let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak <= 1.0, "{freq} Hz peaked at {peak}");
            assert!(peak > 0.5, "{freq} Hz went silent: {peak}");
        }
    }

    #[test]
    fn adds_energy_between_100_and_400_hz() {
        let band = |x: &[f32]| -> f64 {
            [100.0, 150.0, 200.0, 250.0, 300.0, 350.0]
                .iter()
                .map(|f| tone_level(x, *f).powi(2))
                .sum()
        };
        let mut off = BassLift::new();
        set(&mut off, "harmonics", 0.0);
        off.prepare(SR, 256);
        let mut on = BassLift::new();
        set(&mut on, "harmonics", 1.0);
        on.prepare(SR, 256);
        let a = run_sine(&mut off, 50.0, 0.5, 96_000);
        let b = run_sine(&mut on, 50.0, 0.5, 96_000);
        let (a, b) = (&a[48_000..], &b[48_000..]);
        assert!(
            band(b) > 20.0 * band(a) + 1e-6,
            "{} vs {}",
            band(b),
            band(a)
        );
        // The fundamental survives.
        assert!(tone_level(b, 50.0) > 0.3);
    }

    #[test]
    fn sub_gain_scales_the_low_band() {
        let level = |sub: f32| {
            let mut d = BassLift::new();
            set(&mut d, "harmonics", 0.0);
            set(&mut d, "punch", 0.0);
            set(&mut d, "ceiling", 0.0);
            set(&mut d, "sub", sub);
            d.prepare(SR, 256);
            let out = run_sine(&mut d, 50.0, 0.1, 48_000);
            tone_level(&out[24_000..], 50.0)
        };
        let ratio = level(6.0) / level(0.0);
        assert!((ratio - 2.0).abs() < 0.1, "{ratio}");
    }

    #[test]
    fn highs_pass_through_at_neutral_settings() {
        let mut d = BassLift::new();
        set(&mut d, "harmonics", 0.0);
        set(&mut d, "punch", 0.0);
        d.prepare(SR, 256);
        let out = run_sine(&mut d, 3_000.0, 0.5, 48_000);
        let level = tone_level(&out[24_000..], 3_000.0);
        assert!((level - 0.5).abs() < 0.01, "{level}");
    }

    #[test]
    fn automation_does_not_click() {
        automation_is_click_free("peel.fx.bass_lift", 0.25);
    }
}
