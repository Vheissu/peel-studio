//! Vintage Chorus: a bucket-brigade style stereo ensemble.
//!
//! Three fixed modes (I, II and I+II) pick a triangle LFO rate, a sweep
//! depth and a short base delay, the way a classic chorus pedal offers a few
//! buttons instead of free controls. The left and right wet paths are
//! modulated in opposite phase, which is what gives the wide, swirling
//! image. The wet signal is darkened by a steep low-pass, like the clock
//! filters of a bucket-brigade delay, and can carry a little hiss. The hiss
//! follows the input level, so a silent input stays silent.

use peel_dsp::biquad::{Coeffs, FilterKind, StereoBiquad};
use peel_dsp::delay::{DelayLine, Lfo};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::noise::Noise;
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain, one_pole_coeff, sanitize};

use crate::common::{
    Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all, tri_turns,
};

pub const ID_MODE: ParamId = 0;
pub const ID_DEPTH: ParamId = 1;
pub const ID_TONE: ParamId = 2;
pub const ID_NOISE: ParamId = 3;
pub const ID_WIDTH: ParamId = 4;
pub const ID_MIX: ParamId = 5;
pub const ID_OUTPUT: ParamId = 6;

const MODES: &[&str] = &["I", "II", "I+II"];

/// Per mode: LFO rate (Hz), peak sweep (ms), base delay (ms).
const SHAPES: [(f32, f32, f32); 3] = [(0.5, 1.4, 3.6), (0.83, 2.0, 3.9), (9.5, 0.45, 3.4)];

/// Hiss level at `noise = 1` (linear, before the wet low-pass).
const HISS: f32 = 0.004;

static PARAMS: [ParamDescriptor; 7] = [
    ParamDescriptor::choice(ID_MODE, "mode", "Mode", "Mode", MODES, 0),
    ParamDescriptor::linear(
        ID_DEPTH,
        "depth",
        "Depth",
        "Mode",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ID_TONE,
        "tone",
        "Tone",
        "Character",
        1_500.0,
        16_000.0,
        6_500.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_NOISE,
        "noise",
        "Hiss",
        "Character",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_WIDTH,
        "width",
        "Width",
        "Stereo",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(ID_MIX, "mix", "Mix", "Output", 0.0, 1.0, 0.5, Unit::Percent),
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
    type_id: "peel.fx.vintage_chorus",
    name: "Vintage Chorus",
    description: "Bucket-brigade style stereo ensemble with fixed modes",
    category: "Modulation",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Mode I",
        params: &[],
    },
    Preset {
        name: "Mode II",
        params: &[("mode", 1.0)],
    },
    Preset {
        name: "Both",
        params: &[("mode", 2.0), ("mix", 0.55)],
    },
    Preset {
        name: "Wide Wash",
        params: &[
            ("mode", 1.0),
            ("tone", 4_500.0),
            ("noise", 0.25),
            ("mix", 0.65),
        ],
    },
    Preset {
        name: "Subtle",
        params: &[
            ("mode", 0.0),
            ("depth", 0.5),
            ("tone", 9_000.0),
            ("mix", 0.3),
        ],
    },
];

/// Vintage Chorus effect. See the module docs.
pub struct VintageChorus {
    store: ParamStore,
    sample_rate: f32,
    lines: [DelayLine; 2],
    lfo: Lfo,
    noise: [Noise; 2],
    mode: usize,
    intensity: f32,
    rate: Smoothed,
    base: Smoothed,
    sweep: Smoothed,
    hiss: Smoothed,
    width: Smoothed,
    mix: Smoothed,
    output: Smoothed,
    dark: [StereoBiquad; 2],
    env: f32,
    env_release: f32,
    silence: SilenceTracker,
}

impl Default for VintageChorus {
    fn default() -> Self {
        VintageChorus::new()
    }
}

impl VintageChorus {
    pub fn new() -> VintageChorus {
        let mut c = VintageChorus {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            lines: [DelayLine::new(16), DelayLine::new(16)],
            lfo: Lfo::new(0.0),
            noise: [Noise::new(1), Noise::new(2)],
            mode: 0,
            intensity: 1.0,
            rate: Smoothed::new(0.5),
            base: Smoothed::new(0.0),
            sweep: Smoothed::new(0.0),
            hiss: Smoothed::new(0.0),
            width: Smoothed::new(1.0),
            mix: Smoothed::new(0.5),
            output: Smoothed::new(1.0),
            dark: Default::default(),
            env: 0.0,
            env_release: 0.0,
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut c, 48_000.0, 512);
        c
    }

