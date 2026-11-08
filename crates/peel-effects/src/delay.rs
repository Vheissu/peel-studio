//! Stereo / ping-pong / mono delay with tempo sync, filtered feedback and
//! tape-style time glide.
//!
//! * **Time**: free in milliseconds, or synced to the host tempo with the
//!   note value snapped to a musical division (1/32 up to two bars, straight,
//!   triplet and dotted). Left and right are independent.
//! * **Time changes** glide. The read position slews toward its target with a
//!   rate cap, so the repeats bend in pitch (like a tape delay) rather than
//!   clicking or jumping. Large jumps are bounded to roughly a 0.6x to 1.4x
//!   read speed. Cubic Hermite interpolation keeps the glide clean.
//! * **Feedback** is below 100%, filtered by a high-pass and a low-pass that
//!   sit in the loop (so every repeat is a little thinner and darker), and
//!   soft-limited above 0 dBFS so it cannot run away.
//! * **Ping-pong** feeds the input into the left line only and bounces each
//!   repeat to the other side: with equal times the echoes arrive left at
//!   `t`, right at `2t`, left at `3t`, ...
//! * **Mix** is an equal-power crossfade.

use peel_dsp::biquad::{Coeffs, FilterKind, StereoBiquad};
use peel_dsp::delay::{DelayLine, Lfo, TimeGlide};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, sanitize};

use crate::common::{
    Fx, ParamStore, RAMP_SECONDS, SilenceTracker, equal_power, impl_device, refresh_all,
    snap_beats, soft_limit,
};

pub const ID_SYNC: ParamId = 0;
pub const ID_TIME_L: ParamId = 1;
pub const ID_TIME_R: ParamId = 2;
pub const ID_BEATS_L: ParamId = 3;
pub const ID_BEATS_R: ParamId = 4;
pub const ID_MODE: ParamId = 5;
pub const ID_FEEDBACK: ParamId = 6;
pub const ID_FB_HP: ParamId = 7;
pub const ID_FB_LP: ParamId = 8;
pub const ID_MIX: ParamId = 9;
pub const ID_MOD_RATE: ParamId = 10;
pub const ID_MOD_DEPTH: ParamId = 11;

pub const MODE_STEREO: u32 = 0;
pub const MODE_PING_PONG: u32 = 1;
pub const MODE_MONO: u32 = 2;

const MODES: &[&str] = &["Stereo", "Ping-Pong", "Mono"];

/// Longest delay in seconds. Synced times beyond it are clamped.
const MAX_SECONDS: f32 = 4.0;
/// Peak modulation excursion at full depth, in seconds.
const MOD_SECONDS: f32 = 0.003;

