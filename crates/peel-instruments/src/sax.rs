//! `peel.wind.sax`: a saxophone built as a waveguide model.
//!
//! The horn is one delay line closed on itself, in the spirit of Scavone's
//! single-reed waveguides (STK's Clarinet and Saxofony are the baseline). A
//! conical bore resonates at every harmonic, so the loop is positive and the
//! delay is one full period; a fixed low-pass (tone-hole lattice), a
//! pressure-controlled low-pass (lip damping) and a high-pass that keeps the
//! loop from resonating below the fundamental close it. At the mouthpiece the
//! Bernoulli reed flow of McIntyre, Schumacher and Woodhouse is solved once
//! into a table: the reed beats against the lay when blown hard, which is
//! what makes loud notes buzzy. A closed-loop tuner measures the sounding
//! period and trims the bore so the pitch stays put whatever the pressure.
//!
//! Radiated sound is what the bell does not reflect (a high-pass), shaped by
//! a tone low-pass that opens with breath pressure and a presence boost, then
//! by the body resonances fitted to University of Iowa alto sax recordings.
//! Breath noise is injected into the mouth pressure and, shaped around
//! 2 kHz, follows the instantaneous level of the tone; the tongue adds a
//! short burst and a pressure pulse that starts the note quickly.
//!
//! Expression: vibrato is delayed, irregular and moves breath pressure along
//! with pitch like a jaw vibrato; the attack scoops up into pitch; growl is a
//! 20 to 120 Hz pulsing of pressure and level; released notes sag a little.
//!
//! `Mono legato` plays one voice with last-note priority: overlapping notes
//! only change pitch (with optional portamento) and breath pressure.
//! `Poly` plays up to four independent voices.

use peel_dsp::biquad::{Biquad, Coeffs, DcBlocker, FilterKind, OnePoleLowpass};
use peel_dsp::envelope::Adsr;
use peel_dsp::noise::Noise;
use peel_dsp::osc::fast_sin;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Expression, safe_bend, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, ramp_coef,
};

const MODE: ParamId = 1;
const GLIDE: ParamId = 2;
const NOISE: ParamId = 10;
const B_ATTACK: ParamId = 11;
const B_RELEASE: ParamId = 12;
const SCOOP: ParamId = 13;
const BRIGHT: ParamId = 20;
const REED: ParamId = 21;
const EDGE: ParamId = 22;
const BODY: ParamId = 23;
const VIB_DEPTH: ParamId = 30;
const VIB_RATE: ParamId = 31;
const VIB_DELAY: ParamId = 32;
const GROWL: ParamId = 33;
const GROWL_RATE: ParamId = 34;
const LEVEL: ParamId = 40;
const VELOCITY: ParamId = 41;

const MODES: &[&str] = &["Mono legato", "Poly"];

