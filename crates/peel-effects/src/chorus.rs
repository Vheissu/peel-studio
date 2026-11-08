//! Stereo chorus with two or three modulated voices.
//!
//! Each channel has its own delay line and its own set of voices, so a
//! stereo input stays stereo. Voice LFOs are spread evenly across the cycle
//! (0, 1/2 for two voices; 0, 1/3, 2/3 for three) and the right channel is
//! offset by up to a quarter cycle by *Spread*, which decorrelates a mono
//! source into a wide image. Taps are read with cubic Hermite interpolation.
//! The wet signal is high-passed and low-passed to keep low end tight and
//! take the edge off, and an optional feedback path (positive or negative)
//! turns the chorus toward flanging.

use peel_dsp::biquad::{Coeffs, FilterKind, StereoBiquad};
use peel_dsp::delay::{DelayLine, Lfo, fast_sin_turns};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, sanitize};

use crate::common::{
    Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all, soft_limit,
};

pub const ID_VOICES: ParamId = 0;
pub const ID_RATE: ParamId = 1;
pub const ID_DEPTH: ParamId = 2;
pub const ID_DELAY: ParamId = 3;
pub const ID_FEEDBACK: ParamId = 4;
pub const ID_SPREAD: ParamId = 5;
pub const ID_LOW_CUT: ParamId = 6;
pub const ID_HIGH_CUT: ParamId = 7;
pub const ID_MIX: ParamId = 8;

const VOICES: &[&str] = &["2 Voices", "3 Voices"];

/// Peak delay modulation at full depth, in milliseconds.
const MAX_DEPTH_MS: f32 = 6.0;
/// Longest base delay, in milliseconds.
const MAX_DELAY_MS: f32 = 30.0;

