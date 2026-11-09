//! `peel.keys.grand`: a synthesized acoustic piano using modal synthesis.
//!
//! Each note is a bank of damped two-pole resonators, one per partial and per
//! string, run eight at a time so the compiler can vectorize them.
//!
//! * Partials follow the stiff-string law `f_k = k f0 sqrt((1 + B k^2) /
//!   (1 + B))` with a per-register inharmonicity `B` taken from measurements
//!   of a Steinway (University of Iowa Musical Instrument Samples).
//! * The starting spectrum is the product of a hammer term, a strike-position
//!   comb and a fixed soundboard curve. The hammer term gets steeper as the
//!   key is struck more softly, so loud notes are brighter, not just louder.
//! * The two-stage decay comes from the unison strings: one string stands in
//!   for the bridge-coupled in-phase motion and dies quickly (the prompt
//!   sound), the others ring on slowly and beat against each other (the
//!   aftersound). Bass notes use two polarizations of the single string.
//! * Bass notes also get phantom partials (sums of transverse partials) and a
//!   short longitudinal ping.
//! * A hammer thump and tick, and a damper thud on release, are noise bursts.
//! * Everything passes through a stereo soundboard: a bank of short, dense
//!   resonances that glues the notes together. With the sustain pedal down an
//!   88 key bank of undamped strings answers the sound (sympathetic
//!   resonance).

use std::f32::consts::{PI, TAU};

use peel_dsp::filter::{OnePole, Svf};
use peel_dsp::noise::Noise;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz, pan_gains};
use peel_dsp::voice::{StealPolicy, VoiceState};
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, lerp, ramp_coef,
};

const TONE: ParamId = 1;
const STIFFNESS: ParamId = 2;
const STRIKE: ParamId = 3;
const HAMMER: ParamId = 4;
const DYNAMICS: ParamId = 5;
const MECHANICS: ParamId = 6;
const DETUNE: ParamId = 10;
const DECAY: ParamId = 11;
const AFTERSOUND: ParamId = 12;
const PHANTOM: ParamId = 13;
const PEDAL: ParamId = 20;
const DAMPER: ParamId = 21;
const SYMPATHY: ParamId = 22;
const WIDTH: ParamId = 30;
const LEVEL: ParamId = 31;
const BODY: ParamId = 32;