static PARAMS: [ParamDescriptor; 17] = [
    ParamDescriptor::choice(MODE, "voice.mode", "Mode", "Voice", MODES, 0),
    ParamDescriptor::power(
        GLIDE,
        "voice.glide",
        "Glide",
        "Voice",
        0.0,
        1.0,
        0.0,
        Unit::Seconds,
        3.0,
    ),
    ParamDescriptor::linear(
        NOISE,
        "breath.noise",
        "Noise",
        "Breath",
        0.0,
        1.0,
        0.16,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        B_ATTACK,
        "breath.attack",
        "Attack",
        "Breath",
        0.005,
        0.6,
        0.05,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        B_RELEASE,
        "breath.release",
        "Release",
        "Breath",
        0.02,
        1.5,
        0.12,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        SCOOP,
        "breath.scoop",
        "Scoop",
        "Breath",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BRIGHT,
        "tone.bright",
        "Brightness",
        "Tone",
        0.0,
        1.0,
        0.55,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        REED,
        "tone.reed",
        "Reed",
        "Tone",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        EDGE,
        "tone.edge",
        "Edge",
        "Tone",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BODY,
        "tone.body",
        "Body",
        "Tone",
        0.7,
        1.4,
        1.0,
        Unit::Ratio,
    ),
    ParamDescriptor::linear(
        VIB_DEPTH,
        "vibrato.depth",
        "Vibrato",
        "Expression",
        0.0,
        100.0,
        25.0,
        Unit::Cents,
    ),
    ParamDescriptor::linear(
        VIB_RATE,
        "vibrato.rate",
        "Vib Rate",
        "Expression",
        2.0,
        9.0,
        5.3,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        VIB_DELAY,
        "vibrato.delay",
        "Vib Delay",
        "Expression",
        0.0,
        2.0,
        0.35,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        GROWL,
        "growl.amount",
        "Growl",
        "Expression",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        GROWL_RATE,
        "growl.rate",
        "Growl Rate",
        "Expression",
        20.0,
        120.0,
        70.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        LEVEL,
        "out.level",
        "Level",
        "Output",
        -24.0,
        6.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        VELOCITY,
        "out.velocity",
        "Velocity",
        "Output",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.wind.sax",
    name: "Sax",
    description: "Saxophone: waveguide reed model with vibrato, growl and legato",
    category: "Wind",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 8] = [
    Preset {
        name: "Alto",
        params: &[
            ("voice.mode", 0.0),
            ("voice.glide", 0.0),
            ("breath.noise", 0.16),
            ("breath.attack", 0.05),
            ("breath.release", 0.12),
            ("breath.scoop", 0.35),
            ("tone.bright", 0.55),
            ("tone.reed", 0.4),
            ("tone.edge", 0.3),
            ("tone.body", 1.0),
            ("vibrato.depth", 25.0),
            ("vibrato.rate", 5.3),
            ("vibrato.delay", 0.35),
            ("growl.amount", 0.0),
            ("growl.rate", 70.0),
            ("out.level", 0.0),
            ("out.velocity", 0.7),
        ],
    },
    Preset {
        name: "Tenor",
        params: &[
            ("breath.noise", 0.18),
            ("breath.attack", 0.065),
            ("breath.release", 0.15),
            ("tone.bright", 0.4),
            ("tone.reed", 0.35),
            ("tone.edge", 0.35),
            ("tone.body", 0.8),
            ("vibrato.depth", 22.0),
            ("vibrato.rate", 4.9),
        ],
    },
    Preset {
        name: "Soprano",
        params: &[
            ("breath.noise", 0.1),
            ("breath.attack", 0.035),
            ("breath.release", 0.09),
            ("breath.scoop", 0.25),
            ("tone.bright", 0.75),
            ("tone.reed", 0.55),
            ("tone.edge", 0.4),
            ("tone.body", 1.3),
            ("vibrato.depth", 30.0),
            ("vibrato.rate", 6.0),
        ],
    },
    Preset {
        name: "Breathy",
        params: &[
            ("breath.noise", 0.75),
            ("breath.attack", 0.12),
            ("breath.release", 0.25),
            ("breath.scoop", 0.5),
            ("tone.bright", 0.3),
            ("tone.reed", 0.25),
            ("tone.edge", 0.05),
            ("vibrato.depth", 18.0),
            ("vibrato.delay", 0.5),
            ("out.velocity", 0.85),
        ],
    },
    Preset {
        name: "Growl",
        params: &[
            ("voice.glide", 0.05),
            ("breath.noise", 0.25),
            ("breath.attack", 0.02),
            ("breath.scoop", 0.5),
            ("tone.bright", 0.8),
            ("tone.reed", 0.7),
            ("tone.edge", 0.8),
            ("tone.body", 0.9),
            ("vibrato.depth", 15.0),
            ("vibrato.delay", 0.6),
            ("growl.amount", 0.65),
            ("growl.rate", 80.0),
            ("out.velocity", 0.8),
        ],
    },
    Preset {
        name: "Section",
        params: &[
            ("voice.mode", 1.0),
            ("breath.noise", 0.12),
            ("breath.attack", 0.07),
            ("breath.release", 0.2),
            ("tone.bright", 0.5),
            ("tone.reed", 0.4),
            ("tone.edge", 0.25),
            ("vibrato.depth", 20.0),
            ("vibrato.delay", 0.5),
        ],
    },
    Preset {
        name: "Smooth Alto",
        params: &[
            ("breath.noise", 0.1),
            ("breath.attack", 0.07),
            ("breath.release", 0.18),
            ("breath.scoop", 0.2),
            ("tone.bright", 0.45),
            ("tone.reed", 0.35),
            ("tone.edge", 0.15),
            ("vibrato.depth", 20.0),
            ("vibrato.rate", 5.0),
            ("vibrato.delay", 0.45),
            ("out.velocity", 0.6),
        ],
    },
    Preset {
        name: "House Sax",
        params: &[
            ("voice.glide", 0.03),
            ("breath.noise", 0.14),
            ("breath.attack", 0.03),
            ("breath.release", 0.1),
            ("breath.scoop", 0.3),
            ("tone.bright", 0.7),
            ("tone.reed", 0.5),
            ("tone.edge", 0.5),
            ("tone.body", 1.05),
            ("vibrato.depth", 28.0),
            ("vibrato.rate", 5.6),
            ("vibrato.delay", 0.2),
            ("out.velocity", 0.75),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Sax::new()))
}

const POLYPHONY: usize = 4;
const CAPACITY: usize = 6;
const STACK: usize = 16;
const DRIFT_HZ: f32 = 5.0;
const WANDER_HZ: f32 = 0.7;
const IRREG_HZ: f32 = 0.9;
/// Lowest pitch the bore delay line can hold.
const LOWEST_HZ: f32 = 28.0;

/// Resonances of the horn body: frequency (Hz), gain (dB), Q.
const FORMANTS: [(f32, f32, f32); 3] =
    [(440.0, 10.0, 2.2), (1150.0, -3.5, 4.0), (3200.0, 6.0, 0.6)];
const NOISE_HZ: f32 = 2200.0;
/// Overall scale that puts a full-velocity mid-register note near -6 dBFS.
const MASTER: f32 = 1.3;

/// Level of each voice in poly mode, leaving room for a chord.
const POLY_TRIM: f32 = 0.5;

/// Reed embouchure parameter of the Bernoulli flow model.
const REED_Z: f32 = 0.5;
/// How much a stiffer reed raises the pressure needed to speak.
const REED_STIFF: f32 = 0.5;
/// Round-trip gain of the bore at the fundamental.
const LOOP_GAIN: f32 = 0.995;
/// Mouth pressure (relative to the reed closing pressure) at no and full breath.
const PRESS_LO: f32 = 0.55;
const PRESS_RANGE: f32 = 0.45;
/// Cutoff of the lip-damping low-passes in the bore as a multiple of the
/// fundamental: the minimum, and how far hard blowing opens it.
const LOOP_RATIO: f32 = 3.5;
const LOOP_OPEN: f32 = 12.0;
const LOOP_OPEN_EXP: f32 = 1.3;
/// Fixed tone-hole lattice low-pass, as a multiple of the fundamental. It
/// keeps the fundamental the strongest mode so the horn does not overblow.
const LATTICE_RATIO: f32 = 2.0;
/// The loop high-pass sits at this fraction of the fundamental, so the
/// bore cannot resonate below its lowest mode.
const HP_RATIO: f32 = 0.4;
/// Strength of the closed-loop tuning correction, per measured period.
const TRIM_RATE: f32 = 0.3;
/// Size of the tongue-release pressure pulse that starts the note.
const KICK: f32 = 0.1;
/// Output level follows breath pressure to this power.
const GAIN_EXP: f32 = 1.4;
/// Output tone low-pass cutoffs (Hz) at the breath pressures in `TONE_AT`.
const TONE_HZ: [f32; 3] = [1300.0, 5500.0, 11000.0];
const TONE_AT: [f32; 3] = [0.33, 0.55, 0.82];
/// Presence boost at 1.9 kHz for low notes (below `BITE_LOW_HZ`) and per
/// unit of the edge control, both only when blown hard.
const BITE_LOW_HZ: f32 = 270.0;
const BITE_LOW_DB: f32 = 12.0;
const BITE_BASE_DB: f32 = 3.0;
const BITE_EDGE_DB: f32 = 7.0;

/// Gain that normalizes a unit-variance white noise passed through a
/// one-pole low-pass at `hz`, sampled every `step` samples.
fn lp_norm(hz: f32, sr: f32, step: f32) -> f32 {
    let a = 1.0 - (-std::f32::consts::TAU * hz * step / sr).exp();
    1.0 / (a / (2.0 - a) / 3.0).sqrt()
}

const REED_N: usize = 1024;
const REED_S_MIN: f32 = -0.5;

/// Flow through the reed as a function of `s = pm - 2 * r`, solved once from
/// the Bernoulli model: `s = z x + x^2 - z x^3` and `u = z x (1 - x^2)`,
/// with `x` the root in `[x_lo, 1]` (`z` is the embouchure parameter).
fn build_reed(z: f32) -> Vec<f32> {
    let z = z as f64;
    let f = |x: f64| z * x + x * x - z * x * x * x;
    let x_lo = (1.0 - (1.0 + 3.0 * z * z).sqrt()) / (3.0 * z);
    let mut t = Vec::with_capacity(REED_N);
    for i in 0..REED_N {
        let s = REED_S_MIN as f64 + (1.0 - REED_S_MIN as f64) * i as f64 / (REED_N - 1) as f64;
        let (mut lo, mut hi) = (x_lo, 1.0f64);
        if s <= f(lo) {
            hi = lo;
        } else {
            for _ in 0..60 {
                let mid = 0.5 * (lo + hi);
                if f(mid) < s {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
        }
        let x = 0.5 * (lo + hi);
        t.push((z * x * (1.0 - x * x)) as f32);
    }
    t
}

#[inline]
fn reed_flow(t: &[f32], s: f32) -> f32 {
    if s >= 1.0 {
        return 0.0;
    }
    let x = ((s - REED_S_MIN) * ((REED_N - 1) as f32 / (1.0 - REED_S_MIN))).max(0.0);
    let i = x as usize;
    let fr = x - i as f32;
    t[i] + (t[i + 1] - t[i]) * fr
}

/// Settings shared by all voices, refreshed once per control interval.
#[derive(Clone, Copy)]
struct Shared {
    sr: f32,
    ramp_k: f32,
    glide_k: f32,
    legato_k: f32,
    scoop_decay: f32,
    drift_scale: f32,
    wander_scale: f32,
    irreg_scale: f32,
    bright: f32,
    reed: f32,
    edge: f32,
    noise: f32,
    growl: f32,
    growl_inc: f32,
    vib_depth: f32,
    vib_rate: f32,
    vib_delay: f32,
    scoop: f32,
    out_gain: f32,
    lvl: Coeffs,
}

struct SaxVoice {
    active: bool,
    gate: bool,
    env: Adsr,
    fade: Fade,
    /// Live expression, set by the synth each control interval.
    bend: f32,
    xgain: f32,
    /// Runtime voice ID whose per-note expression this voice plays (in mono,
    /// the note on top of the key stack).
    id: u32,
    oct: f32,
    cur_key: f32,
    target_key: f32,
    glide_on: bool,
    press: f32,
    press_target: f32,
    age: f32,
    scoop_env: f32,
    vib_phase: f32,
    growl_phase: f32,
    drift: OnePoleLowpass,
    wander: OnePoleLowpass,
    irreg_r: OnePoleLowpass,
    irreg_d: OnePoleLowpass,
    rng: Noise,
    // Bore: one delay line closed by a low-pass reflection and the reed.
    line: Vec<f32>,
    mask: usize,
    w: usize,
    d_cur: f32,
    d_step: f32,
    ap_x: f32,
    ap_y: f32,
    lp: f32,
    lp2: f32,
    lp_a: f32,
    la: f32,
    la_a: f32,
    loop_g: f32,
    dc_x: f32,
    dc_y: f32,
    dc_r: f32,
    // Pitch tracker: measures the sounding period and trims the bore length.
    pt: f32,
    pt_prev: f32,
    pt_amp: f32,
    pt_neg: bool,
    pt_a: f32,
    t_now: f64,
    t_cross: f64,
    p_meas: f32,
    p_tgt: f32,
    p_tgt_now: f32,
    trim: f32,
    fresh: bool,
    // Reed and mouth pressure.
    pm: f32,
    pm_step: f32,
    pe: f32,
    growl_amt: f32,
    // Output.
    nbp: Biquad,
    tone: Biquad,
    bite: Biquad,
    o_env: f32,
    gain: f32,
    gain_step: f32,
    kick: f32,
    chiff: f32,
    chiff_k: f32,
    chiff_amt: f32,
    noise_amt: f32,
    sr: f32,
}

/// Breath envelope times captured at note start.
struct Breath {
    attack: f32,
    release: f32,
}

impl SaxVoice {
    fn new(sr: f32, seed: u32) -> SaxVoice {
        let len = ((sr / LOWEST_HZ) as usize + 8).next_power_of_two();
        let ctrl = sr / CTRL as f32;
        SaxVoice {
            active: false,
            gate: false,
            env: Adsr::new(sr),
            fade: Fade::new(),
            bend: 0.0,
            xgain: 1.0,
            id: 0,
            oct: 0.0,
            cur_key: 60.0,
            target_key: 60.0,
            glide_on: false,
            press: 0.0,
            press_target: 0.0,
            age: 0.0,
            scoop_env: 0.0,
            vib_phase: 0.0,
            growl_phase: 0.0,
            drift: OnePoleLowpass::new(ctrl, DRIFT_HZ),
            wander: OnePoleLowpass::new(ctrl, WANDER_HZ),
            irreg_r: OnePoleLowpass::new(ctrl, IRREG_HZ),
            irreg_d: OnePoleLowpass::new(ctrl, IRREG_HZ * 0.8),
            rng: Noise::new(seed),
            line: vec![0.0; len],
            mask: len - 1,
            w: 0,
            d_cur: 100.0,
            d_step: 0.0,
            ap_x: 0.0,
            ap_y: 0.0,
            lp: 0.0,
            lp2: 0.0,
            lp_a: 0.5,
            la: 0.0,
            la_a: 0.5,
            loop_g: 0.98,
            dc_x: 0.0,
            dc_y: 0.0,
            dc_r: 1.0 - std::f32::consts::TAU * 6.0 / sr,
            pt: 0.0,
            pt_prev: 0.0,
            pt_amp: 0.0,
            pt_neg: false,
            pt_a: 0.1,
            t_now: 0.0,
            t_cross: -1.0,
            p_meas: 0.0,
            p_tgt: 0.0,
            p_tgt_now: 100.0,
            trim: 1.0,
            fresh: false,
            pm: 0.0,
            pm_step: 0.0,
            pe: 0.0,
            growl_amt: 0.0,
            nbp: Biquad::default(),
            tone: Biquad::default(),
            bite: Biquad::default(),
            o_env: 0.0,
            gain: 0.0,
            gain_step: 0.0,
            kick: 0.0,
            chiff: 0.0,
            chiff_k: (-1.0 / (0.012 * sr)).exp(),
            chiff_amt: 0.0,
            noise_amt: 0.0,
            sr,
        }
    }

    fn clear_state(&mut self) {
        self.line.fill(0.0);
        self.w = 0;
        self.ap_x = 0.0;
        self.ap_y = 0.0;
        self.lp = 0.0;
        self.lp2 = 0.0;
        self.la = 0.0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        self.nbp.reset();
        self.tone.reset();
        self.bite.reset();
        self.drift.reset();
        self.wander.reset();
        self.irreg_r.reset();
        self.irreg_d.reset();
        self.vib_phase = 0.0;
        self.growl_phase = 0.0;
        self.o_env = 0.0;
        self.pm_step = 0.0;
        self.d_step = 0.0;
        self.pt = 0.0;
        self.pt_prev = 0.0;
        self.pt_amp = 0.0;
        self.pt_neg = false;
        self.t_cross = -1.0;
        self.p_meas = 0.0;
        self.p_tgt = 0.0;
        self.trim = 1.0;
        self.fresh = false;
    }

    /// Starts a note. `retrigger` restarts the breath envelope, vibrato
    /// timing and scoop; otherwise only pitch and pressure change.
    fn start(&mut self, key: u8, velocity: f32, vel_sens: f32, retrigger: bool, b: &Breath) {
        // Even the softest note needs enough breath for the reed to speak.
        let vel_press = ((1.0 - vel_sens) * 0.85 + vel_sens * velocity.powf(1.3)).max(0.28);
        self.press_target = vel_press;
        self.target_key = key as f32;
        if retrigger {
            if !self.active {
                self.clear_state();
                self.press = vel_press;
                // Start the bore at the right length.
                self.d_cur = self.sr / key_to_hz(key as f32);
            }
            self.cur_key = key as f32;
            self.env.set(b.attack, 0.12, 0.9, b.release, false);
            self.env.note_on();
            self.age = 0.0;
            self.scoop_env = 1.0;
            self.chiff = 1.0;
            self.kick = 1.0;
            self.chiff_amt = 0.35 + 0.65 * velocity;
        }
        self.fade.reset();
        self.gate = true;
        self.active = true;
    }

    fn release(&mut self) {
        self.gate = false;
        self.env.note_off();
    }

    /// Phase delay (samples) of the loop filters at `f0`.
    fn filter_delay(&self, f0: f32) -> f32 {
        let w = std::f32::consts::TAU * f0 / self.sr;
        let (s, c) = w.sin_cos();
        let b = 1.0 - self.lp_a;
        let lp = (b * s).atan2(1.0 - b * c);
        let ba = 1.0 - self.la_a;
        let la = (ba * s).atan2(1.0 - ba * c);
        let r = self.dc_r;
        let num = s.atan2(1.0 - c);
        let den = (r * s).atan2(1.0 - r * c);
        (2.0 * lp + la + den - num) / w
    }

    fn control(&mut self, sh: &Shared) {
        let dt = CTRL as f32 / sh.sr;
        self.age += dt;
        let k = if self.glide_on {
            sh.glide_k
        } else {
            sh.legato_k
        };
        self.cur_key += (self.target_key - self.cur_key) * k;
        self.press += (self.press_target - self.press) * sh.ramp_k;

        // Vibrato: delayed, with wandering rate and depth, and a smaller
        // pressure component like a real jaw vibrato.
        let rate =
            sh.vib_rate * (1.0 + 0.09 * self.irreg_r.process(self.rng.white()) * sh.irreg_scale);
        self.vib_phase += rate * dt;
        self.vib_phase -= self.vib_phase.floor();
        let f = ((self.age - sh.vib_delay) / 0.7).clamp(0.0, 1.0);
        let fade_in = f * f * (3.0 - 2.0 * f);
        let depth = sh.vib_depth
            * 0.01
            * (1.0 + 0.2 * self.irreg_d.process(self.rng.white()) * sh.irreg_scale)
            * fade_in;
        let vib = fast_sin(self.vib_phase) * depth;
        let vib_p = fast_sin(self.vib_phase - 0.12) * (depth * 0.5).min(0.18);

        let drift = self.drift.process(self.rng.white()) * sh.drift_scale * 0.02
            + self.wander.process(self.rng.white()) * sh.wander_scale * 0.035;
        self.scoop_env *= sh.scoop_decay;
        let growl_fm = sh.growl * 0.1 * fast_sin(self.growl_phase);
        // A released note sags a little as the breath dies away.
        let sag = if self.gate {
            0.0
        } else {
            (1.0 - self.env.level() / 0.9).clamp(0.0, 1.0)
        };
        let key = self.cur_key + vib + drift + growl_fm
            - sh.scoop * 0.9 * self.scoop_env
            - 0.12 * sag * sag;
        let key = key + safe_bend(key, self.bend, 0.12 * sh.sr);
        let f0 = key_to_hz(key).clamp(LOWEST_HZ, 0.12 * sh.sr);

        // Breath pressure: envelope, vibrato, slow flutter.
        let flutter = 0.6 * drift;
        let pe = (self.env.level() * self.press * (1.0 + vib_p + flutter)).clamp(0.0, 1.2);
        self.pe = pe;
        self.growl_amt = sh.growl;

        // Loop filter: tone-hole lattice, bell reflection and lip damping.
        // Soft blowing only lets the lowest modes speak; hard blowing opens
        // the upper ones.
        let ratio = LOOP_RATIO * (1.0 + LOOP_OPEN * sh.bright * pe.powf(LOOP_OPEN_EXP));
        let fc = (f0 * ratio).clamp(500.0, 0.4 * sh.sr);
        self.lp_a = 1.0 - (-std::f32::consts::TAU * fc / sh.sr).exp();
        let fa = (f0 * LATTICE_RATIO).min(0.45 * sh.sr);
        self.la_a = 1.0 - (-std::f32::consts::TAU * fa / sh.sr).exp();

        // Mouth pressure relative to the closing pressure of the reed; a
        // stiffer reed needs more.
        self.loop_g = LOOP_GAIN;
        let boost = 0.12 * (-self.age / 0.05).exp() * self.chiff_amt;
        let pm_new = (PRESS_LO + PRESS_RANGE * pe + boost) / (1.0 + REED_STIFF * (sh.reed - 0.4));
        self.pm_step = (pm_new - self.pm) / CTRL as f32;

        // The loop must not resonate below the lowest mode: a high-pass
        // follows the fundamental.
        self.dc_r = 1.0 - std::f32::consts::TAU * HP_RATIO * f0 / sh.sr;
        self.p_tgt_now = sh.sr / f0;
        self.pt_a = 1.0 - (-std::f32::consts::TAU * 1.3 * f0 / sh.sr).exp();
        if self.fresh {
            // One correction per new period measurement.
            self.fresh = false;
            let e = self.p_meas / self.p_tgt - 1.0;
            self.trim = (self.trim * (1.0 - TRIM_RATE * e)).clamp(0.7, 1.4);
        }
        let tau = self.filter_delay(f0);
        let d_target = (sh.sr * self.trim / f0 - tau).clamp(3.0, (self.mask - 4) as f32);
        self.d_step = (d_target - self.d_cur) / CTRL as f32;

        // Tone: blowing harder opens the upper partials. The cutoff follows
        // breath pressure on a log scale, never below the fundamental.
        let [p0, p1, p2] = TONE_AT;
        let [c0, c1, c2] = TONE_HZ.map(f32::ln);
        let lc = if pe < p1 {
            c0 + (c1 - c0) * ((pe - p0) / (p1 - p0))
        } else {
            c1 + (c2 - c1) * ((pe - p1) / (p2 - p1))
        };
        let fo = (lc.exp() * (2.0f32).powf(2.0 * (sh.bright - 0.55) + self.oct))
            .max(1.15 * f0)
            .min(0.45 * sh.sr);
        self.tone.coeffs = Coeffs::design(FilterKind::LowPass, sh.sr, fo, 0.65, 0.0);
        // Bite: low notes are bright in absolute terms, and the edge control
        // adds presence when blown hard.
        let loud = ((pe - 0.3) / 0.5).clamp(0.0, 1.0);
        let low = ((BITE_LOW_HZ - f0) / 150.0).clamp(0.0, 1.0);
        let bite_db = (BITE_BASE_DB + BITE_LOW_DB * low + BITE_EDGE_DB * sh.edge) * loud;
        self.bite.coeffs = Coeffs::design(FilterKind::Peak, sh.sr, 1900.0, 0.5, bite_db);

        // Output level follows pressure.
        let comp = (sh.lvl.magnitude(sh.sr, f0) as f32).max(0.2).powf(-0.8);
        let gain_new = sh.out_gain * pe.powf(GAIN_EXP) * comp / (1.0 + 0.3 * low) * self.xgain;
        self.gain_step = (gain_new - self.gain) / CTRL as f32;
        // Air noise grows faster than linearly so the top of the range is
        // properly breathy while the default stays subtle.
        self.noise_amt = sh.noise * (1.0 + 3.0 * sh.noise * sh.noise);
    }

    #[inline]
    fn tick(&mut self, sh: &Shared, reed: &[f32]) -> f32 {
        let e = self.env.tick();
        self.d_cur += self.d_step;
        self.pm += self.pm_step;
        self.gain += self.gain_step;

        // Delay read with an all-pass fractional part.
        let dd = self.d_cur;
        let n = (dd - 0.5) as usize;
        let d = dd - n as f32;
        let eta = (1.0 - d) / (1.0 + d);
        let x0 = self.line[self.w.wrapping_sub(n) & self.mask];
        let y = eta * x0 + self.ap_x - eta * self.ap_y;
        self.ap_x = x0;
        self.ap_y = y;

        // Reflection: low-pass, loss, DC removal.
        self.la += self.la_a * (y - self.la);
        self.lp += self.lp_a * (self.la - self.lp);
        self.lp2 += self.lp_a * (self.lp - self.lp2);
        let r0 = self.lp2 * self.loop_g;
        let r = r0 - self.dc_x + self.dc_r * self.dc_y;
        self.dc_x = r0;
        self.dc_y = r;

        // Pitch tracking on the low-passed return wave.
        self.t_now += 1.0;
        self.pt += self.pt_a * (r - self.pt);
        self.pt_amp = (self.pt_amp * 0.9995).max(self.pt.abs());
        let thr = 0.15 * self.pt_amp;
        if self.pt < -thr {
            self.pt_neg = true;
        } else if self.pt_neg && self.pt > 0.0 && self.pt_prev <= 0.0 {
            let frac = (self.pt_prev / (self.pt_prev - self.pt)) as f64;
            let t = self.t_now - 1.0 + frac;
            if self.t_cross >= 0.0 {
                let per = (t - self.t_cross) as f32;
                if per > 0.6 * self.p_tgt_now && per < 1.6 * self.p_tgt_now {
                    self.p_meas = if self.p_meas > 0.0 {
                        self.p_meas + 0.3 * (per - self.p_meas)
                    } else {
                        per
                    };
                    self.p_tgt = if self.p_tgt > 0.0 {
                        self.p_tgt + 0.3 * (self.p_tgt_now - self.p_tgt)
                    } else {
                        self.p_tgt_now
                    };
                    self.fresh = true;
                }
            }
            self.t_cross = t;
            self.pt_neg = false;
        }
        self.pt_prev = self.pt;

        // Growl: pulsed modulation of the blowing pressure.
        self.growl_phase += sh.growl_inc;
        self.growl_phase -= self.growl_phase.floor();
        let m = 0.5 - 0.5 * fast_sin(self.growl_phase);
        let gm = 1.0 - self.growl_amt * 0.16 * m * m;

        // Mouth pressure with turbulence noise.
        let wn = self.rng.white();
        let pm = self.pm * gm + wn * self.noise_amt * 0.012 * (0.3 + self.pe);
        let mut new = r + reed_flow(reed, pm - 2.0 * r);
        if self.kick > 0.0 {
            // The tongue letting go: a short pressure pulse that seeds the
            // oscillation.
            self.kick = (self.kick - 1.0 / (0.0015 * self.sr)).max(0.0);
            let ph = 1.0 - self.kick;
            new += KICK * (ph * std::f32::consts::PI).sin().max(0.0) * self.chiff_amt;
        }
        let new = new.clamp(-4.0, 4.0);
        self.line[self.w & self.mask] = new;
        self.w = (self.w + 1) & self.mask;

        // Bell: radiated pressure is what the reflection does not return.
        let o = y - self.lp;
        self.o_env += 0.002 * (o.abs() - self.o_env);
        let mut o = self.bite.process(self.tone.process(o));

        // Jet noise shaped around 2 kHz, riding on the tone's own level.
        let nz = self.nbp.process(wn);
        let rel = o.abs() / (self.o_env + 1.0e-4);
        let mod_ = (0.35 + 0.45 * rel).min(2.5);
        o += nz * self.noise_amt * (0.03 * self.pe + 0.1 * self.o_env) * mod_;

        // Tonguing: a short burst at the attack.
        self.chiff *= self.chiff_k;
        o += nz * self.chiff * self.chiff_amt * 0.04;

        let x = o * (1.0 - sh.growl * 0.6 * m * m);
        let out = x * self.gain * self.fade.tick() * (e * 12.0).min(1.0);
        if self.env.is_idle() || self.fade.is_done() {
            self.active = false;
        }
        if !out.is_finite() {
            self.clear_state();
            return 0.0;
        }
        out
    }
}

impl PoolVoice for SaxVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.env.level() * self.press
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
        self.gate = false;
        self.env.reset();
    }
}

#[derive(Clone, Copy)]
struct Held {
    id: u32,
    key: u8,
}

pub(crate) struct Sax {
    expr: Expression,
    sr: f32,
    ctrl: usize,
    pool: Pool<SaxVoice>,
    dc: DcBlocker,
    form: [Biquad; 3],
    reed_tab: Vec<f32>,
    sh: Shared,
    stack: [Held; STACK],
    stack_len: usize,
    // Unsmoothed settings.
    mono: bool,
    attack: f32,
    release: f32,
    vel_sens: f32,
    glide_time: f32,
    // Smoothed settings.
    noise: Ramp,
    bright: Ramp,
    reed: Ramp,
    edge: Ramp,
    body: Ramp,
    vib_depth: Ramp,
    vib_rate: Ramp,
    growl: Ramp,
    growl_rate: Ramp,
    scoop: Ramp,
    level: Ramp,
    formant_body: f32,
}

impl Sax {
    pub(crate) fn new() -> Sax {
        let sr = 48_000.0;
        Sax {
            expr: Expression::new(),
            sr,
            ctrl: 0,
            pool: Self::make_pool(sr),
            dc: DcBlocker::new(sr, 20.0),
            form: [Biquad::default(); 3],
            reed_tab: build_reed(REED_Z),
            sh: Self::make_shared(sr),
            stack: [Held { id: 0, key: 0 }; STACK],
            stack_len: 0,
            mono: true,
            attack: 0.05,
            release: 0.12,
            vel_sens: 0.7,
            glide_time: 0.0,
            noise: Ramp::new(0.16),
            bright: Ramp::new(0.55),
            reed: Ramp::new(0.4),
            edge: Ramp::new(0.3),
            body: Ramp::new(1.0),
            vib_depth: Ramp::new(25.0),
            vib_rate: Ramp::new(5.3),
            growl: Ramp::new(0.0),
            growl_rate: Ramp::new(70.0),
            scoop: Ramp::new(0.35),
            level: Ramp::new(0.0),
            formant_body: f32::NAN,
        }
    }

    fn make_pool(sr: f32) -> Pool<SaxVoice> {
        let seed = std::cell::Cell::new(1u32);
        Pool::new(CAPACITY, POLYPHONY, StealPolicy::Quietest, sr, || {
            let s = seed.get();
            seed.set(s + 1);
            SaxVoice::new(sr, s)
        })
    }

    fn make_shared(sr: f32) -> Shared {
        let ctrl = sr / CTRL as f32;
        Shared {
            sr,
            ramp_k: ramp_coef(sr),
            glide_k: 1.0,
            legato_k: 1.0 - (-(CTRL as f32) / (0.012 * sr)).exp(),
            scoop_decay: (-(CTRL as f32) / (0.06 * sr)).exp(),
            drift_scale: lp_norm(DRIFT_HZ, ctrl, 1.0),
            wander_scale: lp_norm(WANDER_HZ, ctrl, 1.0),
            irreg_scale: lp_norm(IRREG_HZ, ctrl, 1.0),
            bright: 0.55,
            reed: 0.4,
            edge: 0.3,
            noise: 0.16,
            growl: 0.0,
            growl_inc: 70.0 / sr,
            vib_depth: 25.0,
            vib_rate: 5.3,
            vib_delay: 0.35,
            scoop: 0.35,
            out_gain: MASTER,
            lvl: Coeffs::IDENTITY,
        }
    }

    fn breath(&self) -> Breath {
        Breath {
            attack: self.attack,
            release: self.release,
        }
    }

    /// Copies the smoothed settings into the shared block, advancing each
    /// ramp by `k`.
    fn refresh(&mut self, k: f32) {
        let sh = &mut self.sh;
        sh.noise = self.noise.tick(k);
        sh.bright = self.bright.tick(k);
        sh.reed = self.reed.tick(k);
        sh.edge = self.edge.tick(k);
        sh.vib_depth = self.vib_depth.tick(k);
        sh.vib_rate = self.vib_rate.tick(k);
        sh.growl = self.growl.tick(k);
        sh.growl_inc = self.growl_rate.tick(k) / self.sr;
        sh.scoop = self.scoop.tick(k);
        let trim = if self.mono { 1.0 } else { POLY_TRIM };
        sh.out_gain = MASTER * trim * db_to_gain(self.level.tick(k));
        self.body.tick(k);
    }

    fn set_formants(&mut self) {
        let body = self.body.get();
        self.formant_body = body;
        let sr = self.sr;
        for (f, &(hz, db, q)) in self.form.iter_mut().zip(FORMANTS.iter()) {
            f.coeffs = Coeffs::design(FilterKind::Peak, sr, hz * body, q, db);
        }
        self.sh.lvl = self.form[0].coeffs;
        let noise = Coeffs::design(FilterKind::BandPass, sr, NOISE_HZ, 0.45, 0.0);
        for v in &mut self.pool.voices {
            v.nbp.coeffs = noise;
        }
    }

    fn control(&mut self) {
        self.refresh(self.sh.ramp_k);
        if self.formant_body.is_nan() || (self.body.get() - self.formant_body).abs() > 1.0e-4 {
            self.set_formants();
        }
        self.expr.tick();
        let sh = self.sh;
        let ex = &self.expr;
        for v in &mut self.pool.voices {
            if v.active {
                let key = v.target_key.round().clamp(0.0, 127.0) as u8;
                let x = ex.voice(v.id, key);
                v.bend = x.bend;
                v.xgain = x.gain();
                v.oct = x.cutoff_oct();
                v.control(&sh);
            }
        }
    }

    fn update_glide(&mut self) {
        self.sh.glide_k = if self.glide_time <= 0.001 {
            1.0
        } else {
            1.0 - (-3.0 * CTRL as f32 / (self.glide_time * self.sr)).exp()
        };
    }

    fn release_everything(&mut self) {
        self.stack_len = 0;
        for v in &mut self.pool.voices {
            if v.gate {
                v.release();
            }
        }
        self.pool.release_all(|_| {});
    }
}

impl Synth for Sax {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.pool = Self::make_pool(sample_rate);
        self.dc = DcBlocker::new(sample_rate, 20.0);
        for f in &mut self.form {
            f.reset();
        }
        let delay = self.sh.vib_delay;
        self.sh = Self::make_shared(sample_rate);
        self.sh.vib_delay = delay;
        self.stack_len = 0;
        self.ctrl = 0;
        self.formant_body = f32::NAN;
        self.update_glide();
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        for v in &mut self.pool.voices {
            v.clear_state();
        }
        for f in &mut self.form {
            f.reset();
        }
        self.stack_len = 0;
        self.dc.reset();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            MODE => {
                let mono = v < 0.5;
                if mono != self.mono {
                    self.mono = mono;
                    self.release_everything();
                }
            }
            GLIDE => {
                self.glide_time = v;
                self.update_glide();
            }
            NOISE => self.noise.set(v, imm),
            B_ATTACK => self.attack = v,
            B_RELEASE => self.release = v,
            SCOOP => self.scoop.set(v, imm),
            BRIGHT => self.bright.set(v, imm),
            REED => self.reed.set(v, imm),
            EDGE => self.edge.set(v, imm),
            BODY => self.body.set(v, imm),
            VIB_DEPTH => self.vib_depth.set(v, imm),
            VIB_RATE => self.vib_rate.set(v, imm),
            VIB_DELAY => self.sh.vib_delay = v,
            GROWL => self.growl.set(v, imm),
            GROWL_RATE => self.growl_rate.set(v, imm),
            LEVEL => self.level.set(v, imm),
            VELOCITY => self.vel_sens = v,
            _ => {}
        }
        if imm {
            self.refresh(1.0);
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let breath = self.breath();
        let sens = self.vel_sens;
        if self.mono {
            let overlapping = self.stack_len > 0;
            if self.stack_len == STACK {
                self.stack.copy_within(1.., 0);
                self.stack_len -= 1;
            }
            self.stack[self.stack_len] = Held { id, key };
            self.stack_len += 1;
            let glide = self.sh.glide_k < 1.0;
            let v = &mut self.pool.voices[0];
            v.glide_on = glide && v.active;
            v.id = id;
            v.start(key, velocity, sens, !overlapping, &breath);
        } else {
            let slot = self.pool.begin(key, id);
            let v = &mut self.pool.voices[slot];
            v.active = false;
            v.glide_on = false;
            v.id = id;
            v.start(key, velocity, sens, true, &breath);
        }
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        if self.mono {
            let Some(pos) = self.stack[..self.stack_len].iter().position(|h| h.id == id) else {
                return;
            };
            let was_top = pos + 1 == self.stack_len;
            self.stack.copy_within(pos + 1..self.stack_len, pos);
            self.stack_len -= 1;
            let glide = self.sh.glide_k < 1.0;
            let v = &mut self.pool.voices[0];
            if self.stack_len == 0 {
                v.release();
            } else if was_top {
                // Fall back to the previous held note without retriggering.
                let back = self.stack[self.stack_len - 1];
                v.glide_on = glide;
                v.target_key = back.key as f32;
                v.id = back.id;
            }
        } else {
            self.pool.release_id(id, SaxVoice::release);
        }
    }

    fn all_notes_off(&mut self) {
        self.release_everything();
    }

    fn all_sound_off(&mut self) {
        self.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        for n in 0..left.len().min(right.len()) {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let mut sum = 0.0;
            let mut any = false;
            for v in &mut self.pool.voices {
                if v.active {
                    any = true;
                    sum += v.tick(&self.sh, &self.reed_tab);
                }
            }
            let out = if any {
                let mut x = sum;
                for f in &mut self.form {
                    x = f.process(x);
                }
                self.dc.process(x)
            } else {
                self.dc.reset();
                for f in &mut self.form {
                    f.reset();
                }
                0.0
            };
            left[n] = out;
            right[n] = out;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