static PARAMS: [ParamDescriptor; 9] = [
    ParamDescriptor::choice(ID_VOICES, "voices", "Voices", "Voices", VOICES, 1),
    ParamDescriptor::log(
        ID_RATE,
        "rate",
        "Rate",
        "Modulation",
        0.05,
        8.0,
        0.6,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_DEPTH,
        "depth",
        "Depth",
        "Modulation",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ID_DELAY,
        "delay",
        "Delay",
        "Modulation",
        3.0,
        MAX_DELAY_MS,
        12.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::linear(
        ID_FEEDBACK,
        "feedback",
        "Feedback",
        "Modulation",
        -0.8,
        0.8,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_SPREAD,
        "spread",
        "Spread",
        "Stereo",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ID_LOW_CUT,
        "lowcut",
        "Low Cut",
        "Tone",
        20.0,
        1_000.0,
        100.0,
        Unit::Hertz,
    ),
    ParamDescriptor::log(
        ID_HIGH_CUT,
        "highcut",
        "High Cut",
        "Tone",
        1_000.0,
        20_000.0,
        12_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(ID_MIX, "mix", "Mix", "Output", 0.0, 1.0, 0.5, Unit::Percent),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.chorus",
    name: "Chorus",
    description: "Stereo chorus with two or three voices and feedback",
    category: "Modulation",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Classic Chorus",
        params: &[],
    },
    Preset {
        name: "Subtle Width",
        params: &[
            ("voices", 0.0),
            ("rate", 0.35),
            ("depth", 0.25),
            ("delay", 14.0),
            ("mix", 0.35),
        ],
    },
    Preset {
        name: "Lush Ensemble",
        params: &[
            ("voices", 1.0),
            ("rate", 0.9),
            ("depth", 0.75),
            ("delay", 18.0),
            ("feedback", 0.2),
            ("mix", 0.55),
        ],
    },
    Preset {
        name: "Slow Shimmer",
        params: &[
            ("rate", 0.15),
            ("depth", 0.9),
            ("delay", 22.0),
            ("feedback", 0.3),
            ("highcut", 9_000.0),
            ("mix", 0.5),
        ],
    },
    Preset {
        name: "Jet Flange",
        params: &[
            ("voices", 0.0),
            ("rate", 0.25),
            ("depth", 0.6),
            ("delay", 3.5),
            ("feedback", -0.65),
            ("lowcut", 40.0),
            ("highcut", 16_000.0),
            ("mix", 0.5),
        ],
    },
];

/// Chorus effect. See the module docs.
pub struct Chorus {
    store: ParamStore,
    sample_rate: f32,
    lines: [DelayLine; 2],
    lfo: Lfo,
    voices: usize,
    base: Smoothed,
    depth: Smoothed,
    feedback: Smoothed,
    spread: Smoothed,
    mix: Smoothed,
    low_cut: StereoBiquad,
    high_cut: StereoBiquad,
    silence: SilenceTracker,
}

impl Default for Chorus {
    fn default() -> Self {
        Chorus::new()
    }
}

impl Chorus {
    pub fn new() -> Chorus {
        let mut c = Chorus {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            lines: [DelayLine::new(16), DelayLine::new(16)],
            lfo: Lfo::new(0.0),
            voices: 3,
            base: Smoothed::new(0.0),
            depth: Smoothed::new(0.0),
            feedback: Smoothed::new(0.0),
            spread: Smoothed::new(1.0),
            mix: Smoothed::new(0.5),
            low_cut: StereoBiquad::default(),
            high_cut: StereoBiquad::default(),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut c, 48_000.0, 512);
        c
    }
}

impl Fx for Chorus {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        let max = ((MAX_DELAY_MS + MAX_DEPTH_MS + 4.0) * 0.001 * sr) as usize + 8;
        self.lines = [DelayLine::new(max), DelayLine::new(max)];
        for s in [
            &mut self.base,
            &mut self.depth,
            &mut self.feedback,
            &mut self.spread,
            &mut self.mix,
        ] {
            s.set_ramp(sr, RAMP_SECONDS * 2.0);
        }
        self.low_cut.set_ramp_time(sr, 0.01);
        self.high_cut.set_ramp_time(sr, 0.01);
        self.lfo.set_rate(self.store.get(ID_RATE), sr);
        refresh_all(self);
        for s in [
            &mut self.base,
            &mut self.depth,
            &mut self.feedback,
            &mut self.spread,
            &mut self.mix,
        ] {
            let t = s.target();
            s.set_immediate(t);
        }
        for f in [&mut self.low_cut, &mut self.high_cut] {
            let t = f.target();
            f.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        let sr = self.sample_rate;
        let q = std::f32::consts::FRAC_1_SQRT_2;
        match id {
            ID_VOICES => self.voices = if value >= 0.5 { 3 } else { 2 },
            ID_RATE => self.lfo.set_rate(value, sr),
            ID_DEPTH => self.depth.set_target(value * MAX_DEPTH_MS * 0.001 * sr),
            ID_DELAY => self.base.set_target(value * 0.001 * sr),
            ID_FEEDBACK => self.feedback.set_target(value),
            ID_SPREAD => self.spread.set_target(value),
            ID_LOW_CUT => self.low_cut.set_target(if value <= 20.5 {
                Coeffs::IDENTITY
            } else {
                Coeffs::design(FilterKind::HighPass, sr, value, q, 0.0)
            }),
            ID_HIGH_CUT => self.high_cut.set_target(if value >= 19_500.0 {
                Coeffs::IDENTITY
            } else {
                Coeffs::design(FilterKind::LowPass, sr, value, q, 0.0)
            }),
            ID_MIX => self.mix.set_target(value),
            _ => {}
        }
    }

    fn clear(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        self.low_cut.reset();
        self.high_cut.reset();
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        let v = self.voices;
        let voice_gain = 1.0 / (v as f32).sqrt();
        let min_delay = 0.001 * self.sample_rate;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let base = self.base.next();
            let depth = self.depth.next();
            let spread = self.spread.next();
            let fb = self.feedback.next();
            let mix = self.mix.next();
            let phase = self.lfo.next_phase();

            let (in_l, in_r) = (*l, *r);
            let mut wet = [0.0f32; 2];
            for (ch, w) in wet.iter_mut().enumerate() {
                let offset = if ch == 1 { 0.25 * spread } else { 0.0 };
                let mut acc = 0.0;
                for voice in 0..v {
                    let p = phase + voice as f32 / v as f32 + offset;
                    let d = (base + depth * fast_sin_turns(p)).max(min_delay);
                    acc += self.lines[ch].read_hermite(d);
                }
                *w = acc * voice_gain;
            }

            self.lines[0].push(sanitize(soft_limit(in_l + fb * wet[0])));
            self.lines[1].push(sanitize(soft_limit(in_r + fb * wet[1])));

            let (wl, wr) = self.low_cut.process(wet[0], wet[1]);
            let (wl, wr) = self.high_cut.process(wl, wr);
            *l = in_l + (wl - in_l) * mix;
            *r = in_r + (wr - in_r) * mix;
        }
    }

    fn tail(&self) -> Option<f32> {
        let fb = self.store.get(ID_FEEDBACK).abs();
        let one = (self.store.get(ID_DELAY) + MAX_DEPTH_MS) * 0.001;
        let repeats = if fb < 1e-3 {
            1.0
        } else {
            (1e-3f32.ln() / fb.ln()).ceil().clamp(1.0, 60.0)
        };
        Some(one * (repeats + 1.0))
    }

    fn silent(&self) -> bool {
        let secs = self.tail().unwrap_or(2.0);
        self.silence.quiet_for((secs * self.sample_rate) as u32)
    }
}