    fn smoothers(&mut self) -> [&mut Smoothed; 7] {
        [
            &mut self.rate,
            &mut self.base,
            &mut self.sweep,
            &mut self.hiss,
            &mut self.width,
            &mut self.mix,
            &mut self.output,
        ]
    }

    fn retarget_mode(&mut self) {
        let (rate, depth_ms, base_ms) = SHAPES[self.mode];
        let sr = self.sample_rate;
        self.rate.set_target(rate);
        self.base.set_target(base_ms * 0.001 * sr);
        self.sweep
            .set_target(depth_ms * self.intensity * 0.001 * sr);
    }
}

impl Fx for VintageChorus {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        let max = (12.0e-3 * sr) as usize + 8;
        self.lines = [DelayLine::new(max), DelayLine::new(max)];
        for s in self.smoothers() {
            s.set_ramp(sr, RAMP_SECONDS * 3.0);
        }
        self.env_release = one_pole_coeff(0.15, sr);
        for f in &mut self.dark {
            f.set_ramp_time(sr, 0.02);
        }
        refresh_all(self);
        for s in self.smoothers() {
            let t = s.target();
            s.set_immediate(t);
        }
        for f in &mut self.dark {
            let t = f.target();
            f.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        let sr = self.sample_rate;
        match id {
            ID_MODE => {
                self.mode = (value.round() as usize).min(SHAPES.len() - 1);
                self.retarget_mode();
            }
            ID_DEPTH => {
                self.intensity = value;
                self.retarget_mode();
            }
            ID_TONE => {
                // Two Butterworth sections: a steep, BBD-like roll-off.
                self.dark[0].set_target(Coeffs::design(
                    FilterKind::LowPass,
                    sr,
                    value,
                    0.541_196,
                    0.0,
                ));
                self.dark[1].set_target(Coeffs::design(
                    FilterKind::LowPass,
                    sr,
                    value,
                    1.306_563,
                    0.0,
                ));
            }
            ID_NOISE => self.hiss.set_target(value * HISS),
            ID_WIDTH => self.width.set_target(value),
            ID_MIX => self.mix.set_target(value),
            ID_OUTPUT => self.output.set_target(db_to_gain(value)),
            _ => {}
        }
    }

    fn clear(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        for f in &mut self.dark {
            f.reset();
        }
        self.env = 0.0;
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        let sr = self.sample_rate;
        let min_delay = 2.0;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let rate = self.rate.next();
            let base = self.base.next();
            let sweep = self.sweep.next();
            let hiss = self.hiss.next();
            let width = self.width.next();
            let mix = self.mix.next();
            let out = self.output.next();

            self.lfo.set_rate(rate, sr);
            let tri = tri_turns(self.lfo.next_phase());
            let (in_l, in_r) = (*l, *r);

            // Opposite-phase modulation of the two wet paths.
            let mut wl = self.lines[0].read_hermite((base + sweep * tri).max(min_delay));
            let mut wr = self.lines[1].read_hermite((base - sweep * tri).max(min_delay));
            self.lines[0].push(sanitize(in_l));
            self.lines[1].push(sanitize(in_r));

            let mid = 0.5 * (wl + wr);
            wl = mid + (wl - mid) * width;
            wr = mid + (wr - mid) * width;

            // Hiss only while there is signal to ride on.
            self.env = in_l
                .abs()
                .max(in_r.abs())
                .max(self.env * (1.0 - self.env_release));
            let gate = ((self.env - 2e-4) / 2e-3).clamp(0.0, 1.0);
            let level = hiss * gate;
            wl += level * self.noise[0].white();
            wr += level * self.noise[1].white();

            for f in &mut self.dark {
                (wl, wr) = f.process(wl, wr);
            }
            *l = (in_l + (wl - in_l) * mix) * out;
            *r = (in_r + (wr - in_r) * mix) * out;
        }
    }

    fn tail(&self) -> Option<f32> {
        if self.store.get(ID_NOISE) > 1e-3 {
            Some(1.6)
        } else {
            Some(0.06)
        }
    }

    fn silent(&self) -> bool {
        let secs = self.tail().unwrap_or(2.0);
        self.silence.quiet_for((secs * self.sample_rate) as u32)
    }
}