static PARAMS: [ParamDescriptor; 12] = [
    ParamDescriptor::choice(ID_MODE, "mode", "Mode", "Time", MODES, 0),
    ParamDescriptor::toggle(ID_SYNC, "sync", "Sync", "Time", true),
    ParamDescriptor::log(
        ID_TIME_L,
        "time.left",
        "Left",
        "Time",
        1.0,
        2_000.0,
        375.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::log(
        ID_TIME_R,
        "time.right",
        "Right",
        "Time",
        1.0,
        2_000.0,
        375.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::log(
        ID_BEATS_L,
        "beats.left",
        "Left",
        "Time",
        0.125,
        8.0,
        0.5,
        Unit::Beats,
    ),
    ParamDescriptor::log(
        ID_BEATS_R,
        "beats.right",
        "Right",
        "Time",
        0.125,
        8.0,
        0.5,
        Unit::Beats,
    ),
    ParamDescriptor::linear(
        ID_FEEDBACK,
        "feedback",
        "Amount",
        "Feedback",
        0.0,
        0.95,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ID_FB_HP,
        "feedback.hp",
        "High-pass",
        "Feedback",
        20.0,
        2_000.0,
        80.0,
        Unit::Hertz,
    ),
    ParamDescriptor::log(
        ID_FB_LP,
        "feedback.lp",
        "Low-pass",
        "Feedback",
        500.0,
        20_000.0,
        8_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::log(
        ID_MOD_RATE,
        "mod.rate",
        "Rate",
        "Modulation",
        0.05,
        5.0,
        0.5,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_MOD_DEPTH,
        "mod.depth",
        "Depth",
        "Modulation",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(ID_MIX, "mix", "Mix", "Output", 0.0, 1.0, 0.3, Unit::Percent),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.delay",
    name: "Delay",
    description: "Stereo and ping-pong delay with tempo sync and filtered feedback",
    category: "Delay",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Eighth Note",
        params: &[],
    },
    Preset {
        name: "Dotted Ping-Pong",
        params: &[
            ("mode", 1.0),
            ("beats.left", 0.75),
            ("beats.right", 0.75),
            ("feedback", 0.5),
            ("feedback.hp", 150.0),
            ("feedback.lp", 6_000.0),
            ("mix", 0.28),
        ],
    },
    Preset {
        name: "Slapback",
        params: &[
            ("sync", 0.0),
            ("time.left", 95.0),
            ("time.right", 110.0),
            ("feedback", 0.1),
            ("feedback.lp", 5_000.0),
            ("mix", 0.22),
        ],
    },
    Preset {
        name: "Tape Echo",
        params: &[
            ("sync", 0.0),
            ("time.left", 340.0),
            ("time.right", 340.0),
            ("feedback", 0.55),
            ("feedback.hp", 200.0),
            ("feedback.lp", 3_200.0),
            ("mod.depth", 0.35),
            ("mod.rate", 0.35),
            ("mix", 0.3),
        ],
    },
    Preset {
        name: "Wide Quarter",
        params: &[
            ("beats.left", 1.0),
            ("beats.right", 0.75),
            ("feedback", 0.35),
            ("mix", 0.25),
        ],
    },
    Preset {
        name: "Long Ambient",
        params: &[
            ("mode", 1.0),
            ("beats.left", 2.0),
            ("beats.right", 1.5),
            ("feedback", 0.75),
            ("feedback.hp", 300.0),
            ("feedback.lp", 4_000.0),
            ("mod.depth", 0.2),
            ("mix", 0.35),
        ],
    },
];

/// Delay effect. See the module docs.
pub struct Delay {
    store: ParamStore,
    sample_rate: f32,
    lines: [DelayLine; 2],
    glide: [TimeGlide; 2],
    hp: StereoBiquad,
    lp: StereoBiquad,
    feedback: Smoothed,
    dry: Smoothed,
    wet: Smoothed,
    lfo: [Lfo; 2],
    mod_samples: f32,
    max_delay: f32,
    fresh: bool,
    spb: f32,
    silence: SilenceTracker,
}

impl Default for Delay {
    fn default() -> Self {
        Delay::new()
    }
}

impl Delay {
    pub fn new() -> Delay {
        let mut d = Delay {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            lines: [DelayLine::new(16), DelayLine::new(16)],
            glide: [TimeGlide::new(0.0); 2],
            hp: StereoBiquad::default(),
            lp: StereoBiquad::default(),
            feedback: Smoothed::new(0.0),
            dry: Smoothed::new(1.0),
            wet: Smoothed::new(0.0),
            // Quarter-cycle offset between channels for a gentle stereo wobble.
            lfo: [Lfo::new(0.0), Lfo::new(0.25)],
            mod_samples: 0.0,
            max_delay: 0.0,
            fresh: true,
            spb: 24_000.0,
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut d, 48_000.0, 512);
        d
    }

    /// Target delay in samples for a channel at the given tempo.
    fn target_samples(&self, channel: usize, spb: f32) -> f32 {
        let s = &self.store;
        let samples = if s.get(ID_SYNC) >= 0.5 {
            let id = if channel == 0 { ID_BEATS_L } else { ID_BEATS_R };
            snap_beats(s.get(id)) * spb
        } else {
            let id = if channel == 0 { ID_TIME_L } else { ID_TIME_R };
            s.get(id) * 0.001 * self.sample_rate
        };
        let margin = self.mod_samples + 4.0;
        samples.clamp(2.0 + self.mod_samples, (self.max_delay - margin).max(8.0))
    }

    fn retarget(&mut self, spb: f32) {
        self.spb = spb;
        let mode = self.store.get(ID_MODE) as u32;
        let left = self.target_samples(0, spb);
        let right = if mode == MODE_MONO {
            left
        } else {
            self.target_samples(1, spb)
        };
        self.glide[0].set_target(left);
        self.glide[1].set_target(right);
        if self.fresh {
            self.glide[0].set_immediate(left);
            self.glide[1].set_immediate(right);
            self.fresh = false;
        }
    }

    fn update_filters(&mut self) {
        let sr = self.sample_rate;
        let hp = self.store.get(ID_FB_HP);
        let lp = self.store.get(ID_FB_LP);
        let q = std::f32::consts::FRAC_1_SQRT_2;
        self.hp.set_target(if hp <= 20.5 {
            Coeffs::IDENTITY
        } else {
            Coeffs::design(FilterKind::HighPass, sr, hp, q, 0.0)
        });
        self.lp.set_target(if lp >= 19_500.0 {
            Coeffs::IDENTITY
        } else {
            Coeffs::design(FilterKind::LowPass, sr, lp, q, 0.0)
        });
    }

    fn update_mix(&mut self) {
        let (d, w) = equal_power(self.store.get(ID_MIX));
        self.dry.set_target(d);
        self.wet.set_target(w);
    }

    fn tail_samples(&self) -> f32 {
        let fb = self.store.get(ID_FEEDBACK);
        let t = self.glide[0].target().max(self.glide[1].target()) + self.mod_samples;
        let repeats = if fb < 1e-3 {
            1.0
        } else {
            (1e-3f32.ln() / fb.ln()).ceil().clamp(1.0, 400.0)
        };
        // Ping-pong passes through both lines per cycle but each pass is one
        // feedback factor, so the same count applies.
        t * (repeats + 1.0)
    }
}

impl Fx for Delay {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        let max = (MAX_SECONDS * sr) as usize + 8;
        self.lines = [DelayLine::new(max), DelayLine::new(max)];
        self.max_delay = max as f32;
        for g in &mut self.glide {
            g.configure(sr, 0.12, 0.4);
        }
        for l in &mut self.lfo {
            l.set_rate(self.store.get(ID_MOD_RATE), sr);
        }
        self.hp.set_ramp_time(sr, 0.01);
        self.lp.set_ramp_time(sr, 0.01);
        for s in [&mut self.feedback, &mut self.dry, &mut self.wet] {
            s.set_ramp(sr, RAMP_SECONDS);
        }
        refresh_all(self);
        for f in [&mut self.hp, &mut self.lp] {
            let t = f.target();
            f.set_immediate(t);
        }
        for s in [&mut self.feedback, &mut self.dry, &mut self.wet] {
            let t = s.target();
            s.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_FEEDBACK => self.feedback.set_target(value),
            ID_FB_HP | ID_FB_LP => self.update_filters(),
            ID_MIX => self.update_mix(),
            ID_MOD_RATE => {
                for l in &mut self.lfo {
                    l.set_rate(value, self.sample_rate);
                }
            }
            ID_MOD_DEPTH => {
                self.mod_samples = value * MOD_SECONDS * self.sample_rate;
                let spb = self.spb;
                self.retarget(spb);
            }
            ID_SYNC | ID_TIME_L | ID_TIME_R | ID_BEATS_L | ID_BEATS_R | ID_MODE => {
                let spb = self.spb;
                self.retarget(spb);
            }
            _ => {}
        }
    }

    fn clear(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        self.hp.reset();
        self.lp.reset();
        self.fresh = true;
        self.silence.reset();
    }

    fn render(&mut self, ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        self.retarget(ctx.samples_per_beat());
        let mode = self.store.get(ID_MODE) as u32;
        let mod_on = self.mod_samples > 0.0;

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (in_l, in_r) = (*l, *r);
            let (m_l, m_r) = if mod_on {
                (
                    self.lfo[0].next_sine() * self.mod_samples,
                    self.lfo[1].next_sine() * self.mod_samples,
                )
            } else {
                (0.0, 0.0)
            };
            let t_l = self.glide[0].next();
            let t_r = self.glide[1].next();
            let d_l = self.lines[0].read_hermite(t_l + m_l);
            let d_r = if mode == MODE_MONO {
                d_l
            } else {
                self.lines[1].read_hermite(t_r + m_r)
            };

            let fb = self.feedback.next();
            let (src_l, src_r) = match mode {
                MODE_PING_PONG => (d_r, d_l),
                _ => (d_l, d_r),
            };
            let (inj_l, inj_r) = match mode {
                MODE_STEREO => (in_l, in_r),
                MODE_PING_PONG => ((in_l + in_r) * 0.5, 0.0),
                _ => ((in_l + in_r) * 0.5, 0.0),
            };
            let (w_l, w_r) = (inj_l + fb * src_l, inj_r + fb * src_r);
            let (w_l, w_r) = self.hp.process(w_l, w_r);
            let (w_l, w_r) = self.lp.process(w_l, w_r);
            self.lines[0].push(sanitize(soft_limit(w_l)));
            self.lines[1].push(sanitize(soft_limit(w_r)));

            let dry = self.dry.next();
            let wet = self.wet.next();
            *l = in_l * dry + d_l * wet;
            *r = in_r * dry + d_r * wet;
        }
    }

    fn tail(&self) -> Option<f32> {
        Some((self.tail_samples() / self.sample_rate).min(60.0))
    }

    fn silent(&self) -> bool {
        let n = (self.tail_samples()).min(60.0 * self.sample_rate);
        self.silence.quiet_for(n as u32)
    }
}

impl_device!(Delay, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;

    fn run_impulse(d: &mut Delay, n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        l[0] = 1.0;
        r[0] = 1.0;
        let ctx = ProcessContext::new(SR);
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            d.process(&ctx, &[], cl, cr);
        }
        (l, r)
    }

    fn free_delay(ms_l: f32, ms_r: f32, fb: f32, mode: u32) -> Delay {
        let mut d = Delay::new();
        d.set_param(ID_SYNC, 0.0);
        d.set_param(ID_TIME_L, ms_l);
        d.set_param(ID_TIME_R, ms_r);
        d.set_param(ID_FEEDBACK, fb);
        d.set_param(ID_FB_HP, 20.0);
        d.set_param(ID_FB_LP, 20_000.0);
        d.set_param(ID_MIX, 1.0);
        d.set_param(ID_MODE, mode as f32);
        d.prepare(SR, 256);
        d
    }

    #[test]
    fn echo_arrives_at_the_exact_sample() {
        let mut d = free_delay(100.0, 100.0, 0.0, MODE_STEREO);
        let (l, r) = run_impulse(&mut d, 9_000);
        let at = (0.1 * SR) as usize;
        assert!((l[at] - 1.0).abs() < 1e-4, "{}", l[at]);
        assert!((r[at] - 1.0).abs() < 1e-4);
        for (i, v) in l.iter().enumerate() {
            if i != at {
                assert!(v.abs() < 1e-4, "stray energy at {i}: {v}");
            }
        }
    }

    #[test]
    fn feedback_repeats_decay_geometrically() {
        let mut d = free_delay(50.0, 50.0, 0.5, MODE_STEREO);
        let (l, _) = run_impulse(&mut d, 20_000);
        let t = (0.05 * SR) as usize;
        assert!((l[t] - 1.0).abs() < 1e-4);
        assert!((l[2 * t] - 0.5).abs() < 1e-4);
        assert!((l[3 * t] - 0.25).abs() < 1e-4);
    }

    #[test]
    fn ping_pong_alternates_sides() {
        let mut d = free_delay(60.0, 60.0, 0.6, MODE_PING_PONG);
        let (l, r) = run_impulse(&mut d, 30_000);
        let t = (0.06 * SR) as usize;
        // Input enters the left line: first echo left, then right, then left.
        assert!(l[t].abs() > 0.4 && r[t].abs() < 1e-4);
        assert!(r[2 * t].abs() > 0.2 && l[2 * t].abs() < 1e-4);
        assert!(l[3 * t].abs() > 0.1 && r[3 * t].abs() < 1e-4);
    }

    #[test]
    fn tempo_sync_uses_samples_per_beat() {
        let mut d = Delay::new();
        d.set_param(ID_SYNC, 1.0);
        d.set_param(ID_BEATS_L, 1.0);
        d.set_param(ID_BEATS_R, 1.0);
        d.set_param(ID_FEEDBACK, 0.0);
        d.set_param(ID_FB_HP, 20.0);
        d.set_param(ID_FB_LP, 20_000.0);
        d.set_param(ID_MIX, 1.0);
        d.prepare(SR, 256);
        let mut ctx = ProcessContext::new(SR);
        ctx.tempo_bpm = 150.0;
        let spb = (SR * 60.0 / 150.0) as usize; // 19200
        let mut l = vec![0.0; 256];
        let mut r = vec![0.0; 256];
        l[0] = 1.0;
        let mut found = None;
        d.process(&ctx, &[], &mut l, &mut r);
        for block in 1..=(spb / 256 + 2) {
            let mut l = vec![0.0; 256];
            let mut r = vec![0.0; 256];
            d.process(&ctx, &[], &mut l, &mut r);
            for (i, v) in l.iter().enumerate() {
                if v.abs() > 0.5 {
                    found = Some(block * 256 + i);
                }
            }
        }
        assert_eq!(found, Some(spb));
    }

    #[test]
    fn time_change_is_smooth() {
        let mut d = free_delay(80.0, 80.0, 0.3, MODE_STEREO);
        d.set_param(ID_MIX, 0.5);
        d.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for block in 0..300 {
            let mut l: Vec<f32> = (0..256)
                .map(|i| {
                    0.5 * (std::f32::consts::TAU * 220.0 * (block * 256 + i) as f32 / SR).sin()
                })
                .collect();
            let mut r = l.clone();
            if block == 100 {
                d.set_param(ID_TIME_L, 700.0);
                d.set_param(ID_TIME_R, 40.0);
            }
            d.process(&ctx, &[], &mut l, &mut r);
            for v in l.iter().chain(r.iter()) {
                assert!(v.is_finite() && v.abs() < 4.0);
            }
            for v in l {
                worst = worst.max((v - prev).abs());
                prev = v;
            }
        }
        // A 220 Hz sine at 0.5 changes at most ~0.03 per sample; glide may
        // raise the pitch by up to 1.4x but must not jump.
        assert!(worst < 0.2, "{worst}");
    }

    #[test]
    fn feedback_filters_darken_repeats() {
        let run = |lp: f32| {
            let mut d = free_delay(20.0, 20.0, 0.8, MODE_STEREO);
            d.set_param(ID_FB_LP, lp);
            d.prepare(SR, 256);
            let (l, _) = run_impulse(&mut d, 24_000);
            // Energy in the later repeats relative to the first.
            let t = (0.02 * SR) as usize;
            let e = |k: usize| l[k * t - 8..k * t + 8].iter().map(|v| v * v).sum::<f32>();
            e(5) / e(1)
        };
        assert!(run(1_000.0) < run(20_000.0) * 0.7);
    }

    #[test]
    fn tail_and_silence() {
        let mut d = free_delay(100.0, 100.0, 0.5, MODE_STEREO);
        let tail = d.tail_seconds().unwrap();
        assert!(tail > 0.5 && tail < 3.0, "{tail}");
        assert!(!d.is_silent() || tail > 0.0);
        let ctx = ProcessContext::new(SR);
        let mut l = vec![0.5; 256];
        let mut r = vec![0.5; 256];
        d.process(&ctx, &[], &mut l, &mut r);
        assert!(!d.is_silent());
        for _ in 0..((tail * SR) as usize / 256 + 4) {
            let mut l = vec![0.0; 256];
            let mut r = vec![0.0; 256];
            d.process(&ctx, &[], &mut l, &mut r);
        }
        assert!(d.is_silent());
    }
}
