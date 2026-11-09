//! `peel.synth.bass`: a monophonic bass with a minBLEP oscillator stack, a
//! clean phase-reset sine sub, a saturating 24 dB ladder filter, a waveshaping
//! growl stage, glide and accent.
//!
//! The filter, its drive and the waveshaper run at 4x the sample rate (halfband
//! oversampling), so heavy drive and growl stay free of aliasing. The sub is
//! a plain sine that restarts at zero phase on every new note and bypasses
//! the filter and distortion (selectable), so it stays mono, clean and
//! DC-free whatever the mid layer does. The reese stack is up to three
//! detuned saws that drift slowly, with an optional slow LFO on the filter and
//! on the detune for the classic phasing movement.
//!
//! Plays last-note priority. In `Retrigger` mode every note restarts the
//! envelopes; in `Legato` overlapping notes only change pitch; `Slide` is
//! `Legato` where glide happens only between overlapping notes, the classic
//! acid behavior. Notes with high velocity are accented: a bigger, shorter
//! filter sweep and a louder level. A pitch envelope gives 808-style dives.

use peel_dsp::noise::Noise;
use peel_dsp::osc::{Lfo, LfoShape, Waveform, fast_sin};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz};
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::analog::{
    BlOsc, DcBlock, Down2, Drift, Env, LadderCoef, PitchEnv, SatLadder, Up2, advance, cents_ratio,
    ratio, shape_by, soft_ceiling,
};
use crate::common::{CTRL, Expression, Instrument, Ramp, Synth, ramp_coef, safe_bend, vel_gain};

const WAVE: ParamId = 1;
const PW: ParamId = 2;
const OSC2: ParamId = 3;
const OSC2_DETUNE: ParamId = 4;
const SUB: ParamId = 5;
const GLIDE: ParamId = 6;
const MODE: ParamId = 7;
const OSC2_VOICES: ParamId = 8;
const SUB_ROUTE: ParamId = 9;
const CUTOFF: ParamId = 10;
const RESONANCE: ParamId = 11;
const ENV_AMOUNT: ParamId = 12;
const FLT_DECAY: ParamId = 13;
const FLT_SUSTAIN: ParamId = 14;
const KEY_TRACK: ParamId = 15;
const DRIVE: ParamId = 16;
const ACCENT: ParamId = 17;
const FILTER_VEL: ParamId = 18;
const DIST_TYPE: ParamId = 19;
const AMP_A: ParamId = 20;
const AMP_D: ParamId = 21;
const AMP_S: ParamId = 22;
const AMP_R: ParamId = 23;
const GAIN: ParamId = 24;
const VELOCITY: ParamId = 25;
const ENV_CURVE: ParamId = 26;
const PITCH_ENV_AMT: ParamId = 27;
const PITCH_ENV_DECAY: ParamId = 28;
const DRIFT: ParamId = 29;
const DIST_AMOUNT: ParamId = 30;
const MOVE_RATE: ParamId = 31;
const MOVE_FILTER: ParamId = 32;
const MOVE_DETUNE: ParamId = 33;

const WAVES: &[&str] = &["Saw", "Square", "Triangle", "Sine"];
const MODES: &[&str] = &["Retrigger", "Legato", "Slide"];
const ROUTES: &[&str] = &["Clean", "Filtered"];
const DISTS: &[&str] = &["Soft", "Hard", "Fold"];

