//! `peel.fx.tape`: the sound of a tape machine in one insert.
//!
//! The signal passes through a short delay whose length wanders (wow, a slow
//! drift under 1 Hz, and flutter, a faster shiver near 7 Hz), then a soft
//! saturation with a small bias, a low bump near 85 Hz where a playback
//! head resonates, and a gentle high cut. Hiss rides on the signal and
//! stops with it, so a silent track stays silent.
//!
//! The saturation is antialiased by integrating the curve between
//! consecutive samples (first-order ADAA), which needs no oversampling
//! filters. Only the part of the curve that bends is treated that way: the
//! straight part passes through untouched, so a quiet signal keeps its top
//! end and only the distortion products are band-limited. The gain is
//! normalized so turning Drive up squeezes rather than
//! clips: quiet material rises by half the drive, while a full-scale signal
//! stays just under full scale.
//!
//! The delay reports its resting length as latency, and the dry signal in
//! the mix is delayed to match, so a partial mix does not comb.

use peel_dsp::biquad::{Coeffs, DcBlocker, FilterKind, StereoBiquad};
use peel_dsp::delay::{DelayLine, Lfo};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::noise::Noise;
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain, sanitize};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

pub const ID_DRIVE: ParamId = 1;
pub const ID_BUMP: ParamId = 2;
pub const ID_TONE: ParamId = 3;
pub const ID_WOW: ParamId = 4;
pub const ID_FLUTTER: ParamId = 5;
pub const ID_HISS: ParamId = 6;
pub const ID_MIX: ParamId = 7;
pub const ID_OUTPUT: ParamId = 8;

static PARAMS: [ParamDescriptor; 8] = [
    ParamDescriptor::linear(
        ID_DRIVE,
        "drive",
        "Drive",
        "Tape",
        0.0,
        24.0,
        6.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_BUMP,
        "bump",
        "Head Bump",
        "Tape",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ID_TONE,
        "tone",
        "Tone",
        "Tape",
        1_500.0,
        20_000.0,
        12_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_WOW,
        "wow",
        "Wow",
        "Motion",
        0.0,
        1.0,
        0.15,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_FLUTTER,
        "flutter",
        "Flutter",
        "Motion",
        0.0,
        1.0,
        0.1,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_HISS,
        "hiss",
        "Hiss",
        "Noise",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(ID_MIX, "mix", "Mix", "Output", 0.0, 1.0, 1.0, Unit::Percent),
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
    type_id: "peel.fx.tape",
    name: "Tape",
    description: "Tape saturation, head bump, wow, flutter and hiss",
    category: "Distortion",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Warm",
        params: &[],
    },
    Preset {
        name: "Master Glue",
        params: &[
            ("drive", 3.0),
            ("bump", 0.15),
            ("tone", 20_000.0),
            ("wow", 0.0),
            ("flutter", 0.0),
        ],
    },
    Preset {
        name: "Cassette",
        params: &[
            ("drive", 9.0),
            ("bump", 0.4),
            ("tone", 6_500.0),
            ("wow", 0.45),
            ("flutter", 0.35),
            ("hiss", 0.35),
        ],
    },
    Preset {
        name: "Worn",
        params: &[
            ("drive", 12.0),
            ("bump", 0.5),
            ("tone", 3_800.0),
            ("wow", 0.7),
            ("flutter", 0.5),
            ("hiss", 0.5),
            ("output", -1.0),
        ],
    },
    Preset {
        name: "Wobble",
        params: &[("drive", 4.0), ("wow", 1.0), ("flutter", 0.2)],
    },
    Preset {
        name: "Hot",
        params: &[
            ("drive", 16.0),
            ("bump", 0.3),
            ("tone", 9_000.0),
            ("wow", 0.05),
            ("flutter", 0.05),
            ("output", -2.0),
        ],
    },
];

/// Tone settings from here up leave the top end alone.
const TONE_OPEN: f32 = 19_500.0;
/// Resting delay of the transport, in seconds.
const BASE_SECONDS: f32 = 0.003;
/// Bias of the saturation curve: a little asymmetry for even harmonics.
const BIAS: f64 = 0.08;
/// Hiss level at `hiss = 1` (linear, before the tone filter).
const HISS: f32 = 0.006;
/// Wow: two slow components, as peak pitch deviation and rate in Hz.
const WOW: [(f32, f32); 2] = [(0.004, 0.83), (0.001_5, 0.31)];
/// Flutter: two faster components.
const FLUTTER: [(f32, f32); 2] = [(0.001_2, 7.3), (0.000_5, 11.9)];

/// `ln(cosh(x))` without overflow.
fn ln_cosh(x: f64) -> f64 {
    let a = x.abs();
    a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
}

/// The saturation curve at input gain `s`: `tanh(s x + b) - tanh(b)`, split
/// into its slope at zero and the rest, with first-order ADAA on the rest.
#[derive(Clone, Copy, Default)]
struct Shaper {
    x1: f64,
}