static PARAMS: [ParamDescriptor; 16] = [
    ParamDescriptor::linear(
        TONE,
        "tone.brightness",
        "Tone",
        "Tone",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        STIFFNESS,
        "tone.stiffness",
        "Stiffness",
        "Tone",
        0.25,
        4.0,
        1.0,
        Unit::None,
    ),
    ParamDescriptor::linear(
        STRIKE,
        "tone.strike",
        "Strike Point",
        "Tone",
        0.06,
        0.25,
        0.125,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        HAMMER,
        "tone.hammer",
        "Hammer Noise",
        "Tone",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        DYNAMICS,
        "tone.dynamics",
        "Dynamics",
        "Tone",
        0.0,
        1.0,
        0.8,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        MECHANICS,
        "tone.mechanics",
        "Mechanics",
        "Tone",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        DETUNE,
        "string.detune",
        "Detune",
        "Strings",
        0.0,
        4.0,
        1.0,
        Unit::Cents,
    ),
    ParamDescriptor::log(
        DECAY,
        "string.decay",
        "Decay",
        "Strings",
        0.25,
        4.0,
        1.0,
        Unit::None,
    ),
    ParamDescriptor::linear(
        AFTERSOUND,
        "string.aftersound",
        "Aftersound",
        "Strings",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        PHANTOM,
        "string.phantom",
        "Phantom",
        "Strings",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::toggle(PEDAL, "pedal.sustain", "Sustain Pedal", "Pedal", false),
    ParamDescriptor::log(
        DAMPER,
        "pedal.damper",
        "Damper Time",
        "Pedal",
        0.05,
        2.0,
        0.22,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        SYMPATHY,
        "pedal.sympathy",
        "Sympathy",
        "Pedal",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        WIDTH,
        "out.width",
        "Width",
        "Output",
        0.0,
        1.0,
        0.6,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BODY,
        "out.body",
        "Body",
        "Output",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
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
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.keys.grand",
    name: "Grand",
    description: "Synthesized grand piano with coupled strings, hammer model and soundboard",
    category: "Keys",
    kind: DeviceKind::Instrument,
    version: 2,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 9] = [
    Preset {
        name: "Natural",
        params: &[
            ("tone.brightness", 0.5),
            ("tone.stiffness", 1.0),
            ("tone.strike", 0.125),
            ("tone.hammer", 0.4),
            ("tone.dynamics", 0.8),
            ("tone.mechanics", 0.4),
            ("string.detune", 1.0),
            ("string.decay", 1.0),
            ("string.aftersound", 0.5),
            ("string.phantom", 0.5),
            ("pedal.sustain", 0.0),
            ("pedal.damper", 0.22),
            ("pedal.sympathy", 0.5),
            ("out.width", 0.6),
            ("out.body", 0.5),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Bright",
        params: &[
            ("tone.brightness", 0.8),
            ("tone.hammer", 0.55),
            ("tone.strike", 0.11),
            ("string.decay", 0.9),
        ],
    },
    Preset {
        name: "Soft",
        params: &[
            ("tone.brightness", 0.2),
            ("tone.hammer", 0.2),
            ("tone.dynamics", 0.6),
            ("tone.mechanics", 0.25),
            ("string.decay", 1.2),
            ("string.aftersound", 0.6),
            ("pedal.damper", 0.4),
            ("pedal.sympathy", 0.6),
        ],
    },
    Preset {
        name: "Close",
        params: &[
            ("tone.brightness", 0.65),
            ("tone.hammer", 0.7),
            ("tone.strike", 0.1),
            ("tone.dynamics", 0.9),
            ("tone.mechanics", 0.6),
            ("string.decay", 0.8),
            ("string.aftersound", 0.35),
            ("out.width", 0.35),
            ("out.body", 0.3),
        ],
    },
    Preset {
        name: "Upright",
        params: &[
            ("tone.brightness", 0.4),
            ("tone.stiffness", 1.8),
            ("tone.strike", 0.14),
            ("tone.hammer", 0.65),
            ("tone.mechanics", 0.7),
            ("string.detune", 2.2),
            ("string.decay", 0.55),
            ("string.aftersound", 0.25),
            ("string.phantom", 0.3),
            ("pedal.damper", 0.15),
            ("pedal.sympathy", 0.3),
            ("out.width", 0.3),
            ("out.body", 0.8),
        ],
    },
    Preset {
        name: "Honky-Tonk",
        params: &[
            ("tone.brightness", 0.7),
            ("tone.stiffness", 2.5),
            ("tone.hammer", 0.8),
            ("tone.mechanics", 0.6),
            ("string.detune", 3.8),
            ("string.decay", 0.6),
            ("string.aftersound", 0.2),
            ("pedal.damper", 0.12),
            ("out.width", 0.7),
            ("out.body", 0.7),
        ],
    },
    Preset {
        name: "Pop",
        params: &[
            ("tone.brightness", 0.78),
            ("tone.hammer", 0.45),
            ("tone.strike", 0.115),
            ("tone.dynamics", 0.7),
            ("tone.mechanics", 0.25),
            ("string.decay", 0.85),
            ("string.aftersound", 0.4),
            ("string.phantom", 0.35),
            ("pedal.sympathy", 0.4),
            ("out.width", 0.75),
            ("out.body", 0.4),
        ],
    },
    Preset {
        name: "Felt",
        params: &[
            ("tone.brightness", 0.15),
            ("tone.hammer", 0.3),
            ("tone.strike", 0.14),
            ("tone.dynamics", 0.55),
            ("tone.mechanics", 0.55),
            ("string.detune", 0.8),
            ("string.decay", 0.6),
            ("string.aftersound", 0.3),
            ("string.phantom", 0.2),
            ("pedal.damper", 0.3),
            ("pedal.sympathy", 0.35),
            ("out.width", 0.5),
            ("out.body", 0.65),
            ("out.level", 2.0),
        ],
    },
    Preset {
        name: "Hall",
        params: &[
            ("tone.brightness", 0.45),
            ("tone.hammer", 0.3),
            ("tone.mechanics", 0.3),
            ("string.decay", 1.3),
            ("string.aftersound", 0.65),
            ("pedal.damper", 0.45),
            ("pedal.sympathy", 0.8),
            ("out.width", 0.9),
            ("out.body", 0.7),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Grand::new()))
}

const LANES: usize = 8;
const CHUNKS: usize = 24;
const MAX_MODES: usize = LANES * CHUNKS;
const MAX_PARTIALS: usize = 72;
/// Mode amplitudes below this (about -90 dBFS) are inaudible.
const AUDIBLE: f32 = 3.0e-5;
/// Nepers per second for one dB per second of amplitude decay.
const NEPER_PER_DB: f32 = 0.115_129_25;
/// Loudness of a full-velocity note, set by measurement.
const NOTE_GAIN: f32 = 0.2245;

type Lane = [f32; LANES];

/// Note-independent settings snapshot used to start a voice.
#[derive(Clone, Copy)]
struct Settings {
    sr: f32,
    tone: f32,
    stiffness: f32,
    strike: f32,
    hammer: f32,
    dynamics: f32,
    mechanics: f32,
    detune: f32,
    decay: f32,
    after: f32,
    phantom: f32,
    damper: f32,
    width: f32,
}

/// Piecewise linear interpolation in a table of `(x, y)`; clamps at the ends.
fn interp(table: &[(f32, f32)], x: f32) -> f32 {
    if x <= table[0].0 {
        return table[0].1;
    }
    for w in table.windows(2) {
        if x <= w[1].0 {
            let t = (x - w[0].0) / (w[1].0 - w[0].0);
            return w[0].1 + (w[1].1 - w[0].1) * t;
        }
    }
    table[table.len() - 1].1
}

/// `(key, log10 B)`. Measured on a Steinway: B bottoms out around C3, jumps at
/// the bass/treble break and then climbs about a factor of 1.9 per octave.
const INHARMONICITY: [(f32, f32); 13] = [
    (21.0, -3.75),
    (24.0, -3.49),
    (36.0, -3.47),
    (45.0, -3.70),
    (48.0, -3.96),
    (57.0, -3.66),
    (60.0, -3.49),
    (69.0, -3.15),
    (72.0, -2.97),
    (81.0, -2.72),
    (84.0, -2.52),
    (96.0, -2.00),
    (108.0, -1.60),
];

fn inharmonicity(key: f32) -> f32 {
    10f32.powf(interp(&INHARMONICITY, key))
}

/// `(key, dB/s)` amplitude decay of the prompt sound's fundamental.
const RATE_PROMPT: [(f32, f32); 10] = [
    (21.0, 1.8),
    (30.0, 2.6),
    (36.0, 3.5),
    (46.0, 9.0),
    (52.0, 12.0),
    (60.0, 24.0),
    (72.0, 28.0),
    (84.0, 34.0),
    (96.0, 42.0),
    (108.0, 52.0),
];

/// `(key, ms)`: rise time of a mezzo-forte note.
const ATTACK_MS: [(f32, f32); 5] = [
    (21.0, 12.0),
    (48.0, 8.0),
    (72.0, 5.0),
    (96.0, 3.5),
    (108.0, 3.0),
];

/// `(key, factor)`: how much duller the hammer sounds in each register at the
/// same velocity. Treble hammers lose their top end faster; in the bass the
/// wound strings keep theirs.
const HAMMER_SOFTNESS: [(f32, f32); 8] = [
    (21.0, 0.35),
    (36.0, 0.4),
    (48.0, 0.55),
    (60.0, 1.0),
    (72.0, 1.9),
    (84.0, 2.6),
    (96.0, 3.2),
    (108.0, 3.6),
];

/// `(key, dB/s)` decay of the aftersound's fundamental.
const RATE_AFTER: [(f32, f32); 7] = [
    (21.0, 0.8),
    (36.0, 1.2),
    (48.0, 1.6),
    (60.0, 2.2),
    (72.0, 2.8),
    (84.0, 4.5),
    (108.0, 12.0),
];

/// `(key, dB)` loudness by register: bass notes radiate less than the middle
/// of the keyboard, and the top octave is quieter again.
const LEVEL_TRIM: [(f32, f32); 8] = [
    (21.0, -6.5),
    (36.0, -1.6),
    (45.0, 0.0),
    (60.0, 1.8),
    (72.0, 3.2),
    (84.0, 5.0),
    (96.0, 1.5),
    (108.0, -7.0),
];

/// `(Hz, dB)`: the radiated spectrum of a struck string before the hammer
/// term. A soundboard that barely moves below 100 Hz, a plateau around
/// 200-400 Hz and a slope of about -7 dB/octave above that, measured on Iowa
/// mf and ff Steinway notes.
const BODY_CURVE: [(f32, f32); 15] = [
    (20.0, -46.0),
    (33.0, -40.0),
    (65.0, -30.0),
    (131.0, -16.0),
    (190.0, -6.0),
    (262.0, 0.0),
    (330.0, -2.0),
    (524.0, -8.5),
    (1049.0, -16.0),
    (1580.0, -15.0),
    (2100.0, -18.0),
    (3200.0, -24.0),
    (4400.0, -27.0),
    (8000.0, -32.0),
    (20000.0, -44.0),
];

fn body_db(f: f32) -> f32 {
    let curve = interp(&BODY_CURVE, f);
    curve + fine_db(f)
}

fn hash_signed(i: i32) -> f32 {
    let mut x = (i as u32).wrapping_mul(0x9E37_79B1);
    x ^= x >> 15;
    x = x.wrapping_mul(0x85EB_CA77);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE3D);
    x ^= x >> 16;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// The soundboard's resonances make the spectrum of every note slightly
/// ragged, and the same way each time: a fixed value noise over log frequency.
fn fine_db(f: f32) -> f32 {
    let p = f.max(20.0).log2() * 4.0;
    let i = p.floor();
    let t = p - i;
    let t = t * t * (3.0 - 2.0 * t);
    let a = hash_signed(i as i32);
    let b = hash_signed(i as i32 + 1);
    3.0 * (a + (b - a) * t)
}

/// Velocity (0..1) to amplitude, the "dynamics" curve before the Dynamics
/// control flattens it.
fn velocity_curve(v: f32) -> f32 {
    db_to_gain(-26.0 * (1.0 - v).powf(0.9))
}

struct GrandVoice {
    active: bool,
    releasing: bool,
    pending_release: bool,
    a1: [Lane; CHUNKS],
    a2: [Lane; CHUNKS],
    s1: [Lane; CHUNKS],
    s2: [Lane; CHUNKS],
    sg: [Lane; CHUNKS],
    chunks: usize,
    /// Per chunk: the sample at which it and every later chunk have died.
    chunk_end: [u32; CHUNKS],
    age: u32,
    flush: u32,
    pan_l: f32,
    pan_r: f32,
    spread: f32,
    amp0: f32,
    after: f32,
    att: f32,
    att_step: f32,
    pr_env: f32,
    pr_coef: f32,
    af_env: f32,
    af_coef: f32,
    dmp: f32,
    dmp_coef: f32,
    lp_l: OnePole,
    lp_r: OnePole,
    lp_primed: bool,
    noise: Noise,
    tick_bp: Svf,
    tick_env: f32,
    tick_coef: f32,
    tick_gain: f32,
    thump_lp: OnePole,
    thump_env: f32,
    thump_coef: f32,
    thump_gain: f32,
    rel_bp: Svf,
    rel_env: f32,
    rel_rise: f32,
    rel_step: f32,
    rel_coef: f32,
    rel_gain: f32,
    mech: f32,
    fade: Fade,
    key: u8,
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
    /// Pitch ratio the resonator coefficients currently express.
    ratio: f32,
    /// Mode angle and twice the pole radius, kept so a live bend can
    /// recompute `a1` without touching the ringing state.
    ang: [[f32; LANES]; CHUNKS],
    two_r: [[f32; LANES]; CHUNKS],
}

impl GrandVoice {
    fn new() -> GrandVoice {
        GrandVoice {
            active: false,
            releasing: false,
            pending_release: false,
            a1: [[0.0; LANES]; CHUNKS],
            a2: [[0.0; LANES]; CHUNKS],
            s1: [[0.0; LANES]; CHUNKS],
            s2: [[0.0; LANES]; CHUNKS],
            sg: [[0.0; LANES]; CHUNKS],
            chunks: 1,
            chunk_end: [0; CHUNKS],
            age: 0,
            flush: 0,
            pan_l: 1.0,
            pan_r: 1.0,
            spread: 0.0,
            amp0: 0.0,
            after: 0.2,
            att: 1.0,
            att_step: 1.0,
            pr_env: 1.0,
            pr_coef: 0.999,
            af_env: 1.0,
            af_coef: 0.9999,
            dmp: 1.0,
            dmp_coef: 1.0,
            lp_l: OnePole::new(),
            lp_r: OnePole::new(),
            lp_primed: false,
            noise: Noise::new(1),
            tick_bp: Svf::new(),
            tick_env: 0.0,
            tick_coef: 0.0,
            tick_gain: 0.0,
            thump_lp: OnePole::new(),
            thump_env: 0.0,
            thump_coef: 0.0,
            thump_gain: 0.0,
            rel_bp: Svf::new(),
            rel_env: 0.0,
            rel_rise: 0.0,
            rel_step: 0.0,
            rel_coef: 0.0,
            rel_gain: 0.0,
            mech: 0.0,
            fade: Fade::new(),
            key: 60,
            xgain: 1.0,
            ratio: 1.0,
            ang: [[0.0; LANES]; CHUNKS],
            two_r: [[0.0; LANES]; CHUNKS],
        }
    }

    fn start(&mut self, s: &Settings, key: u8, velocity: f32, seed: u32) {
        let sr = s.sr;
        let inv_sr = 1.0 / sr;
        let kf = key as f32;
        let f0 = key_to_hz(kf);
        let nyq = 0.45 * sr;
        let vel = velocity.clamp(0.0, 1.0);

        self.a1 = [[0.0; LANES]; CHUNKS];
        self.a2 = [[0.0; LANES]; CHUNKS];
        self.s1 = [[0.0; LANES]; CHUNKS];
        self.s2 = [[0.0; LANES]; CHUNKS];
        self.sg = [[0.0; LANES]; CHUNKS];
        self.ang = [[0.0; LANES]; CHUNKS];
        self.two_r = [[0.0; LANES]; CHUNKS];
        self.ratio = 1.0;
        self.key = key;
        self.chunk_end = [0; CHUNKS];
        self.chunks = 1;
        self.age = 0;
        self.flush = 0;
        self.releasing = false;
        self.pending_release = false;
        self.dmp = 1.0;
        self.dmp_coef = 1.0;
        self.fade.reset();
        self.tick_env = 0.0;
        self.thump_env = 0.0;
        self.rel_env = 0.0;
        if f0 > nyq {
            self.active = false;
            return;
        }

        let bump = (inharmonicity(kf) * s.stiffness).min(0.12);
        let mut rng = Noise::new(seed.wrapping_mul(2_654_435_761) ^ key as u32);

        // Strings: bass notes are a single string with two polarizations,
        // the tenor has two strings and everything above has three.
        let nv = if key < 47 { 2 } else { 3 };
        let bass = key < 33;
        let ramp = ((kf - 30.0) / 17.0).clamp(0.0, 1.0);
        let ramp = ramp * ramp * (3.0 - 2.0 * ramp);
        let fa_mid = lerp(0.03, 0.3, s.after);
        let fa = lerp(0.42, fa_mid, ramp);
        let weights: [f32; 3] = match nv {
            2 => [1.0 - fa, fa, 0.0],
            _ => [1.0 - fa, 0.55 * fa, 0.45 * fa],
        };
        let d = s.detune * 1.4;
        let cents: [f32; 3] = if bass {
            [0.0, 0.5 * d * (0.6 + 0.8 * rng.uniform()), 0.0]
        } else if nv == 2 {
            [0.0, d * (0.7 + 0.6 * rng.uniform()), 0.0]
        } else {
            [
                0.0,
                d * (0.7 + 0.6 * rng.uniform()),
                -d * (0.5 + 0.8 * rng.uniform()),
            ]
        };
        let side: [f32; 3] = [0.0, 1.0, -1.0];

        // Hammer: spectral slope depends on velocity and Tone.
        let dyn_v = lerp(0.75, vel.max(0.03), s.dynamics);
        let tone_scale = 2f32.powf((s.tone - 0.5) * 3.0);
        let fc = 2_400.0 * tone_scale;
        let reg = interp(&HAMMER_SOFTNESS, kf);
        let slope = (7.0 * reg / (dyn_v * dyn_v)).min(45.0);
        let slope_ref = 7.0 * reg / (0.75 * 0.75);
        let x0 = s.strike * (1.0 + (60.0 - kf) * 0.002).clamp(0.8, 1.3);

        let rate_scale = 1.0 / s.decay;
        let rate_p = interp(&RATE_PROMPT, kf);
        let rate_a = interp(&RATE_AFTER, kf);

        let base_roll = (f0 / fc).powf(1.4);
        let mut freq = [0.0f32; MAX_PARTIALS];
        let mut amp = [0.0f32; MAX_PARTIALS];
        let mut phase = [0.0f32; MAX_PARTIALS];
        let mut sig_p = [0.0f32; MAX_PARTIALS];
        let mut e_ref = 0.0f32;
        let mut n_p = 0;
        let max_p = ((MAX_MODES - 12) / nv).min(MAX_PARTIALS);
        for k in 1..=max_p {
            let kk = k as f32;
            let fk = kk * f0 * ((1.0 + bump * kk * kk) / (1.0 + bump)).sqrt();
            if fk > nyq {
                break;
            }
            let ratio = fk / fc;
            // The hammer rolls off what is above the fundamental, so a
            // treble note's fundamental is not itself attenuated.
            let rolled = (ratio.powf(1.4) - base_roll).max(0.0);
            // A real hammer has width, so the strike-point nulls are shallow.
            let comb = (((PI * kk * x0).sin().powi(2) + 0.04) / 1.04).sqrt();
            let common = comb * db_to_gain(body_db(fk) + 0.8 * rng.white());
            let g = common * db_to_gain(-slope * rolled);
            let g_ref = common * db_to_gain(-slope_ref * rolled);
            e_ref += g_ref * g_ref;
            freq[k - 1] = fk;
            amp[k - 1] = g;
            // The radiated attack is not a clean impulse: soundboard
            // dispersion scatters the phases of the upper partials.
            phase[k - 1] = rng.white() * 0.65 * PI * fk / (fk + 700.0);
            sig_p[k - 1] = (rate_p + 4.5 * (fk * 0.001).powf(1.4)) * rate_scale * NEPER_PER_DB;
            n_p = k;
            if k >= 4 && g < 3.0e-5 * (fc / 2_400.0).max(0.2) {
                break;
            }
        }
        let n_p = n_p.max(1);

        // Loudness: normalize on a fixed reference spectrum so that harder
        // hits are both louder and brighter. High notes are a little quieter.
        let level = lerp(0.65, velocity_curve(vel), s.dynamics);
        let tilt = db_to_gain(interp(&LEVEL_TRIM, kf));
        let norm = NOTE_GAIN / e_ref.sqrt().max(1.0e-6) * level * tilt;
        let n0 = (0.5 + 0.35 * (1.0 - vel)) * 0.001 * sr; // hammer contact half-length

        let mut idx = 0usize;
        let set_mode = |me: &mut GrandVoice,
                        idx: &mut usize,
                        fk: f32,
                        amp_m: f32,
                        sigma: f32,
                        ph: f32,
                        sg: f32| {
            if *idx >= MAX_MODES || fk > nyq {
                return;
            }
            let w = TAU * fk * inv_sr;
            let r = (-sigma * inv_sr).exp();
            let (c, l) = (*idx / LANES, *idx % LANES);
            let y = |n: f32| amp_m * r.powf(n) * (w * (n - n0) + ph).cos();
            me.a1[c][l] = 2.0 * r * w.cos();
            me.ang[c][l] = w;
            me.two_r[c][l] = 2.0 * r;
            me.a2[c][l] = -r * r;
            me.s1[c][l] = y(-1.0);
            me.s2[c][l] = y(-2.0);
            me.sg[c][l] = sg;
            let life = if amp_m > AUDIBLE {
                (amp_m / AUDIBLE).ln() / sigma.max(1.0e-3)
            } else {
                0.0
            };
            let end = (life.min(120.0) * sr) as u32;
            me.chunk_end[c] = me.chunk_end[c].max(end);
            *idx += 1;
        };

        let mut max_amp = 0.0f32;
        let mut e_sum = 0.0f32;
        for k in 0..n_p {
            let a_k = amp[k] * norm;
            max_amp = max_amp.max(a_k);
            e_sum += a_k * a_k;
            for string in 0..nv {
                let ratio = (cents[string] * (std::f32::consts::LN_2 / 1200.0)).exp();
                let fk = freq[k] * ratio;
                let var = 1.0 + 0.06 * rng.white();
                let sigma = if string == 0 {
                    sig_p[k] * (1.0 + 0.15 * rng.white())
                } else {
                    let slow = (rate_a + 1.0 * (fk * 0.001).powf(1.3)) * rate_scale * NEPER_PER_DB;
                    slow * (1.0 + 0.12 * rng.white()) * if string == 2 { 1.15 } else { 1.0 }
                };
                set_mode(
                    self,
                    &mut idx,
                    fk,
                    a_k * weights[string] * var,
                    sigma,
                    phase[k],
                    side[string],
                );
            }
        }

        // Phantom partials (sums of two transverse partials) and a short
        // longitudinal ping, strongest on loud bass notes.
        let pw = ((66.0 - kf) / 30.0).clamp(0.0, 1.0) * s.phantom;
        if pw > 0.0 {
            let pairs: [(usize, usize); 6] = [(1, 2), (2, 3), (1, 3), (3, 4), (2, 4), (4, 5)];
            let vv = vel * vel;
            for &(a, b) in &pairs {
                if b > n_p {
                    continue;
                }
                let (ia, ib) = (a - 1, b - 1);
                let amp_ph = 0.12 * pw * vv * (amp[ia] * amp[ib]).sqrt() * norm;
                let sigma = 1.5 * 0.5 * (sig_p[ia] + sig_p[ib]);
                set_mode(
                    self,
                    &mut idx,
                    freq[ia] + freq[ib],
                    amp_ph,
                    sigma,
                    phase[ia],
                    0.0,
                );
            }
            let f_long = f0 * (13.0 + (60.0 - kf) * 0.05);
            set_mode(
                self,
                &mut idx,
                f_long,
                0.08 * pw * vv * max_amp,
                30.0 * rate_scale,
                0.3,
                0.0,
            );
        }

        self.chunks = idx.div_ceil(LANES).max(1);
        for c in (0..CHUNKS - 1).rev() {
            self.chunk_end[c] = self.chunk_end[c].max(self.chunk_end[c + 1]);
        }
        self.amp0 = e_sum.sqrt();

        // The note swells over a few milliseconds as the hammer pushes the
        // string and the bridge and soundboard answer; the tick supplies the
        // crisp part of the attack.
        let att_ms = interp(&ATTACK_MS, kf) * (1.0 + 0.8 * (1.0 - vel)) / tone_scale.sqrt();
        self.att = 0.0;
        self.att_step = 1.0 / (att_ms * 0.001 * sr).max(1.0);

        // Stereo: a player's view, bass left and treble right, plus a little
        // spread between strings.
        let pan = ((kf - 64.5) / 43.5).clamp(-1.0, 1.0) * 0.65 * s.width;
        let (l, r) = pan_gains(pan);
        self.pan_l = l;
        self.pan_r = r;
        self.spread = 0.22 * s.width;

        // Envelope estimates used for stealing and the release thud.
        self.after = fa;
        self.pr_env = 1.0;
        self.af_env = 1.0;
        let sig_f = sig_p[0].max(1.0e-3);
        let sig_s =
            ((rate_a + 1.0 * (f0 * 0.001).powf(1.3)) * rate_scale * NEPER_PER_DB).max(1.0e-3);
        self.pr_coef = (-sig_f * inv_sr).exp();
        self.af_coef = (-sig_s * inv_sr).exp();

        self.lp_primed = false;
        self.lp_l.reset();
        self.lp_r.reset();
        self.lp_l.set_cutoff(18_000.0, sr);
        self.lp_r.set_cutoff(18_000.0, sr);

        // Hammer tick and soundboard thump.
        self.noise.reseed(seed ^ 0x9E37);
        let amp_ref = NOTE_GAIN * level * tilt;
        let hit = s.hammer * vel.powf(1.5);
        self.tick_bp
            .set_q((2_200.0 + 2_500.0 * vel) * tone_scale.sqrt(), 0.8, sr);
        self.tick_bp.reset();
        self.tick_env = 1.0;
        self.tick_coef = (-6.9 / (0.006 * sr)).exp();
        self.tick_gain = hit * amp_ref * 0.55;
        self.thump_lp.reset();
        self.thump_lp.set_cutoff((f0 * 2.0).clamp(90.0, 320.0), sr);
        self.thump_env = 1.0;
        let thump_t = (0.05 * 2f32.powf(-(kf - 40.0) / 36.0)).clamp(0.02, 0.08);
        self.thump_coef = (-6.9 / (thump_t * sr)).exp();
        self.thump_gain = s.hammer * vel.powf(1.3) * amp_ref * 2.6;
        self.mech = s.mechanics;
        self.noise_prep_release(sr, f0);

        self.active = true;
    }

    fn noise_prep_release(&mut self, sr: f32, f0: f32) {
        self.rel_bp.set_q((f0 * 3.0).clamp(450.0, 1_100.0), 0.7, sr);
        self.rel_bp.reset();
        self.rel_coef = (-6.9 / (0.035 * sr)).exp();
        self.rel_step = 1.0 / (0.005 * sr);
    }

    fn lvl_now(&self) -> f32 {
        self.amp0 * ((1.0 - self.after) * self.pr_env + self.after * self.af_env)
    }

    fn release(&mut self, sr: f32, damper: f32, key: f32) {
        if self.releasing {
            return;
        }
        self.releasing = true;
        self.pending_release = false;
        let mut t = (damper * 2f32.powf(-(key - 60.0) / 30.0)).clamp(0.03, 4.0);
        // The top octaves have no dampers.
        if key > 90.0 {
            t *= 1.0 + (key - 90.0) * 0.8;
        }
        self.dmp_coef = (-6.908 / (t * sr)).exp();
        self.rel_env = 1.0;
        self.rel_rise = 0.0;
        self.rel_gain = if key <= 90.0 {
            self.mech * 0.5 * self.lvl_now().min(0.3)
        } else {
            0.0
        };
    }

    /// Moves every mode to `ratio` times its played frequency without
    /// disturbing the ringing state, so envelopes and decay carry on.
    fn retune(&mut self, ratio: f32) {
        if ratio == self.ratio {
            return;
        }
        self.ratio = ratio;
        let top = 0.9 * PI;
        for c in 0..self.chunks.min(CHUNKS) {
            for l in 0..LANES {
                let w = self.ang[c][l];
                let a = if ratio == 1.0 { w } else { (w * ratio).min(top.max(w)) };
                self.a1[c][l] = self.two_r[c][l] * a.cos();
            }
        }
    }

    /// Control-rate bookkeeping: drop inaudible partials, close the damper
    /// filter and flush denormals.
    fn control(&mut self, sr: f32) {
        self.age = self.age.saturating_add(CTRL as u32);
        if self.age > self.chunk_end[0] {
            self.active = false;
            return;
        }
        while self.chunks > 1 && self.age > self.chunk_end[self.chunks - 1] {
            self.chunks -= 1;
        }
        if self.releasing {
            let cutoff = 400.0 + 16_000.0 * self.dmp.powf(2.5);
            self.lp_l.set_cutoff(cutoff, sr);
            self.lp_r.set_cutoff(cutoff, sr);
        }
        self.flush += 1;
        if self.flush >= 8 {
            self.flush = 0;
            for c in 0..self.chunks {
                for j in 0..LANES {
                    if self.s1[c][j].abs() < 1.0e-20 && self.s2[c][j].abs() < 1.0e-20 {
                        self.s1[c][j] = 0.0;
                        self.s2[c][j] = 0.0;
                    }
                }
            }
        }
    }

    #[inline]
    fn tick(&mut self) -> (f32, f32) {
        let n = self.chunks.min(CHUNKS);
        let mut m = [0.0f32; LANES];
        let mut sd = [0.0f32; LANES];
        for c in 0..n {
            let (a1, a2, sg) = (&self.a1[c], &self.a2[c], &self.sg[c]);
            let (s1, s2) = (&mut self.s1[c], &mut self.s2[c]);
            for j in 0..LANES {
                let y = a1[j] * s1[j] + a2[j] * s2[j];
                s2[j] = s1[j];
                s1[j] = y;
                m[j] += y;
                sd[j] += y * sg[j];
            }
        }
        let mut mid = (m[0] + m[4]) + (m[1] + m[5]) + (m[2] + m[6]) + (m[3] + m[7]);
        let mut side = (sd[0] + sd[4]) + (sd[1] + sd[5]) + (sd[2] + sd[6]) + (sd[3] + sd[7]);
        if self.att < 1.0 {
            self.att = (self.att + self.att_step).min(1.0);
            let g = self.att * self.att * (3.0 - 2.0 * self.att);
            mid *= g;
            side *= g;
        }

        if self.thump_env > 1.0e-4 {
            let w = self.noise.white();
            let t = self.tick_bp.tick(w).band_unity * self.tick_env * self.tick_gain;
            let h = self.thump_lp.low(w) * self.thump_env * self.thump_gain;
            self.tick_env *= self.tick_coef;
            self.thump_env *= self.thump_coef;
            mid += t + h;
        }
        self.pr_env *= self.pr_coef;
        self.af_env *= self.af_coef;

        let mut l = self.pan_l * mid + self.spread * side;
        let mut r = self.pan_r * mid - self.spread * side;
        if self.releasing {
            self.dmp *= self.dmp_coef;
            if !self.lp_primed {
                // Start the damper filter from the current signal so that
                // closing it does not step the output.
                self.lp_primed = true;
                for _ in 0..24 {
                    self.lp_l.low(l);
                    self.lp_r.low(r);
                }
            }
            l = self.lp_l.low(l);
            r = self.lp_r.low(r);
        }
        let g = self.dmp * self.fade.tick() * self.xgain;
        l *= g;
        r *= g;
        if self.rel_env > 1.0e-3 {
            let w = self.noise.white();
            self.rel_rise = (self.rel_rise + self.rel_step).min(1.0);
            let x = self.rel_bp.tick(w).band_unity * self.rel_env * self.rel_gain * self.rel_rise;
            self.rel_env *= self.rel_coef;
            l += x * self.pan_l;
            r += x * self.pan_r;
        }
        if self.dmp < 1.0e-4 || self.fade.is_done() {
            self.active = false;
        }
        (l, r)
    }
}

impl PoolVoice for GrandVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.lvl_now() * self.dmp
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

/// A bank of band-pass resonators `y = b (x - x[-2]) + a1 y[-1] + a2 y[-2]`
/// driven by one input and mixed to stereo with per-resonator gains.
struct ResBank<const C: usize> {
    a1: [Lane; C],
    a2: [Lane; C],
    b: [Lane; C],
    gl: [Lane; C],
    gr: [Lane; C],
    s1: [Lane; C],
    s2: [Lane; C],
    x1: f32,
    x2: f32,
}

impl<const C: usize> ResBank<C> {
    fn new() -> ResBank<C> {
        ResBank {
            a1: [[0.0; LANES]; C],
            a2: [[0.0; LANES]; C],
            b: [[0.0; LANES]; C],
            gl: [[0.0; LANES]; C],
            gr: [[0.0; LANES]; C],
            s1: [[0.0; LANES]; C],
            s2: [[0.0; LANES]; C],
            x1: 0.0,
            x2: 0.0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Configures resonator `i`: frequency, T60, peak gain and output gains.
    fn set(&mut self, i: usize, sr: f32, hz: f32, t60: f32, peak: f32, gl: f32, gr: f32) {
        let (c, l) = (i / LANES, i % LANES);
        if hz >= 0.45 * sr {
            self.b[c][l] = 0.0;
            return;
        }
        let r = (-6.908 / (t60 * sr)).exp();
        let w = TAU * hz / sr;
        self.a1[c][l] = 2.0 * r * w.cos();
        self.a2[c][l] = -r * r;
        self.b[c][l] = (1.0 - r) * peak;
        self.gl[c][l] = gl;
        self.gr[c][l] = gr;
    }

    fn clear(&mut self) {
        self.s1 = [[0.0; LANES]; C];
        self.s2 = [[0.0; LANES]; C];
        self.x1 = 0.0;
        self.x2 = 0.0;
    }

    /// Sum of squared states, to tell when the bank has rung out.
    fn energy(&self) -> f32 {
        self.s1.iter().flatten().map(|v| v * v).sum()
    }

    fn flush(&mut self) {
        for c in 0..C {
            for j in 0..LANES {
                if self.s1[c][j].abs() < 1.0e-20 && self.s2[c][j].abs() < 1.0e-20 {
                    self.s1[c][j] = 0.0;
                    self.s2[c][j] = 0.0;
                }
            }
        }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> (f32, f32) {
        let xin = x - self.x2;
        self.x2 = self.x1;
        self.x1 = x;
        let mut l = [0.0f32; LANES];
        let mut r = [0.0f32; LANES];
        for c in 0..C {
            let (a1, a2, b) = (&self.a1[c], &self.a2[c], &self.b[c]);
            let (gl, gr) = (&self.gl[c], &self.gr[c]);
            let (s1, s2) = (&mut self.s1[c], &mut self.s2[c]);
            for j in 0..LANES {
                let y = b[j] * xin + a1[j] * s1[j] + a2[j] * s2[j];
                s2[j] = s1[j];
                s1[j] = y;
                l[j] += y * gl[j];
                r[j] += y * gr[j];
            }
        }
        (
            (l[0] + l[4]) + (l[1] + l[5]) + (l[2] + l[6]) + (l[3] + l[7]),
            (r[0] + r[4]) + (r[1] + r[5]) + (r[2] + r[6]) + (r[3] + r[7]),
        )
    }
}

const BODY_MODES: usize = 40;
const BODY_CHUNKS: usize = BODY_MODES / LANES;
const SYMP_KEYS: usize = 88;
const SYMP_CHUNKS: usize = SYMP_KEYS / LANES;

pub(crate) struct Grand {
    expr: Expression,
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    notes: u32,
    pool: Pool<GrandVoice>,
    set: Settings,
    pedal: bool,
    level: Ramp,
    body_amt: Ramp,
    symp_amt: Ramp,
    width_r: Ramp,
    out_gain: f32,
    body: Box<ResBank<BODY_CHUNKS>>,
    body_live: bool,
    symp: Box<ResBank<SYMP_CHUNKS>>,
    symp_live: bool,
    symp_gate: f32,
    idle: u32,
}

impl Grand {
    pub(crate) fn new() -> Grand {
        let mut g = Grand {
            expr: Expression::new(),
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            notes: 0,
            pool: Pool::new(36, 32, StealPolicy::Quietest, 48_000.0, GrandVoice::new),
            set: Settings {
                sr: 48_000.0,
                tone: 0.5,
                stiffness: 1.0,
                strike: 0.125,
                hammer: 0.4,
                dynamics: 0.8,
                mechanics: 0.4,
                detune: 1.0,
                decay: 1.0,
                after: 0.5,
                phantom: 0.5,
                damper: 0.22,
                width: 0.6,
            },
            pedal: false,
            level: Ramp::new(0.0),
            body_amt: Ramp::new(0.5),
            symp_amt: Ramp::new(0.5),
            width_r: Ramp::new(0.6),
            out_gain: 1.0,
            body: Box::new(ResBank::new()),
            body_live: false,
            symp: Box::new(ResBank::new()),
            symp_live: false,
            symp_gate: 0.0,
            idle: 0,
        };
        g.build_banks();
        g
    }

    /// Computes the soundboard and sympathetic-string resonators for the
    /// current sample rate.
    fn build_banks(&mut self) {
        let sr = self.sr;
        let mut rng = Noise::new(0x50AB_D0A7);
        *self.body = ResBank::new();
        for i in 0..BODY_MODES {
            let u = (i as f32 + 0.5 + 0.35 * rng.white()) / BODY_MODES as f32;
            let hz = 70.0 * 100f32.powf(u);
            let t60 =
                (0.12 * (hz / 100.0).powf(-0.28) * (0.7 + 0.6 * rng.uniform())).clamp(0.015, 0.14);
            let peak = (0.4 + 0.6 * rng.uniform()) * (hz / 400.0).powf(-0.12);
            // Similar magnitudes keep the image centered; random signs
            // decorrelate the channels.
            let gl = (0.7 + 0.3 * rng.uniform()) * if rng.white() < 0.0 { -1.0 } else { 1.0 };
            let gr = (0.7 + 0.3 * rng.uniform()) * if rng.white() < 0.0 { -1.0 } else { 1.0 };
            self.body.set(i, sr, hz, t60, peak, gl, gr);
        }
        *self.symp = ResBank::new();
        for i in 0..SYMP_KEYS {
            let key = 21.0 + i as f32;
            let hz = key_to_hz(key);
            let t60 = (3.0 * 2f32.powf(-(key - 21.0) / 40.0)).clamp(0.8, 3.5);
            let pan = ((key - 64.5) / 43.5).clamp(-1.0, 1.0) * 0.6;
            let (l, r) = pan_gains(pan);
            self.symp.set(i, sr, hz, t60, 1.0, l * 0.7, r * 0.7);
        }
        self.body_live = false;
        self.symp_live = false;
    }

    fn derive(&mut self) {
        self.out_gain = db_to_gain(self.level.get());
    }

    fn control(&mut self) {
        self.level.tick(self.ramp_k);
        self.body_amt.tick(self.ramp_k);
        self.symp_amt.tick(self.ramp_k);
        self.width_r.tick(self.ramp_k);
        self.derive();
        let sr = self.sr;
        self.expr.tick();
        let ex = &self.expr;
        let ratio = ex.bend_ratio();
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.xgain = ex.gain(v.key);
            v.retune(ratio);
            v.control(sr);
        }
        self.pool.sync();

        let quiet = self.pool.is_silent();
        // Pedal gate for the sympathetic strings (about 40 ms).
        let target = if self.pedal { 1.0 } else { 0.0 };
        self.symp_gate += (target - self.symp_gate) * 0.12;
        if self.symp_live {
            self.symp.flush();
            let dead = if self.pedal {
                quiet && self.symp.energy() < 1.0e-14
            } else {
                self.symp_gate < 1.0e-3
            };
            if dead {
                self.symp.clear();
                self.symp_live = false;
            }
        }
        if self.body_live {
            self.body.flush();
            if quiet && !self.symp_live {
                self.idle += CTRL as u32;
                if self.idle as f32 > 0.6 * sr {
                    self.body.clear();
                    self.body_live = false;
                }
            } else {
                self.idle = 0;
            }
        }
    }

    fn release_pending(&mut self) {
        let (sr, damper) = (self.sr, self.set.damper);
        for i in 0..self.pool.voices.len() {
            if self.pool.voices[i].pending_release
                && self.pool.alloc.slot(i).state == VoiceState::Held
            {
                let key = self.pool.alloc.slot(i).key as f32;
                self.pool.alloc.release(i);
                self.pool.voices[i].release(sr, damper, key);
            }
            self.pool.voices[i].pending_release = false;
        }
    }
}

impl Synth for Grand {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.set.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(36, 32, StealPolicy::Quietest, sample_rate, GrandVoice::new);
        self.ctrl = 0;
        self.idle = 0;
        self.symp_gate = 0.0;
        self.build_banks();
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.body.clear();
        self.symp.clear();
        self.body_live = false;
        self.symp_live = false;
        self.symp_gate = 0.0;
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            TONE => self.set.tone = v,
            STIFFNESS => self.set.stiffness = v,
            STRIKE => self.set.strike = v,
            HAMMER => self.set.hammer = v,
            DYNAMICS => self.set.dynamics = v,
            MECHANICS => self.set.mechanics = v,
            DETUNE => self.set.detune = v,
            DECAY => self.set.decay = v,
            AFTERSOUND => self.set.after = v,
            PHANTOM => self.set.phantom = v,
            PEDAL => {
                let down = v >= 0.5;
                if self.pedal && !down {
                    self.pedal = false;
                    self.release_pending();
                }
                self.pedal = down;
                if down && !self.pool.is_silent() {
                    self.symp_live = true;
                }
            }
            DAMPER => self.set.damper = v,
            SYMPATHY => self.symp_amt.set(v, imm),
            WIDTH => {
                self.set.width = v;
                self.width_r.set(v, imm);
            }
            BODY => self.body_amt.set(v, imm),
            LEVEL => self.level.set(v, imm),
            _ => {}
        }
        if imm {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.pool.begin(key, id);
        self.notes = self.notes.wrapping_add(1);
        let set = self.set;
        let seed = self.notes;
        let (ratio, xgain) = (self.expr.bend_ratio(), self.expr.gain(key));
        let v = &mut self.pool.voices[slot];
        v.start(&set, key, velocity, seed);
        v.xgain = xgain;
        v.retune(ratio);
        self.body_live = true;
        self.idle = 0;
        if self.pedal {
            self.symp_live = true;
        }
    }

    fn note_off(&mut self, key: u8, _velocity: f32, id: u32) {
        if self.pedal {
            let mut found = [0usize; 4];
            let n = self.pool.alloc.find_held(id, &mut found);
            for &slot in &found[..n] {
                self.pool.voices[slot].pending_release = true;
            }
        } else {
            let (sr, damper) = (self.sr, self.set.damper);
            self.pool
                .release_id(id, |v| v.release(sr, damper, key as f32));
        }
    }

    fn all_notes_off(&mut self) {
        let (sr, damper) = (self.sr, self.set.damper);
        for i in 0..self.pool.voices.len() {
            if self.pool.alloc.slot(i).state == VoiceState::Held {
                let key = self.pool.alloc.slot(i).key as f32;
                self.pool.alloc.release(i);
                self.pool.voices[i].release(sr, damper, key);
            }
            self.pool.voices[i].pending_release = false;
        }
    }

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
        self.body.clear();
        self.symp.clear();
        self.body_live = false;
        self.symp_live = false;
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        for n in 0..left.len().min(right.len()) {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let (mut l, mut r) = (0.0, 0.0);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick();
                l += a;
                r += b;
            }
            if self.symp_live {
                let (sl, sr) = self.symp.tick((l + r) * 0.5);
                let g = self.symp_amt.get() * self.symp_gate * SYMP_GAIN;
                l += sl * g;
                r += sr * g;
            }
            if self.body_live {
                let (bl, br) = self.body.tick((l + r) * 0.5);
                // The soundboard's left and right responses only differ as
                // far as Width allows.
                let mid = (bl + br) * 0.5;
                let wd = (self.width_r.get() * 1.2).min(0.85);
                let g = self.body_amt.get() * BODY_GAIN;
                l += (mid + (bl - mid) * wd) * g;
                r += (mid + (br - mid) * wd) * g;
            }
            left[n] = l * self.out_gain;
            right[n] = r * self.out_gain;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent() && !self.body_live && !self.symp_live
    }
}

/// Wet level of the sympathetic strings at full Sympathy.
const SYMP_GAIN: f32 = 0.15;
/// Wet level of the soundboard at full Body.
const BODY_GAIN: f32 = 0.9;