impl_device!(VintageChorus, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;
    use peel_dsp::device::Device;

    fn run(d: &mut dyn Device, blocks: usize, f: impl Fn(usize) -> f32) -> (Vec<f32>, Vec<f32>) {
        let ctx = ProcessContext::new(SR);
        let (mut ol, mut or) = (Vec::new(), Vec::new());
        for b in 0..blocks {
            let mut l: Vec<f32> = (0..256).map(|i| f(b * 256 + i)).collect();
            let mut r = l.clone();
            d.process(&ctx, &[], &mut l, &mut r);
            ol.extend(l);
            or.extend(r);
        }
        (ol, or)
    }

    #[test]
    fn every_preset_is_finite() {
        presets_are_finite("peel.fx.vintage_chorus");
    }

    #[test]
    fn mix_zero_passes_audio_unchanged() {
        let mut d = VintageChorus::new();
        d.set_param(ID_MIX, 0.0);
        d.set_param(ID_NOISE, 1.0);
        d.prepare(SR, 256);
        let src = |i| sine(440.0, i, 0.5);
        let (l, _) = run(&mut d, 40, src);
        for (i, v) in l.iter().enumerate() {
            assert!((v - src(i)).abs() < 1e-6);
        }
    }

    #[test]
    fn channels_modulate_in_opposite_phase() {
        let mut d = VintageChorus::new();
        d.set_param(ID_MIX, 1.0);
        d.set_param(ID_MODE, 1.0);
        d.set_param(ID_TONE, 16_000.0);
        d.prepare(SR, 256);
        let (l, r) = run(&mut d, 400, |i| sine(1_000.0, i, 0.5));
        let (mut diff, mut total) = (0.0f64, 0.0f64);
        for i in 20_000..l.len() {
            diff += f64::from((l[i] - r[i]).powi(2));
            total += f64::from(l[i].powi(2) + r[i].powi(2));
        }
        assert!(diff / total > 0.05, "{}", diff / total);

        // Width zero collapses the wet signal to mono.
        d.set_param(ID_WIDTH, 0.0);
        let (l, r) = run(&mut d, 100, |i| sine(1_000.0, i, 0.5));
        for i in 10_000..l.len() {
            assert!((l[i] - r[i]).abs() < 1e-5);
        }
    }

    #[test]
    fn tone_darkens_the_wet_path() {
        let level = |tone: f32| {
            let mut d = VintageChorus::new();
            d.set_param(ID_MIX, 1.0);
            d.set_param(ID_TONE, tone);
            d.prepare(SR, 256);
            let (l, _) = run(&mut d, 200, |i| sine(8_000.0, i, 0.5));
            l[20_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        assert!(level(2_000.0) < 0.1 * level(16_000.0));
    }

    #[test]
    fn hiss_needs_a_signal_and_adds_noise() {
        let tone = |i: usize| sine(100.0, i, 0.5);
        let make = |hiss: f32| {
            let mut d = VintageChorus::new();
            d.set_param(ID_NOISE, hiss);
            d.set_param(ID_TONE, 16_000.0);
            d.prepare(SR, 256);
            d
        };
        // Silent input stays silent however much hiss is dialled in.
        let mut d = make(1.0);
        let (l, _) = run(&mut d, 40, |_| 0.0);
        assert!(l.iter().all(|v| v.abs() < 1e-7));
        // With a signal the output differs from the hiss-free one.
        let mut hissy = make(1.0);
        let mut clean = make(0.0);
        let (a, _) = run(&mut hissy, 40, tone);
        let (b, _) = run(&mut clean, 40, tone);
        let diff: f32 = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum();
        assert!(diff > 0.01, "{diff}");
    }

    #[test]
    fn automation_does_not_click() {
        automation_is_click_free("peel.fx.vintage_chorus", 0.25);
    }
}