impl Shaper {
    /// Slope of the curve at zero.
    fn slope(s: f64) -> f64 {
        s * (1.0 - BIAS.tanh().powi(2))
    }

    /// The curve minus its tangent at zero: nothing for quiet signals.
    fn bend(x: f64, s: f64) -> f64 {
        (s * x + BIAS).tanh() - BIAS.tanh() - Self::slope(s) * x
    }

    /// Antiderivative of [`Shaper::bend`] in `x`.
    fn integral(x: f64, s: f64) -> f64 {
        ln_cosh(s * x + BIAS) / s - BIAS.tanh() * x - 0.5 * Self::slope(s) * x * x
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
    }

    #[inline]
    fn tick(&mut self, x: f32, s: f64) -> f32 {
        let x = f64::from(x);
        let dx = x - self.x1;
        // Both ends are integrated at today's gain, so a moving Drive
        // never leaves a stale term in the difference.
        let bend = if dx.abs() < 1e-6 {
            Self::bend(0.5 * (x + self.x1), s)
        } else {
            (Self::integral(x, s) - Self::integral(self.x1, s)) / dx
        };
        self.x1 = x;
        (Self::slope(s) * x + bend) as f32
    }
}

/// Tape effect. See the module docs.
pub struct Tape {
    store: ParamStore,
    sample_rate: f32,
    base: f32,
    lines: [DelayLine; 2],
    dry: [DelayLine; 2],
    shaper: [Shaper; 2],
    dc: [DcBlocker; 2],
    bump: StereoBiquad,
    tone: StereoBiquad,
    hiss_hp: StereoBiquad,
    wow_lfo: [Lfo; 2],
    flutter_lfo: [Lfo; 2],
    noise: [Noise; 2],
    env: f32,
    env_release: f32,
    drive: Smoothed,
    wow: Smoothed,
    flutter: Smoothed,
    hiss: Smoothed,
    mix: Smoothed,
    output: Smoothed,
    silence: SilenceTracker,
}

impl Default for Tape {
    fn default() -> Self {
        Tape::new()
    }
}

impl Tape {
    pub fn new() -> Tape {
        let mut t = Tape {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            base: 144.0,
            lines: [DelayLine::new(512), DelayLine::new(512)],
            dry: [DelayLine::new(512), DelayLine::new(512)],
            shaper: [Shaper::default(); 2],
            dc: [DcBlocker::new(48_000.0, 8.0), DcBlocker::new(48_000.0, 8.0)],
            bump: StereoBiquad::default(),
            tone: StereoBiquad::default(),
            hiss_hp: StereoBiquad::default(),
            wow_lfo: [Lfo::new(0.0), Lfo::new(0.37)],
            flutter_lfo: [Lfo::new(0.11), Lfo::new(0.62)],
            noise: [Noise::new(0x7A9E), Noise::new(0x7A9F)],
            env: 0.0,
            env_release: 0.0,
            drive: Smoothed::new(1.0),
            wow: Smoothed::new(0.0),
            flutter: Smoothed::new(0.0),
            hiss: Smoothed::new(0.0),
            mix: Smoothed::new(1.0),
            output: Smoothed::new(1.0),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut t, 48_000.0, 512);
        t
    }

    fn smoothers(&mut self) -> [&mut Smoothed; 6] {
        [
            &mut self.drive,
            &mut self.wow,
            &mut self.flutter,
            &mut self.hiss,
            &mut self.mix,
            &mut self.output,
        ]
    }

    fn rewind(&mut self) {
        let sr = self.sample_rate;
        let phases = [[0.0, 0.37], [0.11, 0.62]];
        for (i, l) in self.wow_lfo.iter_mut().enumerate() {
            *l = Lfo::new(phases[0][i]);
            l.set_rate(WOW[i].1, sr);
        }
        for (i, l) in self.flutter_lfo.iter_mut().enumerate() {
            *l = Lfo::new(phases[1][i]);
            l.set_rate(FLUTTER[i].1, sr);
        }
        self.noise = [Noise::new(0x7A9E), Noise::new(0x7A9F)];
    }
}

/// Delay swing in samples for a pitch deviation `dev` at `hz`.
fn swing(dev: f32, hz: f32, sr: f32) -> f32 {
    dev / (std::f32::consts::TAU * hz) * sr
}