static PARAMS: [ParamDescriptor; 33] = [
    ParamDescriptor::choice(WAVE, "osc.wave", "Wave", "Oscillator", WAVES, 3),
    ParamDescriptor::linear(
        PW,
        "osc.pw",
        "Pulse Width",
        "Oscillator",
        0.05,
        0.95,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        OSC2,
        "osc2.level",
        "Osc 2 Saw",
        "Oscillator",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        OSC2_DETUNE,
        "osc2.detune",
        "Osc 2 Detune",
        "Oscillator",
        -50.0,
        50.0,
        10.0,
        Unit::Cents,
    ),
    ParamDescriptor::choice(
        OSC2_VOICES,
        "osc2.voices",
        "Osc 2 Voices",
        "Oscillator",
        &["1", "2", "3"],
        0,
    ),
    ParamDescriptor::linear(
        SUB,
        "osc.sub",
        "Sub",
        "Oscillator",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::choice(
        SUB_ROUTE,
        "osc.sub_route",
        "Sub Route",
        "Oscillator",
        ROUTES,
        0,
    ),
    ParamDescriptor::power(
        GLIDE,
        "voice.glide",
        "Glide",
        "Oscillator",
        0.0,
        1.0,
        0.0,
        Unit::Seconds,
        3.0,
    ),
    ParamDescriptor::choice(MODE, "voice.mode", "Mode", "Oscillator", MODES, 1),
    ParamDescriptor::linear(
        DRIFT,
        "analog.drift",
        "Drift",
        "Oscillator",
        0.0,
        1.0,
        0.25,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        CUTOFF,
        "filter.cutoff",
        "Cutoff",
        "Filter",
        30.0,
        16_000.0,
        900.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        RESONANCE,
        "filter.resonance",
        "Resonance",
        "Filter",
        0.0,
        1.0,
        0.05,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ENV_AMOUNT,
        "filter.env_amount",
        "Env Amount",
        "Filter",
        -6.0,
        6.0,
        0.8,
        Unit::Octaves,
    ),
    ParamDescriptor::log(
        FLT_DECAY,
        "filter.decay",
        "Env Decay",
        "Filter",
        0.03,
        4.0,
        0.5,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        FLT_SUSTAIN,
        "filter.sustain",
        "Env Sustain",
        "Filter",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        KEY_TRACK,
        "filter.key_track",
        "Key Track",
        "Filter",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        DRIVE,
        "filter.drive",
        "Drive",
        "Filter",
        0.0,
        1.0,
        0.15,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ACCENT,
        "filter.accent",
        "Accent",
        "Filter",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        FILTER_VEL,
        "filter.velocity",
        "Velocity",
        "Filter",
        0.0,
        1.0,
        0.2,
        Unit::Percent,
    ),
    ParamDescriptor::choice(DIST_TYPE, "dist.type", "Type", "Growl", DISTS, 0),
    ParamDescriptor::linear(
        DIST_AMOUNT,
        "dist.amount",
        "Amount",
        "Growl",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        MOVE_RATE,
        "move.rate",
        "Rate",
        "Movement",
        0.05,
        12.0,
        0.4,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        MOVE_FILTER,
        "move.filter",
        "Filter",
        "Movement",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        MOVE_DETUNE,
        "move.detune",
        "Detune",
        "Movement",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        AMP_A,
        "amp.attack",
        "Attack",
        "Amp",
        0.001,
        1.0,
        0.004,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        AMP_D,
        "amp.decay",
        "Decay",
        "Amp",
        0.02,
        3.0,
        0.5,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        AMP_S,
        "amp.sustain",
        "Sustain",
        "Amp",
        0.0,
        1.0,
        0.9,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        AMP_R,
        "amp.release",
        "Release",
        "Amp",
        0.01,
        3.0,
        0.15,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        ENV_CURVE,
        "env.curve",
        "Curve",
        "Amp",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        PITCH_ENV_AMT,
        "pitch.env_amount",
        "Pitch Env",
        "Amp",
        -48.0,
        48.0,
        0.0,
        Unit::Semitones,
    ),
    ParamDescriptor::log(
        PITCH_ENV_DECAY,
        "pitch.env_decay",
        "Pitch Decay",
        "Amp",
        0.005,
        2.0,
        0.1,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        GAIN,
        "out.gain",
        "Gain",
        "Amp",
        -24.0,
        6.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        VELOCITY,
        "out.velocity",
        "Velocity",
        "Amp",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.synth.bass",
    name: "Bass",
    description: "Monophonic bass: clean sub, acid, reese and growl with glide and drive",
    category: "Synth",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 15] = [
    Preset {
        name: "Sub",
        params: &[
            ("osc.wave", 3.0),
            ("osc.sub", 0.3),
            ("voice.mode", 1.0),
            ("filter.cutoff", 900.0),
            ("filter.env_amount", 0.8),
            ("amp.sustain", 0.9),
            ("amp.release", 0.15),
        ],
    },
    Preset {
        name: "Acid",
        params: &[
            ("osc.wave", 0.0),
            ("osc.sub", 0.0),
            ("voice.glide", 0.07),
            ("voice.mode", 2.0),
            ("filter.cutoff", 360.0),
            ("filter.resonance", 0.78),
            ("filter.env_amount", 3.2),
            ("filter.decay", 0.4),
            ("filter.key_track", 0.4),
            ("filter.drive", 0.45),
            ("filter.accent", 0.9),
            ("amp.attack", 0.002),
            ("amp.decay", 0.3),
            ("amp.sustain", 0.8),
            ("amp.release", 0.05),
            ("env.curve", 0.65),
            ("out.velocity", 0.8),
            ("out.gain", -2.0),
        ],
    },
    Preset {
        name: "Reese",
        params: &[
            ("osc.wave", 0.0),
            ("osc2.level", 0.9),
            ("osc2.detune", 14.0),
            ("osc2.voices", 2.0),
            ("osc.sub", 0.3),
            ("voice.mode", 1.0),
            ("voice.glide", 0.04),
            ("filter.cutoff", 650.0),
            ("filter.resonance", 0.2),
            ("filter.env_amount", 0.5),
            ("filter.decay", 0.9),
            ("filter.sustain", 0.4),
            ("filter.drive", 0.4),
            ("dist.amount", 0.25),
            ("move.rate", 0.25),
            ("move.filter", 0.3),
            ("move.detune", 0.5),
            ("amp.attack", 0.01),
            ("amp.sustain", 1.0),
            ("amp.release", 0.3),
            ("out.gain", -3.0),
        ],
    },
    Preset {
        name: "Pluck",
        params: &[
            ("osc.wave", 1.0),
            ("osc.pw", 0.35),
            ("osc.sub", 0.2),
            ("voice.mode", 0.0),
            ("filter.cutoff", 240.0),
            ("filter.resonance", 0.35),
            ("filter.env_amount", 3.5),
            ("filter.decay", 0.3),
            ("filter.drive", 0.2),
            ("amp.decay", 0.22),
            ("amp.sustain", 0.0),
            ("amp.release", 0.1),
            ("env.curve", 0.75),
        ],
    },
    Preset {
        name: "Funk",
        params: &[
            ("osc.wave", 1.0),
            ("osc2.level", 0.4),
            ("osc2.detune", 5.0),
            ("osc.sub", 0.2),
            ("voice.mode", 0.0),
            ("filter.cutoff", 480.0),
            ("filter.resonance", 0.4),
            ("filter.env_amount", 2.6),
            ("filter.decay", 0.22),
            ("filter.key_track", 0.6),
            ("filter.drive", 0.3),
            ("amp.attack", 0.002),
            ("amp.decay", 0.2),
            ("amp.sustain", 0.6),
            ("amp.release", 0.06),
            ("env.curve", 0.65),
            ("out.gain", 1.0),
        ],
    },
    Preset {
        name: "Growl",
        params: &[
            ("osc.wave", 0.0),
            ("osc2.level", 0.7),
            ("osc2.detune", 25.0),
            ("osc2.voices", 1.0),
            ("osc.sub", 0.25),
            ("voice.glide", 0.03),
            ("voice.mode", 1.0),
            ("filter.cutoff", 560.0),
            ("filter.resonance", 0.55),
            ("filter.env_amount", 2.0),
            ("filter.decay", 0.6),
            ("filter.drive", 0.85),
            ("filter.accent", 0.7),
            ("dist.type", 1.0),
            ("dist.amount", 0.4),
            ("amp.sustain", 0.9),
            ("amp.release", 0.12),
            ("out.gain", -5.5),
        ],
    },
    Preset {
        name: "808",
        params: &[
            ("osc.wave", 3.0),
            ("osc.sub", 0.0),
            ("voice.mode", 2.0),
            ("voice.glide", 0.12),
            ("filter.cutoff", 2400.0),
            ("filter.env_amount", 0.0),
            ("filter.drive", 0.35),
            ("dist.amount", 0.2),
            ("pitch.env_amount", 12.0),
            ("pitch.env_decay", 0.05),
            ("amp.decay", 1.6),
            ("amp.sustain", 0.0),
            ("amp.release", 0.3),
            ("env.curve", 0.6),
        ],
    },
    Preset {
        name: "Fingered",
        params: &[
            ("osc.wave", 2.0),
            ("osc2.level", 0.35),
            ("osc2.detune", 0.0),
            ("osc.sub", 0.25),
            ("voice.mode", 0.0),
            ("filter.cutoff", 700.0),
            ("filter.resonance", 0.1),
            ("filter.env_amount", 1.6),
            ("filter.decay", 0.25),
            ("filter.key_track", 0.5),
            ("amp.attack", 0.003),
            ("amp.decay", 0.9),
            ("amp.sustain", 0.45),
            ("amp.release", 0.12),
            ("env.curve", 0.6),
        ],
    },
    Preset {
        name: "Synthwave",
        params: &[
            ("osc.wave", 0.0),
            ("osc2.level", 0.5),
            ("osc2.detune", 8.0),
            ("osc2.voices", 1.0),
            ("osc.sub", 0.35),
            ("voice.mode", 0.0),
            ("filter.cutoff", 420.0),
            ("filter.resonance", 0.3),
            ("filter.env_amount", 2.8),
            ("filter.decay", 0.18),
            ("filter.drive", 0.3),
            ("amp.decay", 0.3),
            ("amp.sustain", 0.5),
            ("amp.release", 0.08),
            ("env.curve", 0.7),
            ("out.gain", 1.5),
        ],
    },
    Preset {
        name: "Wide Reese",
        params: &[
            ("osc.wave", 0.0),
            ("osc2.level", 1.0),
            ("osc2.detune", 22.0),
            ("osc2.voices", 2.0),
            ("osc.sub", 0.45),
            ("voice.mode", 1.0),
            ("voice.glide", 0.05),
            ("filter.cutoff", 1100.0),
            ("filter.resonance", 0.25),
            ("filter.env_amount", 0.3),
            ("filter.drive", 0.6),
            ("dist.amount", 0.3),
            ("move.rate", 0.18),
            ("move.filter", 0.25),
            ("move.detune", 0.6),
            ("amp.release", 0.2),
            ("out.gain", -4.0),
        ],
    },
    Preset {
        name: "Mid Growl",
        params: &[
            ("osc.wave", 0.0),
            ("osc2.level", 0.8),
            ("osc2.detune", 18.0),
            ("osc2.voices", 2.0),
            ("osc.sub", 0.3),
            ("voice.mode", 1.0),
            ("voice.glide", 0.03),
            ("filter.cutoff", 700.0),
            ("filter.resonance", 0.45),
            ("filter.env_amount", 2.2),
            ("filter.decay", 0.35),
            ("filter.sustain", 0.25),
            ("filter.drive", 0.5),
            ("dist.type", 2.0),
            ("dist.amount", 0.45),
            ("move.rate", 3.5),
            ("move.filter", 0.55),
            ("move.detune", 0.3),
            ("amp.sustain", 1.0),
            ("amp.release", 0.1),
            ("out.gain", -5.5),
        ],
    },
    Preset {
        name: "Deep House",
        params: &[
            ("osc.wave", 2.0),
            ("osc2.level", 0.0),
            ("osc.sub", 0.5),
            ("voice.mode", 0.0),
            ("filter.cutoff", 520.0),
            ("filter.resonance", 0.12),
            ("filter.env_amount", 1.3),
            ("filter.decay", 0.25),
            ("filter.key_track", 0.5),
            ("filter.drive", 0.1),
            ("amp.attack", 0.003),
            ("amp.decay", 0.35),
            ("amp.sustain", 0.55),
            ("amp.release", 0.1),
            ("env.curve", 0.65),
        ],
    },
    Preset {
        name: "Pluck Bass",
        params: &[
            ("osc.wave", 0.0),
            ("osc2.level", 0.4),
            ("osc2.detune", 7.0),
            ("osc.sub", 0.35),
            ("voice.mode", 0.0),
            ("filter.cutoff", 300.0),
            ("filter.resonance", 0.3),
            ("filter.env_amount", 3.0),
            ("filter.decay", 0.22),
            ("filter.drive", 0.3),
            ("amp.attack", 0.002),
            ("amp.decay", 0.3),
            ("amp.sustain", 0.0),
            ("amp.release", 0.08),
            ("env.curve", 0.85),
            ("out.gain", 2.5),
        ],
    },
    Preset {
        name: "Sub Drop",
        params: &[
            ("osc.wave", 3.0),
            ("osc.sub", 0.0),
            ("voice.mode", 0.0),
            ("filter.cutoff", 1800.0),
            ("filter.env_amount", 0.0),
            ("filter.drive", 0.25),
            ("pitch.env_amount", 24.0),
            ("pitch.env_decay", 0.28),
            ("amp.attack", 0.002),
            ("amp.decay", 2.2),
            ("amp.sustain", 0.0),
            ("amp.release", 0.4),
            ("env.curve", 0.55),
        ],
    },
    Preset {
        name: "Rolling",
        params: &[
            ("osc.wave", 0.0),
            ("osc2.level", 0.45),
            ("osc2.detune", 9.0),
            ("osc.sub", 0.4),
            ("voice.mode", 0.0),
            ("filter.cutoff", 380.0),
            ("filter.resonance", 0.25),
            ("filter.env_amount", 1.9),
            ("filter.decay", 0.16),
            ("filter.sustain", 0.1),
            ("filter.drive", 0.3),
            ("amp.attack", 0.003),
            ("amp.decay", 0.2),
            ("amp.sustain", 0.7),
            ("amp.release", 0.07),
            ("env.curve", 0.7),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Bass::new()))
}

const STACK: usize = 16;
const MAX_O2: usize = 3;
/// Detune of each reese saw as a multiple of the detune parameter.
const O2_PATTERN: [[f32; MAX_O2]; MAX_O2] = [[1.0, 0.0, 0.0], [1.0, -1.0, 0.0], [1.0, -1.0, 0.38]];
/// Oversampling factor of the filter and distortion.
const OS: f32 = 4.0;

#[derive(Clone, Copy)]
struct Held {
    id: u32,
    key: u8,
}

pub(crate) struct Bass {
    expr: Expression,
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    notes: u32,
    // Voice.
    active: bool,
    gate: bool,
    cur_key: f32,
    target_key: f32,
    glide_on: bool,
    osc1: BlOsc,
    osc2: [BlOsc; MAX_O2],
    sub_ph: f32,
    dt1: f32,
    dt2: [f32; MAX_O2],
    dt_sub: f32,
    ladder: SatLadder,
    up_a: Up2,
    up_b: Up2,
    down_a: Down2,
    down_b: Down2,
    dc: DcBlock,
    amp_env: Env,
    flt_env: Env,
    pitch_env: PitchEnv,
    drift_o1: Drift,
    drift_o2: [Drift; MAX_O2],
    drift_cut: Drift,
    lfo: Lfo,
    lfo_value: f32,
    velocity: f32,
    accent_amt: f32,
    vel_level: f32,
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
    stack: [Held; STACK],
    stack_len: usize,
    /// Runtime voice ID of the note on top of the stack; its per-note
    /// expression drives the single voice.
    voice_id: u32,
    last_key: Option<f32>,
    // Unsmoothed settings.
    wave: Waveform,
    mode: usize,
    o2_voices: usize,
    sub_clean: bool,
    dist_kind: usize,
    amp_adsr: [f32; 4],
    flt_decay: f32,
    flt_sustain: f32,
    curve: f32,
    pitch_env_decay: f32,
    move_rate: f32,
    vel_sens: f32,
    // Smoothed settings.
    pw: Ramp,
    osc2_level: Ramp,
    detune2: Ramp,
    sub_level: Ramp,
    glide: Ramp,
    cutoff: Ramp,
    res: Ramp,
    env_amt: Ramp,
    key_track: Ramp,
    drive: Ramp,
    accent: Ramp,
    filter_vel: Ramp,
    dist_amount: Ramp,
    move_filter: Ramp,
    move_detune: Ramp,
    pitch_env_amt: Ramp,
    drift: Ramp,
    gain: Ramp,
    // Derived.
    glide_k: f32,
    out_gain: f32,
}

impl Bass {
    pub(crate) fn new() -> Bass {
        Bass {
            expr: Expression::new(),
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            notes: 0,
            active: false,
            gate: false,
            cur_key: 36.0,
            target_key: 36.0,
            glide_on: false,
            osc1: BlOsc::new(0.0),
            osc2: [BlOsc::new(0.0); MAX_O2],
            sub_ph: 0.0,
            dt1: 0.0,
            dt2: [0.0; MAX_O2],
            dt_sub: 0.0,
            ladder: SatLadder::new(),
            up_a: Up2::new(),
            up_b: Up2::new(),
            down_a: Down2::new(),
            down_b: Down2::new(),
            dc: DcBlock::new(48_000.0),
            amp_env: Env::new(48_000.0),
            flt_env: Env::new(48_000.0),
            pitch_env: PitchEnv::new(),
            drift_o1: Drift::new(101),
            drift_o2: std::array::from_fn(|i| Drift::new(110 + i as u32)),
            drift_cut: Drift::new(120),
            lfo: Lfo::new(0x2B),
            lfo_value: 0.0,
            velocity: 0.0,
            accent_amt: 0.0,
            xgain: 1.0,
            vel_level: 1.0,
            stack: [Held { id: 0, key: 0 }; STACK],
            stack_len: 0,
            voice_id: 0,
            last_key: None,
            wave: Waveform::Sine,
            mode: 1,
            o2_voices: 1,
            sub_clean: true,
            dist_kind: 0,
            amp_adsr: [0.004, 0.5, 0.9, 0.15],
            flt_decay: 0.5,
            flt_sustain: 0.0,
            curve: 0.5,
            pitch_env_decay: 0.1,
            move_rate: 0.4,
            vel_sens: 0.5,
            pw: Ramp::new(0.5),
            osc2_level: Ramp::new(0.0),
            detune2: Ramp::new(10.0),
            sub_level: Ramp::new(0.3),
            glide: Ramp::new(0.0),
            cutoff: Ramp::new(900.0f32.ln()),
            res: Ramp::new(0.05),
            env_amt: Ramp::new(0.8),
            key_track: Ramp::new(0.3),
            drive: Ramp::new(0.15),
            accent: Ramp::new(0.4),
            filter_vel: Ramp::new(0.2),
            dist_amount: Ramp::new(0.0),
            move_filter: Ramp::new(0.0),
            move_detune: Ramp::new(0.0),
            pitch_env_amt: Ramp::new(0.0),
            drift: Ramp::new(0.25),
            gain: Ramp::new(0.0),
            glide_k: 1.0,
            out_gain: 1.0,
        }
    }

    fn apply_envelopes(&mut self) {
        let a = self.amp_adsr;
        self.amp_env.set(a[0], a[1], a[2], a[3], self.curve);
        // The filter envelope shortens with accent; the decay used here is
        // refreshed per note, but keep the idle value sensible too.
        self.flt_env
            .set(0.001, self.flt_decay, self.flt_sustain, a[3], self.curve);
        self.pitch_env.set_decay(self.pitch_env_decay, self.sr);
    }

    fn derive(&mut self) {
        let glide = self.glide.get();
        self.glide_k = if glide <= 0.001 {
            1.0
        } else {
            1.0 - (-3.0 * CTRL as f32 / (glide * self.sr)).exp()
        };
        self.out_gain = db_to_gain(self.gain.get());
    }

    /// Pitch, filter and glide update, once per control interval.
    fn control(&mut self) {
        let k = self.ramp_k;
        self.pw.tick(k);
        self.osc2_level.tick(k);
        self.detune2.tick(k);
        self.sub_level.tick(k);
        self.glide.tick(k);
        self.cutoff.tick(k);
        self.res.tick(k);
        self.env_amt.tick(k);
        self.key_track.tick(k);
        self.drive.tick(k);
        self.accent.tick(k);
        self.filter_vel.tick(k);
        self.dist_amount.tick(k);
        self.move_filter.tick(k);
        self.move_detune.tick(k);
        self.pitch_env_amt.tick(k);
        self.drift.tick(k);
        self.gain.tick(k);
        self.derive();
        self.expr.tick();
        let dt = self.move_rate / self.sr * CTRL as f32;
        self.lfo_value = self.lfo.tick(LfoShape::Sine, dt);
        if self.active {
            self.update_voice();
        }
    }

    fn update_voice(&mut self) {
        if self.glide_on {
            self.cur_key += (self.target_key - self.cur_key) * self.glide_k;
        } else {
            self.cur_key = self.target_key;
        }
        let dr = self.drift.get();
        let slew = 1.0 - (-(CTRL as f32) / (0.8 * self.sr)).exp();
        let hold = (1.3 * self.sr / CTRL as f32) as u32;
        let pe = self.pitch_env.level() * self.pitch_env_amt.get();
        let base = self.cur_key + pe;
        let key = self.target_key.round().clamp(0.0, 127.0) as u8;
        let x = self.expr.voice(self.voice_id, key);
        let base = base + safe_bend(base + 1.0, x.bend, 0.45 * self.sr);
        self.xgain = x.gain();
        let inv_sr = 1.0 / self.sr;
        let hz = key_to_hz(base);
        self.dt1 = hz * cents_ratio(self.drift_o1.tick(slew, hold) * 3.0 * dr) * inv_sr;
        // Slow movement: the LFO swings the reese detune.
        let d2 = self.detune2.get() * (1.0 + 0.8 * self.move_detune.get() * self.lfo_value);
        let pat = O2_PATTERN[self.o2_voices.clamp(1, MAX_O2) - 1];
        for (i, &p) in pat.iter().enumerate() {
            let det = ratio(d2 * p * 0.01);
            self.dt2[i] =
                hz * det * cents_ratio(self.drift_o2[i].tick(slew, hold) * 4.0 * dr) * inv_sr;
        }
        self.dt_sub = hz * 0.5 * inv_sr;
        let acc = self.accent_amt;
        let env_amt = self.env_amt.get() * (1.0 + 0.6 * acc) + acc * 0.8;
        let oct = env_amt * self.flt_env.level()
            + self.key_track.get() * (self.cur_key - 48.0) / 12.0
            + self.filter_vel.get() * (self.velocity - 0.6) * 3.0
            + self.move_filter.get() * self.lfo_value * 1.5
            + self.drift_cut.tick(slew, hold) * 0.08 * dr
            + x.cutoff_oct();
        let cutoff =
            (self.cutoff.get().exp() * oct.clamp(-12.0, 12.0).exp2()).clamp(20.0, 0.45 * self.sr);
        let drive = self.drive.get();
        let drive_g = 1.0 + 7.0 * drive * drive + 0.8 * acc;
        let res = (self.res.get() + 0.12 * acc).min(1.0);
        let c = LadderCoef::new(cutoff, res, drive_g, self.sr * OS);
        self.ladder.glide_to(c, CTRL as f32 * OS);
    }

    fn start_note(&mut self, key: u8, velocity: f32, retrigger: bool) {
        self.velocity = velocity;
        self.accent_amt = self.accent.get() * ((velocity - 0.7) / 0.25).clamp(0.0, 1.0);
        self.vel_level = vel_gain(velocity, self.vel_sens) * (1.0 + 0.5 * self.accent_amt);
        self.target_key = key as f32;
        let fresh = !self.active || self.amp_env.level() < 1.0e-3;
        if retrigger {
            let decay = self.flt_decay * (1.0 - 0.5 * self.accent_amt);
            let a = self.amp_adsr;
            // A note that begins from silence starts every oscillator at a
            // known phase: the sine sub at its zero crossing, so there is
            // never a click and every note hits the same way.
            if fresh {
                self.notes = self.notes.wrapping_add(1);
                let mut rng = Noise::new(self.notes ^ 0x5EED);
                self.osc1.reset(0.0);
                for o in self.osc2.iter_mut() {
                    o.reset(rng.uniform());
                }
                self.sub_ph = 0.0;
                self.ladder.reset();
                self.up_a.reset();
                self.up_b.reset();
                self.down_a.reset();
                self.down_b.reset();
                self.drift_o1.reseed(self.notes.wrapping_mul(17));
                self.drift_cut.reseed(self.notes.wrapping_mul(19));
                for (i, d) in self.drift_o2.iter_mut().enumerate() {
                    d.reseed(self.notes.wrapping_mul(23).wrapping_add(i as u32 * 313));
                }
            }
            self.amp_env.set(a[0], a[1], a[2], a[3], self.curve);
            self.flt_env
                .set(0.001, decay, self.flt_sustain, a[3], self.curve);
            self.amp_env.note_on();
            self.flt_env.note_on();
            self.pitch_env.trigger();
        }
        self.gate = true;
        if !self.active {
            self.active = true;
        }
        self.update_voice();
        if retrigger && fresh {
            self.ladder.settle();
        }
    }

    /// The oversampled filter and growl stage: one input sample in, one out.
    #[inline]
    fn filter_stage(&mut self, x: f32, dist_gain: f32) -> f32 {
        let kind = self.dist_kind;
        let (a, b) = self.up_a.tick(x);
        let (a0, a1) = self.up_b.tick(a);
        let (b0, b1) = self.up_b.tick(b);
        let mut y = [a0, a1, b0, b1];
        for s in y.iter_mut() {
            let f = self.ladder.tick(*s);
            *s = if dist_gain > 1.0 {
                shape_by(kind, f, dist_gain)
            } else {
                f
            };
        }
        let d0 = self.down_b.tick(y[0], y[1]);
        let d1 = self.down_b.tick(y[2], y[3]);
        self.down_a.tick(d0, d1)
    }
}

impl Synth for Bass {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.amp_env = Env::new(sample_rate);
        self.flt_env = Env::new(sample_rate);
        self.dc = DcBlock::new(sample_rate);
        self.ctrl = 0;
        self.apply_envelopes();
    }

    fn reset(&mut self) {
        self.active = false;
        self.gate = false;
        self.stack_len = 0;
        self.last_key = None;
        self.amp_env.reset();
        self.flt_env.reset();
        self.pitch_env.reset();
        self.ladder.reset();
        self.up_a.reset();
        self.up_b.reset();
        self.down_a.reset();
        self.down_b.reset();
        self.dc.reset();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            WAVE => {
                self.wave = match v as usize {
                    0 => Waveform::Saw,
                    1 => Waveform::Square,
                    2 => Waveform::Triangle,
                    _ => Waveform::Sine,
                }
            }
            PW => self.pw.set(v, imm),
            OSC2 => self.osc2_level.set(v, imm),
            OSC2_DETUNE => self.detune2.set(v, imm),
            OSC2_VOICES => self.o2_voices = (v as usize + 1).clamp(1, MAX_O2),
            SUB => self.sub_level.set(v, imm),
            SUB_ROUTE => self.sub_clean = v < 0.5,
            GLIDE => self.glide.set(v, imm),
            MODE => self.mode = v as usize,
            DRIFT => self.drift.set(v, imm),
            CUTOFF => self.cutoff.set(v.ln(), imm),
            RESONANCE => self.res.set(v, imm),
            ENV_AMOUNT => self.env_amt.set(v, imm),
            FLT_DECAY => {
                self.flt_decay = v;
                self.apply_envelopes();
            }
            FLT_SUSTAIN => {
                self.flt_sustain = v;
                self.apply_envelopes();
            }
            KEY_TRACK => self.key_track.set(v, imm),
            DRIVE => self.drive.set(v, imm),
            ACCENT => self.accent.set(v, imm),
            FILTER_VEL => self.filter_vel.set(v, imm),
            DIST_TYPE => self.dist_kind = (v as usize).min(2),
            DIST_AMOUNT => self.dist_amount.set(v, imm),
            MOVE_RATE => self.move_rate = v,
            MOVE_FILTER => self.move_filter.set(v, imm),
            MOVE_DETUNE => self.move_detune.set(v, imm),
            AMP_A => {
                self.amp_adsr[0] = v;
                self.apply_envelopes();
            }
            AMP_D => {
                self.amp_adsr[1] = v;
                self.apply_envelopes();
            }
            AMP_S => {
                self.amp_adsr[2] = v;
                self.apply_envelopes();
            }
            AMP_R => {
                self.amp_adsr[3] = v;
                self.apply_envelopes();
            }
            ENV_CURVE => {
                self.curve = v;
                self.apply_envelopes();
            }
            PITCH_ENV_AMT => self.pitch_env_amt.set(v, imm),
            PITCH_ENV_DECAY => {
                self.pitch_env_decay = v;
                self.apply_envelopes();
            }
            GAIN => self.gain.set(v, imm),
            VELOCITY => self.vel_sens = v,
            _ => {}
        }
        if imm {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let overlapping = self.stack_len > 0;
        if self.stack_len == STACK {
            self.stack.copy_within(1.., 0);
            self.stack_len -= 1;
        }
        self.stack[self.stack_len] = Held { id, key };
        self.stack_len += 1;
        self.voice_id = id;

        let legato = self.mode != 0 && overlapping;
        // Glide: always in Retrigger/Legato; only between overlapping notes
        // in Slide.
        self.glide_on = match self.mode {
            2 => overlapping && self.glide_k < 1.0,
            _ => self.glide_k < 1.0 && self.last_key.is_some(),
        };
        if !self.active && self.glide_on {
            self.cur_key = self.last_key.unwrap_or(key as f32);
        }
        self.last_key = Some(key as f32);
        self.start_note(key, velocity, !legato);
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        let Some(pos) = self.stack[..self.stack_len].iter().position(|h| h.id == id) else {
            return;
        };
        let was_top = pos + 1 == self.stack_len;
        self.stack.copy_within(pos + 1..self.stack_len, pos);
        self.stack_len -= 1;
        if self.stack_len == 0 {
            self.gate = false;
            self.amp_env.note_off();
            self.flt_env.note_off();
        } else if was_top {
            // Fall back to the previous held note without retriggering.
            let back = self.stack[self.stack_len - 1];
            self.glide_on = self.glide_k < 1.0;
            self.target_key = back.key as f32;
            self.last_key = Some(back.key as f32);
            self.voice_id = back.id;
        }
    }

    fn all_notes_off(&mut self) {
        self.stack_len = 0;
        self.gate = false;
        self.amp_env.note_off();
        self.flt_env.note_off();
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
            if !self.active {
                left[n] = 0.0;
                right[n] = 0.0;
                continue;
            }
            let amp = self.amp_env.tick();
            self.flt_env.tick();
            if self.pitch_env_amt.get() != 0.0 {
                self.pitch_env.tick();
            }
            let pw = self.pw.get();
            let mut s = self.osc1.tick(self.wave, self.dt1, pw);
            let o2 = self.osc2_level.get();
            if o2 > 1.0e-4 {
                let nv = self.o2_voices;
                let mut sum = 0.0;
                for i in 0..nv {
                    sum += self.osc2[i].tick(Waveform::Saw, self.dt2[i], 0.5);
                }
                let osc2 = sum / (nv as f32).sqrt();
                s = s * (1.0 - 0.4 * o2) + osc2 * o2 * 0.6;
            }
            let sub = fast_sin(self.sub_ph) * self.sub_level.get() * 0.8;
            advance(&mut self.sub_ph, self.dt_sub);
            let amount = self.dist_amount.get();
            let dist_gain = if amount > 0.001 {
                1.0 + 11.0 * amount * amount
            } else {
                1.0
            };
            let mid = if self.sub_clean { s } else { s + sub };
            let mut y = self.filter_stage(mid * 0.7, dist_gain);
            if self.sub_clean {
                y += sub * 0.7;
            }
            let out = soft_ceiling(
                self.dc.tick(y) * 0.75 * amp * self.vel_level * self.out_gain * self.xgain,
            );
            left[n] = out;
            right[n] = out;
            if self.amp_env.is_idle() {
                self.active = false;
            }
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        !self.active
    }
}