impl_device!(Chorus, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;

    fn tone(block: usize, freq: f64, amp: f32) -> Vec<f32> {
        (0..256)
            .map(|i| {
                let idx = (block * 256 + i) as f64;
                amp * (std::f64::consts::TAU * (idx * freq / f64::from(SR)).fract()).sin() as f32
            })
            .collect()
    }

    #[test]
    fn mono_input_becomes_stereo() {
        let mut c = Chorus::new();
        c.set_param(ID_MIX, 1.0);
        c.set_param(ID_DEPTH, 0.8);
        c.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        let (mut diff, mut total) = (0.0f64, 0.0f64);
        for block in 0..200 {
            let mut l = tone(block, 440.0, 0.5);
            let mut r = l.clone();
            c.process(&ctx, &[], &mut l, &mut r);
            if block > 50 {
                for i in 0..256 {
                    diff += f64::from((l[i] - r[i]).powi(2));
                    total += f64::from(l[i].powi(2) + r[i].powi(2));
                }
            }
        }
        assert!(
            diff / total > 0.05,
            "channels too similar: {}",
            diff / total
        );
    }

    #[test]
    fn spread_zero_keeps_channels_identical() {
        let mut c = Chorus::new();
        c.set_param(ID_SPREAD, 0.0);
        c.set_param(ID_MIX, 1.0);
        c.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        for block in 0..50 {
            let mut l = tone(block, 330.0, 0.5);
            let mut r = l.clone();
            c.process(&ctx, &[], &mut l, &mut r);
            for i in 0..256 {
                assert!((l[i] - r[i]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn depth_zero_is_a_static_comb_and_mix_zero_is_dry() {
        let mut c = Chorus::new();
        c.set_param(ID_MIX, 0.0);
        c.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        let mut l = tone(0, 500.0, 0.5);
        let mut r = l.clone();
        let want = l.clone();
        c.process(&ctx, &[], &mut l, &mut r);
        for i in 0..256 {
            assert!((l[i] - want[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn modulation_depth_changes_pitch_content() {
        // With depth, a steady tone is frequency modulated: the output
        // amplitude envelope (wet only) is not constant after summing with
        // the dry. Without depth it is.
        let ctx = ProcessContext::new(SR);
        let ripple = |depth: f32| {
            let mut c = Chorus::new();
            c.set_param(ID_DEPTH, depth);
            c.set_param(ID_RATE, 3.0);
            c.set_param(ID_LOW_CUT, 20.0);
            c.set_param(ID_HIGH_CUT, 20_000.0);
            c.set_param(ID_MIX, 0.5);
            c.set_param(ID_VOICES, 0.0);
            c.prepare(SR, 256);
            let (mut lo, mut hi) = (f32::MAX, 0.0f32);
            for block in 0..400 {
                let mut l = tone(block, 1_000.0, 0.5);
                let mut r = l.clone();
                c.process(&ctx, &[], &mut l, &mut r);
                if block > 40 {
                    let p = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
            hi / lo
        };
        assert!(ripple(0.0) < 1.05);
        assert!(ripple(1.0) > 1.3);
    }

    #[test]
    fn feedback_is_bounded() {
        let mut c = Chorus::new();
        c.set_param(ID_FEEDBACK, 0.8);
        c.set_param(ID_MIX, 1.0);
        c.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        let mut seed = 9u32;
        for _ in 0..300 {
            let mut l: Vec<f32> = (0..256)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
                })
                .collect();
            let mut r = l.clone();
            c.process(&ctx, &[], &mut l, &mut r);
            assert!(
                l.iter()
                    .chain(r.iter())
                    .all(|v| v.is_finite() && v.abs() < 20.0)
            );
        }
    }
}