impl Fx for Tape {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        self.base = (BASE_SECONDS * sr).round();
        let len = (self.base as usize) * 2 + 16;
        self.lines = [DelayLine::new(len), DelayLine::new(len)];
        self.dry = [DelayLine::new(len), DelayLine::new(len)];
        self.dc = [DcBlocker::new(sr, 8.0), DcBlocker::new(sr, 8.0)];
        self.env_release = 1.0 - (-1.0 / (0.06 * sr)).exp();
        for s in self.smoothers() {
            s.set_ramp(sr, RAMP_SECONDS);
        }
        self.bump.set_ramp_time(sr, 0.02);
        self.tone.set_ramp_time(sr, 0.02);
        self.hiss_hp.set_immediate(Coeffs::design(
            FilterKind::HighPass,
            sr,
            1_200.0,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        ));
        refresh_all(self);
        for s in self.smoothers() {
            let t = s.target();
            s.set_immediate(t);
        }
        for f in [&mut self.bump, &mut self.tone] {
            let t = f.target();
            f.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        let sr = self.sample_rate;
        match id {
            ID_DRIVE => self.drive.set_target(db_to_gain(value)),
            ID_BUMP => self.bump.set_target(if value < 1e-3 {
                Coeffs::IDENTITY
            } else {
                Coeffs::design(FilterKind::Peak, sr, 85.0, 0.8, 5.0 * value)
            }),
            // Fully open is no filter at all, not a cut near Nyquist.
            ID_TONE => self.tone.set_target(if value >= TONE_OPEN {
                Coeffs::IDENTITY
            } else {
                Coeffs::design(FilterKind::LowPass, sr, value.min(0.45 * sr), 0.6, 0.0)
            }),
            ID_WOW => self.wow.set_target(value.powf(1.5)),
            ID_FLUTTER => self.flutter.set_target(value.powf(1.5)),
            ID_HISS => self.hiss.set_target(value * value * HISS),
            ID_MIX => self.mix.set_target(value),
            ID_OUTPUT => self.output.set_target(db_to_gain(value)),
            _ => {}
        }
    }

    fn clear(&mut self) {
        for l in self.lines.iter_mut().chain(self.dry.iter_mut()) {
            l.clear();
        }
        for sh in &mut self.shaper {
            sh.reset();
        }
        for d in &mut self.dc {
            d.reset();
        }
        self.bump.reset();
        self.tone.reset();
        self.hiss_hp.reset();
        self.env = 0.0;
        self.rewind();
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        let sr = self.sample_rate;
        let base = self.base;
        let wow_swing = [swing(WOW[0].0, WOW[0].1, sr), swing(WOW[1].0, WOW[1].1, sr)];
        let flutter_swing = [
            swing(FLUTTER[0].0, FLUTTER[0].1, sr),
            swing(FLUTTER[1].0, FLUTTER[1].1, sr),
        ];
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = self.drive.next();
            let wow = self.wow.next();
            let flutter = self.flutter.next();
            let hiss = self.hiss.next();
            let mix = self.mix.next();
            let out = self.output.next();
            let (in_l, in_r) = (sanitize(*l), sanitize(*r));

            // One transport moves both channels.
            let mut d = base;
            for i in 0..2 {
                d += wow * wow_swing[i] * self.wow_lfo[i].next_sine();
                d += flutter * flutter_swing[i] * self.flutter_lfo[i].next_sine();
            }
            let d = d.clamp(2.0, 2.0 * base);
            self.lines[0].push(in_l);
            self.lines[1].push(in_r);
            let tl = self.lines[0].read_hermite(d + 1.0);
            let tr = self.lines[1].read_hermite(d + 1.0);

            // Unity gain for quiet signals rises with the root of the drive.
            let s = 0.5 * f64::from(g);
            let norm = 2.0 / g.sqrt();
            let mut wl = self.shaper[0].tick(tl, s) * norm;
            let mut wr = self.shaper[1].tick(tr, s) * norm;
            wl = self.dc[0].process(wl);
            wr = self.dc[1].process(wr);
            (wl, wr) = self.bump.process(wl, wr);

            // Hiss only while there is signal to ride on.
            let level = in_l.abs().max(in_r.abs());
            self.env = level.max(self.env - self.env * self.env_release);
            let gate = ((self.env - 2e-4) / 2e-3).clamp(0.0, 1.0);
            let (hl, hr) = self
                .hiss_hp
                .process(self.noise[0].white(), self.noise[1].white());
            wl += hiss * gate * hl;
            wr += hiss * gate * hr;

            (wl, wr) = self.tone.process(wl, wr);

            self.dry[0].push(in_l);
            self.dry[1].push(in_r);
            let dl = self.dry[0].read(base as usize + 1);
            let dr = self.dry[1].read(base as usize + 1);
            *l = (dl + (wl - dl) * mix) * out;
            *r = (dr + (wr - dr) * mix) * out;
        }
    }

    fn latency_samples(&self) -> u32 {
        self.base as u32
    }

    fn tail(&self) -> Option<f32> {
        Some(2.0 * BASE_SECONDS + 0.5)
    }

    fn silent(&self) -> bool {
        self.silence
            .quiet_for((self.sample_rate * 0.5) as u32 + 2 * self.base as u32)
    }
}

impl_device!(Tape, DESCRIPTOR);
