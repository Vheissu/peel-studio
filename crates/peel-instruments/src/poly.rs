//! `peel.synth.poly`: a 16-voice subtractive synthesizer.
//!
//! Two band-limited oscillators (4-point PolyBLEP saw/pulse), a sine sub and
//! noise feed up to nine detuned, stereo-spread unison copies per note. The
//! `Super` unison mode follows Adam Szabo's analysis of the Roland JP-8000
//! supersaw ("How to Emulate the Super Saw", 2010): the measured detune
//! curve and side/center level curves, free-running random phases and a
//! pitch-tracking high-pass on the stack. Then a zero-delay-feedback filter
//! (12 dB state-variable or 24 dB ladder, both with soft saturation and an
//! optional 2x-oversampled drive stage). Curved ADSR envelopes drive the amp
//! and filter, a pitch envelope handles dives and zaps, and one LFO (free or
//! tempo synced) modulates pitch, filter, amplitude, pulse width or unison
//! detune. Each voice drifts slightly in pitch and cutoff like analog
//! hardware. Voices can also play as a mono/legato lead with glide.

use peel_dsp::envelope::Stage;
use peel_dsp::noise::{Noise, Pink};
use peel_dsp::osc::{Lfo, LfoShape, Waveform, fast_sin};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz, pan_gains};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::analog::{
    BlOsc, DcBlock, Down2, Drift, Env, LadderCoef, PitchEnv, SatLadder, SatSvf, SvfCoef, Up2,
    advance, cents_ratio, ratio, soft_ceiling,
};
use crate::common::{
    CTRL, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, ramp_coef, vel_gain,
};

const OSC1_WAVE: ParamId = 1;
const OSC2_WAVE: ParamId = 2;
const OSC2_SEMI: ParamId = 3;
const OSC2_DETUNE: ParamId = 4;
const OSC_MIX: ParamId = 5;
const OSC_PW: ParamId = 6;
const OSC_SUB: ParamId = 7;
const OSC_NOISE: ParamId = 8;
const UNISON: ParamId = 9;
const UNISON_DETUNE: ParamId = 10;
const UNISON_SPREAD: ParamId = 11;
const GLIDE: ParamId = 12;
const VOICE_MODE: ParamId = 13;
const UNISON_TYPE: ParamId = 14;
const UNISON_BLEND: ParamId = 15;
const UNISON_LOWCUT: ParamId = 16;
const NOISE_COLOR: ParamId = 17;
const NOISE_DECAY: ParamId = 18;
const FILTER_TYPE: ParamId = 20;
const CUTOFF: ParamId = 21;
const RESONANCE: ParamId = 22;
const ENV_AMOUNT: ParamId = 23;
const KEY_TRACK: ParamId = 24;
const FILTER_VEL: ParamId = 25;
const FILTER_DRIVE: ParamId = 26;
const AMP_A: ParamId = 30;
const AMP_D: ParamId = 31;
const AMP_S: ParamId = 32;
const AMP_R: ParamId = 33;
const FLT_A: ParamId = 34;
const FLT_D: ParamId = 35;
const FLT_S: ParamId = 36;
const FLT_R: ParamId = 37;
const ENV_CURVE: ParamId = 38;
const PITCH_ENV_AMT: ParamId = 39;
const LFO_RATE: ParamId = 40;
const LFO_SYNC: ParamId = 41;
const LFO_SHAPE: ParamId = 42;
const LFO_DEPTH: ParamId = 43;
const LFO_TARGET: ParamId = 44;
const PITCH_ENV_DECAY: ParamId = 45;
const GAIN: ParamId = 50;
const VEL_AMP: ParamId = 51;
const DRIFT: ParamId = 52;

const WAVES: &[&str] = &["Sine", "Triangle", "Saw", "Square"];
const FILTERS: &[&str] = &["LP 12", "LP 24", "Band Pass", "High Pass", "Notch"];
const SYNCS: &[&str] = &[
    "Free", "2 Bars", "1 Bar", "1/2", "1/4", "1/8", "1/8T", "1/16", "1/16T",
];
/// Quarter notes per LFO cycle for each sync choice (0 means free-running).
const SYNC_BEATS: [f64; 9] = [0.0, 8.0, 4.0, 2.0, 1.0, 0.5, 1.0 / 3.0, 0.25, 1.0 / 6.0];
const SHAPES: &[&str] = &["Sine", "Triangle", "Saw", "Square", "Random"];
const TARGETS: &[&str] = &["Pitch", "Filter", "Amp", "Pulse Width", "Detune"];
const MODES: &[&str] = &["Poly", "Mono", "Legato"];
const UNISON_TYPES: &[&str] = &["Classic", "Super"];
const NOISE_COLORS: &[&str] = &["White", "Pink"];

static PARAMS: [ParamDescriptor; 44] = [
    ParamDescriptor::choice(OSC1_WAVE, "osc1.wave", "Osc 1", "Oscillators", WAVES, 2),
    ParamDescriptor::choice(OSC2_WAVE, "osc2.wave", "Osc 2", "Oscillators", WAVES, 2),
    ParamDescriptor::linear(
        OSC2_SEMI,
        "osc2.semi",
        "Osc 2 Pitch",
        "Oscillators",
        -24.0,
        24.0,
        0.0,
        Unit::Semitones,
    ),
    ParamDescriptor::linear(
        OSC2_DETUNE,
        "osc2.detune",
        "Osc 2 Detune",
        "Oscillators",
        -100.0,
        100.0,
        9.0,
        Unit::Cents,
    ),
    ParamDescriptor::linear(
        OSC_MIX,
        "osc.mix",
        "Osc Mix",
        "Oscillators",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        OSC_PW,
        "osc.pw",
        "Pulse Width",
        "Oscillators",
        0.05,
        0.95,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        OSC_SUB,
        "osc.sub",
        "Sub",
        "Oscillators",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        OSC_NOISE,
        "osc.noise",
        "Noise",
        "Oscillators",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::choice(
        NOISE_COLOR,
        "osc.noise_color",
        "Noise Color",
        "Oscillators",
        NOISE_COLORS,
        0,
    ),
    ParamDescriptor::power(
        NOISE_DECAY,
        "osc.noise_decay",
        "Noise Decay",
        "Oscillators",
        0.0,
        4.0,
        0.0,
        Unit::Seconds,
        3.0,
    ),
    ParamDescriptor::choice(
        UNISON,
        "unison.voices",
        "Oscillators",
        "Oscillators",
        &["1", "2", "3", "4", "5", "6", "7", "8", "9"],
        2,
    ),
    ParamDescriptor::choice(
        UNISON_TYPE,
        "unison.type",
        "Type",
        "Oscillators",
        UNISON_TYPES,
        0,
    ),
    ParamDescriptor::linear(
        UNISON_DETUNE,
        "unison.detune",
        "Unison Detune",
        "Oscillators",
        0.0,
        100.0,
        14.0,
        Unit::Cents,
    ),
    ParamDescriptor::linear(
        UNISON_BLEND,
        "unison.blend",
        "Blend",
        "Oscillators",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        UNISON_SPREAD,
        "unison.spread",
        "Stereo Spread",
        "Oscillators",
        0.0,
        1.0,
        0.8,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        UNISON_LOWCUT,
        "unison.lowcut",
        "Low Cut",
        "Oscillators",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::choice(VOICE_MODE, "voice.mode", "Mode", "Oscillators", MODES, 0),
    ParamDescriptor::power(
        GLIDE,
        "voice.glide",
        "Glide",
        "Oscillators",
        0.0,
        2.0,
        0.0,
        Unit::Seconds,
        3.0,
    ),
    ParamDescriptor::linear(
        DRIFT,
        "analog.drift",
        "Drift",
        "Oscillators",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::choice(FILTER_TYPE, "filter.type", "Type", "Filter", FILTERS, 1),
    ParamDescriptor::log(
        CUTOFF,
        "filter.cutoff",
        "Cutoff",
        "Filter",
        20.0,
        20_000.0,
        1_800.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        RESONANCE,
        "filter.resonance",
        "Resonance",
        "Filter",
        0.0,
        1.0,
        0.15,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        FILTER_DRIVE,
        "filter.drive",
        "Drive",
        "Filter",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ENV_AMOUNT,
        "filter.env_amount",
        "Env Amount",
        "Filter",
        -6.0,
        6.0,
        1.5,
        Unit::Octaves,
    ),
    ParamDescriptor::linear(
        KEY_TRACK,
        "filter.key_track",
        "Key Track",
        "Filter",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        FILTER_VEL,
        "filter.velocity",
        "Velocity",
        "Filter",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        AMP_A,
        "amp.attack",
        "Amp Attack",
        "Envelopes",
        0.001,
        8.0,
        0.06,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        AMP_D,
        "amp.decay",
        "Amp Decay",
        "Envelopes",
        0.005,
        8.0,
        0.5,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        AMP_S,
        "amp.sustain",
        "Amp Sustain",
        "Envelopes",
        0.0,
        1.0,
        0.8,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        AMP_R,
        "amp.release",
        "Amp Release",
        "Envelopes",
        0.005,
        10.0,
        0.7,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        FLT_A,
        "filter.attack",
        "Filter Attack",
        "Envelopes",
        0.001,
        8.0,
        0.04,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        FLT_D,
        "filter.decay",
        "Filter Decay",
        "Envelopes",
        0.005,
        8.0,
        0.8,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        FLT_S,
        "filter.sustain",
        "Filter Sustain",
        "Envelopes",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        FLT_R,
        "filter.release",
        "Filter Release",
        "Envelopes",
        0.005,
        10.0,
        0.6,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        ENV_CURVE,
        "env.curve",
        "Curve",
        "Envelopes",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        PITCH_ENV_AMT,
        "pitch.env_amount",
        "Pitch Env",
        "Envelopes",
        -48.0,
        48.0,
        0.0,
        Unit::Semitones,
    ),
    ParamDescriptor::log(
        PITCH_ENV_DECAY,
        "pitch.env_decay",
        "Pitch Decay",
        "Envelopes",
        0.005,
        2.0,
        0.15,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        LFO_RATE,
        "lfo.rate",
        "Rate",
        "Modulation",
        0.05,
        30.0,
        4.5,
        Unit::Hertz,
    ),
    ParamDescriptor::choice(LFO_SYNC, "lfo.sync", "Sync", "Modulation", SYNCS, 0),
    ParamDescriptor::choice(LFO_SHAPE, "lfo.shape", "Shape", "Modulation", SHAPES, 0),
    ParamDescriptor::linear(
        LFO_DEPTH,
        "lfo.depth",
        "Depth",
        "Modulation",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::choice(LFO_TARGET, "lfo.target", "Target", "Modulation", TARGETS, 0),
    ParamDescriptor::linear(
        GAIN,
        "out.gain",
        "Gain",
        "Output",
        -24.0,
        6.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        VEL_AMP,
        "out.velocity",
        "Velocity",
        "Output",
        0.0,
        1.0,
        0.6,
        Unit::Percent,
    ),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.synth.poly",
    name: "Poly",
    description: "Polyphonic subtractive synth with supersaw unison, filter and LFO",
    category: "Synth",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 19] = [
    Preset {
        name: "Warm Pad",
        params: &[
            ("osc1.wave", 2.0),
            ("osc2.wave", 2.0),
            ("osc2.semi", 0.0),
            ("osc2.detune", 9.0),
            ("osc.mix", 0.5),
            ("unison.voices", 2.0),
            ("unison.type", 0.0),
            ("unison.detune", 14.0),
            ("unison.spread", 0.8),
            ("filter.type", 1.0),
            ("filter.cutoff", 1_800.0),
            ("filter.resonance", 0.15),
            ("filter.env_amount", 1.5),
            ("amp.attack", 0.06),
            ("amp.sustain", 0.8),
            ("amp.release", 0.7),
            ("lfo.depth", 0.0),
            ("out.gain", 0.0),
        ],
    },
    Preset {
        name: "Pluck",
        params: &[
            ("osc2.wave", 3.0),
            ("osc2.semi", 12.0),
            ("osc2.detune", 4.0),
            ("osc.mix", 0.3),
            ("osc.noise", 0.12),
            ("osc.noise_color", 1.0),
            ("osc.noise_decay", 0.05),
            ("unison.voices", 1.0),
            ("unison.detune", 9.0),
            ("unison.spread", 0.6),
            ("filter.cutoff", 700.0),
            ("filter.resonance", 0.22),
            ("filter.env_amount", 4.0),
            ("filter.key_track", 0.6),
            ("filter.velocity", 0.6),
            ("amp.attack", 0.001),
            ("amp.decay", 0.45),
            ("amp.sustain", 0.0),
            ("amp.release", 0.2),
            ("filter.attack", 0.001),
            ("filter.decay", 0.22),
            ("filter.sustain", 0.0),
            ("filter.release", 0.2),
            ("env.curve", 0.8),
            ("out.gain", 2.0),
        ],
    },
    Preset {
        name: "Supersaw Lead",
        params: &[
            ("osc2.semi", -12.0),
            ("osc2.detune", 6.0),
            ("osc.mix", 0.25),
            ("unison.voices", 6.0),
            ("unison.type", 1.0),
            ("unison.detune", 48.0),
            ("unison.blend", 0.75),
            ("unison.spread", 0.9),
            ("unison.lowcut", 0.9),
            ("voice.glide", 0.05),
            ("filter.type", 1.0),
            ("filter.cutoff", 7_000.0),
            ("filter.resonance", 0.1),
            ("filter.env_amount", 1.2),
            ("filter.key_track", 0.8),
            ("amp.attack", 0.005),
            ("amp.decay", 0.3),
            ("amp.sustain", 0.85),
            ("amp.release", 0.3),
            ("filter.attack", 0.005),
            ("filter.decay", 0.5),
            ("filter.sustain", 0.6),
            ("filter.release", 0.3),
            ("lfo.rate", 5.5),
            ("lfo.depth", 0.1),
            ("lfo.target", 0.0),
            ("out.gain", -5.0),
        ],
    },
    Preset {
        name: "Brass",
        params: &[
            ("osc2.detune", 5.0),
            ("unison.voices", 1.0),
            ("unison.detune", 8.0),
            ("unison.spread", 0.4),
            ("filter.cutoff", 560.0),
            ("filter.resonance", 0.15),
            ("filter.env_amount", 3.3),
            ("filter.velocity", 0.5),
            ("amp.attack", 0.035),
            ("amp.decay", 0.25),
            ("amp.sustain", 0.85),
            ("amp.release", 0.12),
            ("filter.attack", 0.07),
            ("filter.decay", 0.4),
            ("filter.sustain", 0.45),
            ("filter.release", 0.15),
            ("analog.drift", 0.45),
        ],
    },
    Preset {
        name: "Strings",
        params: &[
            ("osc2.detune", 11.0),
            ("unison.voices", 5.0),
            ("unison.detune", 20.0),
            ("unison.spread", 1.0),
            ("filter.type", 0.0),
            ("filter.cutoff", 3_000.0),
            ("filter.resonance", 0.05),
            ("filter.env_amount", 0.5),
            ("amp.attack", 0.35),
            ("amp.decay", 0.6),
            ("amp.sustain", 0.9),
            ("amp.release", 0.9),
            ("filter.attack", 0.4),
            ("filter.decay", 0.8),
            ("filter.sustain", 0.7),
            ("filter.release", 0.9),
            ("lfo.rate", 0.35),
            ("lfo.depth", 0.4),
            ("lfo.target", 4.0),
            ("analog.drift", 0.5),
            ("out.gain", -2.0),
        ],
    },
    Preset {
        name: "Wobble",
        params: &[
            ("osc1.wave", 2.0),
            ("osc2.wave", 3.0),
            ("osc2.semi", -12.0),
            ("osc2.detune", 6.0),
            ("osc.mix", 0.5),
            ("osc.sub", 0.4),
            ("unison.voices", 1.0),
            ("unison.detune", 10.0),
            ("filter.cutoff", 420.0),
            ("filter.resonance", 0.55),
            ("filter.drive", 0.3),
            ("filter.env_amount", 0.0),
            ("amp.attack", 0.005),
            ("amp.decay", 0.3),
            ("amp.sustain", 1.0),
            ("amp.release", 0.15),
            ("lfo.sync", 5.0),
            ("lfo.shape", 1.0),
            ("lfo.depth", 0.75),
            ("lfo.target", 1.0),
            ("out.gain", -2.5),
        ],
    },
    Preset {
        name: "Soft Keys",
        params: &[
            ("osc1.wave", 1.0),
            ("osc2.wave", 0.0),
            ("osc2.semi", 12.0),
            ("osc2.detune", 3.0),
            ("osc.mix", 0.35),
            ("unison.voices", 1.0),
            ("unison.detune", 6.0),
            ("filter.type", 0.0),
            ("filter.cutoff", 3_000.0),
            ("filter.env_amount", 1.2),
            ("amp.attack", 0.008),
            ("amp.decay", 0.9),
            ("amp.sustain", 0.35),
            ("amp.release", 0.45),
            ("filter.attack", 0.005),
            ("filter.decay", 0.4),
            ("filter.sustain", 0.2),
            ("filter.release", 0.4),
            ("env.curve", 0.6),
            ("lfo.rate", 5.0),
            ("lfo.depth", 0.0),
        ],
    },
    Preset {
        name: "Super Chord",
        params: &[
            ("osc1.wave", 2.0),
            ("osc2.wave", 2.0),
            ("osc2.semi", -12.0),
            ("osc2.detune", 8.0),
            ("osc.mix", 0.25),
            ("unison.voices", 4.0),
            ("unison.type", 1.0),
            ("unison.detune", 52.0),
            ("unison.blend", 0.7),
            ("unison.spread", 1.0),
            ("unison.lowcut", 0.9),
            ("filter.type", 1.0),
            ("filter.cutoff", 2_600.0),
            ("filter.resonance", 0.12),
            ("filter.env_amount", 2.4),
            ("filter.attack", 0.001),
            ("filter.decay", 0.35),
            ("filter.sustain", 0.25),
            ("amp.attack", 0.003),
            ("amp.decay", 0.4),
            ("amp.sustain", 0.55),
            ("amp.release", 0.35),
            ("env.curve", 0.6),
            ("out.gain", -2.5),
        ],
    },
    Preset {
        name: "Glass Bell",
        params: &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 1.0),
            ("osc2.semi", 19.0),
            ("osc2.detune", 2.0),
            ("osc.mix", 0.35),
            ("osc.noise", 0.06),
            ("osc.noise_color", 0.0),
            ("osc.noise_decay", 0.04),
            ("unison.voices", 0.0),
            ("filter.type", 0.0),
            ("filter.cutoff", 6_000.0),
            ("filter.env_amount", 1.5),
            ("filter.decay", 1.2),
            ("filter.sustain", 0.2),
            ("amp.attack", 0.001),
            ("amp.decay", 1.6),
            ("amp.sustain", 0.0),
            ("amp.release", 1.4),
            ("out.velocity", 0.8),
        ],
    },
    Preset {
        name: "Noise Riser",
        params: &[
            ("osc1.wave", 2.0),
            ("osc.mix", 0.0),
            ("osc.noise", 1.0),
            ("unison.voices", 4.0),
            ("unison.type", 1.0),
            ("unison.detune", 70.0),
            ("unison.spread", 1.0),
            ("filter.type", 2.0),
            ("filter.cutoff", 600.0),
            ("filter.resonance", 0.5),
            ("filter.env_amount", 0.0),
            ("filter.key_track", 0.0),
            ("filter.velocity", 0.0),
            ("amp.attack", 3.0),
            ("amp.sustain", 1.0),
            ("amp.release", 1.2),
            ("env.curve", 0.2),
            ("pitch.env_amount", -12.0),
            ("pitch.env_decay", 2.0),
            ("out.gain", -6.0),
        ],
    },
    Preset {
        name: "Square Lead",
        params: &[
            ("osc1.wave", 3.0),
            ("osc2.wave", 3.0),
            ("osc2.semi", 12.0),
            ("osc2.detune", 6.0),
            ("osc.mix", 0.3),
            ("osc.pw", 0.4),
            ("unison.voices", 1.0),
            ("unison.detune", 8.0),
            ("voice.glide", 0.06),
            ("filter.type", 1.0),
            ("filter.cutoff", 3_800.0),
            ("filter.resonance", 0.2),
            ("filter.env_amount", 1.2),
            ("amp.attack", 0.004),
            ("amp.sustain", 0.85),
            ("amp.release", 0.25),
            ("lfo.sync", 5.0),
            ("lfo.depth", 0.08),
            ("lfo.target", 0.0),
            ("out.gain", -2.5),
        ],
    },
    Preset {
        name: "Dark Pad",
        params: &[
            ("osc1.wave", 2.0),
            ("osc2.wave", 1.0),
            ("osc2.semi", -12.0),
            ("osc2.detune", 7.0),
            ("unison.voices", 3.0),
            ("unison.detune", 18.0),
            ("unison.spread", 1.0),
            ("filter.type", 1.0),
            ("filter.cutoff", 700.0),
            ("filter.resonance", 0.2),
            ("filter.env_amount", 1.0),
            ("filter.attack", 1.5),
            ("filter.decay", 3.0),
            ("amp.attack", 1.2),
            ("amp.sustain", 0.9),
            ("amp.release", 2.5),
            ("lfo.sync", 1.0),
            ("lfo.depth", 0.25),
            ("lfo.target", 1.0),
            ("analog.drift", 0.5),
            ("out.gain", -3.0),
        ],
    },
    Preset {
        name: "Supersaw",
        params: &[
            ("osc2.semi", -12.0),
            ("osc2.detune", 5.0),
            ("osc.mix", 0.25),
            ("unison.voices", 6.0),
            ("unison.type", 1.0),
            ("unison.detune", 55.0),
            ("unison.blend", 0.75),
            ("unison.spread", 1.0),
            ("unison.lowcut", 1.0),
            ("filter.type", 1.0),
            ("filter.cutoff", 6_500.0),
            ("filter.resonance", 0.08),
            ("filter.env_amount", 1.0),
            ("filter.key_track", 0.75),
            ("amp.attack", 0.008),
            ("amp.decay", 0.4),
            ("amp.sustain", 0.85),
            ("amp.release", 0.45),
            ("filter.attack", 0.008),
            ("filter.decay", 0.6),
            ("filter.sustain", 0.7),
            ("analog.drift", 0.35),
            ("out.gain", -5.0),
        ],
    },
    Preset {
        name: "Future Pad",
        params: &[
            ("osc2.semi", 12.0),
            ("osc2.detune", 8.0),
            ("osc.mix", 0.4),
            ("osc.noise", 0.04),
            ("osc.noise_color", 1.0),
            ("unison.voices", 5.0),
            ("unison.type", 1.0),
            ("unison.detune", 45.0),
            ("unison.blend", 0.6),
            ("unison.spread", 1.0),
            ("unison.lowcut", 0.8),
            ("filter.type", 1.0),
            ("filter.cutoff", 1_500.0),
            ("filter.resonance", 0.18),
            ("filter.env_amount", 1.8),
            ("filter.key_track", 0.6),
            ("amp.attack", 0.9),
            ("amp.decay", 1.2),
            ("amp.sustain", 0.85),
            ("amp.release", 1.8),
            ("filter.attack", 1.8),
            ("filter.decay", 2.5),
            ("filter.sustain", 0.45),
            ("filter.release", 1.5),
            ("lfo.rate", 0.18),
            ("lfo.depth", 0.35),
            ("lfo.target", 1.0),
            ("analog.drift", 0.5),
            ("out.gain", -5.0),
        ],
    },
    Preset {
        name: "Sidechain Chord",
        params: &[
            ("osc2.semi", -12.0),
            ("osc.mix", 0.3),
            ("unison.voices", 4.0),
            ("unison.type", 1.0),
            ("unison.detune", 48.0),
            ("unison.spread", 1.0),
            ("unison.lowcut", 0.9),
            ("filter.type", 1.0),
            ("filter.cutoff", 2_800.0),
            ("filter.resonance", 0.1),
            ("filter.env_amount", 1.5),
            ("filter.decay", 0.5),
            ("filter.sustain", 0.5),
            ("amp.attack", 0.004),
            ("amp.decay", 0.3),
            ("amp.sustain", 0.9),
            ("amp.release", 0.25),
            ("lfo.sync", 4.0),
            ("lfo.shape", 2.0),
            ("lfo.depth", 0.75),
            ("lfo.target", 2.0),
            ("out.gain", -2.0),
        ],
    },
    Preset {
        name: "Big Room Lead",
        params: &[
            ("osc2.wave", 3.0),
            ("osc2.semi", 12.0),
            ("osc2.detune", 7.0),
            ("osc.mix", 0.3),
            ("unison.voices", 4.0),
            ("unison.type", 1.0),
            ("unison.detune", 50.0),
            ("unison.blend", 0.8),
            ("unison.spread", 0.9),
            ("unison.lowcut", 0.7),
            ("voice.mode", 2.0),
            ("voice.glide", 0.06),
            ("filter.type", 1.0),
            ("filter.cutoff", 5_000.0),
            ("filter.resonance", 0.15),
            ("filter.drive", 0.25),
            ("filter.env_amount", 1.2),
            ("filter.key_track", 0.8),
            ("amp.attack", 0.006),
            ("amp.decay", 0.3),
            ("amp.sustain", 0.85),
            ("amp.release", 0.25),
            ("lfo.rate", 5.5),
            ("lfo.depth", 0.12),
            ("lfo.target", 0.0),
            ("out.gain", -4.5),
        ],
    },
    Preset {
        name: "Hoover",
        params: &[
            ("osc1.wave", 3.0),
            ("osc2.wave", 2.0),
            ("osc2.semi", -12.0),
            ("osc2.detune", 12.0),
            ("osc.mix", 0.5),
            ("unison.voices", 4.0),
            ("unison.detune", 40.0),
            ("unison.spread", 1.0),
            ("filter.type", 1.0),
            ("filter.cutoff", 2_400.0),
            ("filter.resonance", 0.3),
            ("filter.drive", 0.3),
            ("filter.env_amount", 1.5),
            ("amp.attack", 0.003),
            ("amp.decay", 0.4),
            ("amp.sustain", 0.8),
            ("amp.release", 0.3),
            ("filter.decay", 0.6),
            ("pitch.env_amount", 5.0),
            ("pitch.env_decay", 0.25),
            ("lfo.rate", 3.5),
            ("lfo.depth", 0.9),
            ("lfo.target", 3.0),
            ("out.gain", -3.0),
        ],
    },
    Preset {
        name: "Arp",
        params: &[
            ("osc2.wave", 3.0),
            ("osc2.detune", 6.0),
            ("osc.mix", 0.35),
            ("osc.noise", 0.05),
            ("osc.noise_color", 1.0),
            ("osc.noise_decay", 0.03),
            ("unison.voices", 1.0),
            ("unison.detune", 10.0),
            ("unison.spread", 0.7),
            ("filter.cutoff", 900.0),
            ("filter.resonance", 0.3),
            ("filter.env_amount", 3.0),
            ("filter.key_track", 0.6),
            ("filter.velocity", 0.5),
            ("amp.attack", 0.002),
            ("amp.decay", 0.22),
            ("amp.sustain", 0.0),
            ("amp.release", 0.12),
            ("filter.attack", 0.002),
            ("filter.decay", 0.18),
            ("filter.sustain", 0.0),
            ("env.curve", 0.75),
            ("out.gain", 1.0),
        ],
    },
    Preset {
        name: "Stab",
        params: &[
            ("osc2.semi", 0.0),
            ("osc2.detune", 9.0),
            ("osc.mix", 0.3),
            ("unison.voices", 4.0),
            ("unison.type", 1.0),
            ("unison.detune", 60.0),
            ("unison.blend", 0.7),
            ("unison.spread", 1.0),
            ("unison.lowcut", 0.9),
            ("filter.type", 1.0),
            ("filter.cutoff", 3_500.0),
            ("filter.resonance", 0.2),
            ("filter.drive", 0.3),
            ("filter.env_amount", 2.5),
            ("filter.attack", 0.001),
            ("filter.decay", 0.22),
            ("filter.sustain", 0.1),
            ("amp.attack", 0.002),
            ("amp.decay", 0.3),
            ("amp.sustain", 0.35),
            ("amp.release", 0.2),
            ("env.curve", 0.75),
            ("out.gain", -1.5),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Poly::new()))
}

const MAX_UNISON: usize = 9;
const MONO_STACK: usize = 8;

/// Outermost side-voice offset of the JP-8000 supersaw at full detune,
/// as a frequency ratio (Szabo).
const SUPER_MAX_OFFSET: f32 = 0.110_023_13;
/// Measured JP-8000 side offsets (seven voices) relative to the outermost.
const SUPER7: [f32; 7] = [-1.0, -0.571_6, -0.177_45, 0.0, 0.180_98, 0.565_03, 0.976_6];

/// Szabo's measured detune knob curve (0..1 in, 0..1 relative offset out).
fn super_detune_curve(x: f32) -> f32 {
    const C: [f64; 12] = [
        10_028.731_289_163_4,
        -50_818.865_204_592_4,
        111_363.480_872_936_8,
        -138_150.676_108_054_8,
        106_649.667_915_829_2,
        -53_046.964_275_187_5,
        17_019.951_858_008,
        -3_425.083_659_131_8,
        404.270_393_838_8,
        -24.187_882_439_1,
        0.671_741_763_4,
        0.003_011_559_6,
    ];
    let x = x.clamp(0.0, 1.0) as f64;
    let mut acc = 0.0f64;
    for c in C {
        acc = acc * x + c;
    }
    acc as f32
}

/// Detune positions (-1..1, ascending) and relative levels for `n` stacked
/// voices in `Super` mode; `blend` is Szabo's mix knob.
fn super_layout(n: usize, blend: f32, pos: &mut [f32; MAX_UNISON], gain: &mut [f32; MAX_UNISON]) {
    let x = blend.clamp(0.0, 1.0);
    let center = -0.553_66 * x + 0.997_85;
    let side = -0.737_64 * x * x + 1.284_1 * x + 0.044_372;
    if n == 7 {
        pos[..7].copy_from_slice(&SUPER7);
    } else {
        let pairs = n / 2;
        let odd = n % 2 == 1;
        let mut i = 0;
        for k in (0..pairs).rev() {
            pos[i] = -(((k + 1) as f32) / pairs as f32).powf(1.5);
            i += 1;
        }
        if odd {
            pos[i] = 0.0;
            i += 1;
        }
        for k in 0..pairs {
            pos[i] = (((k + 1) as f32) / pairs as f32).powf(1.5);
            i += 1;
        }
    }
    let mut power = 0.0;
    for u in 0..n {
        let g = if pos[u] == 0.0 { center } else { side };
        gain[u] = g;
        power += g * g;
    }
    let norm = 1.0 / power.sqrt().max(1.0e-3);
    for g in gain.iter_mut().take(n) {
        *g *= norm;
    }
}

/// Values shared by all voices, recomputed every control interval.
#[derive(Clone, Copy)]
struct Mod {
    sr: f32,
    w1: Waveform,
    w2: Waveform,
    n_uni: usize,
    sup: bool,
    mix1: f32,
    mix2: f32,
    pw: f32,
    semi: f32,
    detune2: f32,
    /// Classic: cents at the outermost voice. Super: ratio per unit position.
    uni_detune: f32,
    lowcut: f32,
    sub_gain: f32,
    noise_gain: f32,
    noise_pink: bool,
    noise_coef: f32,
    ftype: usize,
    cutoff: f32,
    res: f32,
    drive: f32,
    env_amt: f32,
    key_track: f32,
    vel_filter: f32,
    lfo_pitch: f32,
    lfo_filter: f32,
    lfo_pw: f32,
    pe_amt: f32,
    drift: f32,
    drift_slew: f32,
    drift_hold: u32,
    /// Fraction of the remaining glide distance covered per control tick.
    glide_k: f32,
}

#[derive(Clone, Copy)]
struct PolyVoice {
    active: bool,
    cur_key: f32,
    target_key: f32,
    osc1: [BlOsc; MAX_UNISON],
    osc2: [BlOsc; MAX_UNISON],
    sub_ph: f32,
    dt1: [f32; MAX_UNISON],
    dt2: [f32; MAX_UNISON],
    sub_dt: f32,
    pos: [f32; MAX_UNISON],
    gl: [f32; MAX_UNISON],
    gr: [f32; MAX_UNISON],
    pw: f32,
    noise: Noise,
    pink: Pink,
    noise_env: f32,
    drift_pitch: Drift,
    drift_cut: Drift,
    drift_uni: [Drift; MAX_UNISON],
    hp: [SatSvf; 2],
    svf: [SatSvf; 2],
    lad: [SatLadder; 2],
    up: [Up2; 2],
    down: [Down2; 2],
    os: bool,
    amp_env: Env,
    flt_env: Env,
    pitch_env: PitchEnv,
    velocity: f32,
    vel_gain: f32,
    fade: Fade,
}

impl PolyVoice {
    fn new(sr: f32) -> PolyVoice {
        let mut pitch_env = PitchEnv::new();
        pitch_env.set_decay(0.15, sr);
        PolyVoice {
            active: false,
            cur_key: 60.0,
            target_key: 60.0,
            osc1: [BlOsc::new(0.0); MAX_UNISON],
            osc2: [BlOsc::new(0.0); MAX_UNISON],
            sub_ph: 0.0,
            dt1: [0.0; MAX_UNISON],
            dt2: [0.0; MAX_UNISON],
            sub_dt: 0.0,
            pos: [0.0; MAX_UNISON],
            gl: [1.0; MAX_UNISON],
            gr: [1.0; MAX_UNISON],
            pw: 0.5,
            noise: Noise::new(1),
            pink: Pink::new(1),
            noise_env: 1.0,
            drift_pitch: Drift::new(11),
            drift_cut: Drift::new(12),
            drift_uni: std::array::from_fn(|i| Drift::new(20 + i as u32)),
            hp: [SatSvf::new(); 2],
            svf: [SatSvf::new(); 2],
            lad: [SatLadder::new(); 2],
            up: [Up2::new(); 2],
            down: [Down2::new(); 2],
            os: false,
            amp_env: Env::new(sr),
            flt_env: Env::new(sr),
            pitch_env,
            velocity: 0.0,
            vel_gain: 1.0,
            fade: Fade::new(),
        }
    }

    /// Recomputes pitches and filter coefficients from the shared state.
    fn control(&mut self, m: &Mod) {
        self.cur_key += (self.target_key - self.cur_key) * m.glide_k;
        let dr = m.drift;
        let dp = self.drift_pitch.tick(m.drift_slew, m.drift_hold) * 4.0 * dr;
        let dc = self.drift_cut.tick(m.drift_slew, m.drift_hold) * 0.09 * dr;
        let pe = self.pitch_env.level() * m.pe_amt;
        let base = self.cur_key + m.lfo_pitch + pe + dp * 0.01;
        let hz1 = key_to_hz(base);
        let hz2 = key_to_hz(base + m.semi + m.detune2 * 0.01);
        let inv_sr = 1.0 / m.sr;
        for u in 0..m.n_uni {
            let micro = cents_ratio(self.drift_uni[u].tick(m.drift_slew, m.drift_hold) * 3.0 * dr);
            let r = if m.sup {
                1.0 + m.uni_detune * self.pos[u]
            } else {
                ratio(self.pos[u] * m.uni_detune * 0.01)
            } * micro;
            self.dt1[u] = hz1 * r * inv_sr;
            self.dt2[u] = hz2 * r * inv_sr;
        }
        self.sub_dt = hz1 * 0.5 * inv_sr;
        self.pw = (m.pw + m.lfo_pw).clamp(0.05, 0.95);

        let os = m.drive > 1.05;
        self.os = os;
        let (sr_f, ramp) = if os {
            (2.0 * m.sr, 2.0 * CTRL as f32)
        } else {
            (m.sr, CTRL as f32)
        };
        let oct = m.env_amt * self.flt_env.level()
            + m.key_track * (self.cur_key - 60.0) / 12.0
            + m.vel_filter * (self.velocity - 0.6) * 4.0
            + m.lfo_filter
            + dc;
        let cutoff = (m.cutoff * oct.clamp(-12.0, 12.0).exp2()).clamp(20.0, 0.45 * sr_f);
        if m.ftype == 1 {
            let c = LadderCoef::new(cutoff, m.res, m.drive, sr_f);
            self.lad[0].glide_to(c, ramp);
            self.lad[1].glide_to(c, ramp);
        } else {
            let c = SvfCoef::new(cutoff, m.res, m.drive, sr_f);
            self.svf[0].glide_to(c, ramp);
            self.svf[1].glide_to(c, ramp);
        }
        if m.lowcut > 0.01 && m.n_uni > 1 {
            let c = SvfCoef::new(hz1 * m.lowcut, 0.05, 1.0, m.sr);
            self.hp[0].glide_to(c, CTRL as f32);
            self.hp[1].glide_to(c, CTRL as f32);
        }
    }

    #[inline]
    fn filter(&mut self, ch: usize, x: f32, ftype: usize) -> f32 {
        match ftype {
            1 => self.lad[ch].tick(x),
            t => self.svf[ch].tick(x, if t == 0 { 0 } else { t - 1 }),
        }
    }

    #[inline]
    fn tick(&mut self, m: &Mod) -> (f32, f32) {
        let amp = self.amp_env.tick();
        self.flt_env.tick();
        if m.pe_amt != 0.0 {
            self.pitch_env.tick();
        }
        let n = m.n_uni;
        let (mut l, mut r) = (0.0f32, 0.0f32);
        if m.mix1 > 1.0e-4 {
            let (mut sl, mut sr) = (0.0f32, 0.0f32);
            for u in 0..n {
                let s = self.osc1[u].tick(m.w1, self.dt1[u], self.pw);
                sl += s * self.gl[u];
                sr += s * self.gr[u];
            }
            l += sl * m.mix1;
            r += sr * m.mix1;
        }
        if m.mix2 > 1.0e-4 {
            let (mut sl, mut sr) = (0.0f32, 0.0f32);
            for u in 0..n {
                let s = self.osc2[u].tick(m.w2, self.dt2[u], self.pw);
                sl += s * self.gl[u];
                sr += s * self.gr[u];
            }
            l += sl * m.mix2;
            r += sr * m.mix2;
        }
        if m.lowcut > 0.01 && n > 1 {
            l = self.hp[0].tick(l, 2);
            r = self.hp[1].tick(r, 2);
        }
        if m.sub_gain > 0.0 {
            let s = fast_sin(self.sub_ph) * m.sub_gain;
            advance(&mut self.sub_ph, self.sub_dt);
            l += s;
            r += s;
        }
        if m.noise_gain > 0.0 {
            self.noise_env *= m.noise_coef;
            let w = if m.noise_pink {
                self.pink.tick() * 2.5
            } else {
                self.noise.white()
            };
            let nz = w * m.noise_gain * self.noise_env;
            l += nz;
            r += nz;
        }
        let (fl, fr) = if self.os {
            let (l0, l1) = self.up[0].tick(l);
            let (r0, r1) = self.up[1].tick(r);
            let a0 = self.filter(0, l0, m.ftype);
            let a1 = self.filter(0, l1, m.ftype);
            let b0 = self.filter(1, r0, m.ftype);
            let b1 = self.filter(1, r1, m.ftype);
            (self.down[0].tick(a0, a1), self.down[1].tick(b0, b1))
        } else {
            (self.filter(0, l, m.ftype), self.filter(1, r, m.ftype))
        };
        let g = amp * self.vel_gain * self.fade.tick();
        // A note decayed to nothing at zero sustain is finished even though
        // the key is still down.
        let dead = self.amp_env.stage() == Stage::Sustain && self.amp_env.level() < 1.0e-4;
        if self.amp_env.is_idle() || self.fade.is_done() || dead {
            self.active = false;
        }
        (fl * g, fr * g)
    }
}

impl PoolVoice for PolyVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.amp_env.level()
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
        self.amp_env.reset();
        self.flt_env.reset();
        self.pitch_env.reset();
    }
}

#[derive(Clone, Copy)]
struct Held {
    id: u32,
    key: u8,
}

pub(crate) struct Poly {
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    notes: u32,
    pool: Pool<PolyVoice>,
    m: Mod,
    // Unsmoothed settings.
    unison: usize,
    unison_super: bool,
    voice_mode: usize,
    lfo_sync: usize,
    lfo_shape: LfoShape,
    lfo_target: usize,
    amp_adsr: [f32; 4],
    flt_adsr: [f32; 4],
    curve: f32,
    vel_amp: f32,
    // Smoothed settings.
    semi: Ramp,
    detune2: Ramp,
    mix: Ramp,
    pw: Ramp,
    sub: Ramp,
    noise: Ramp,
    uni_detune: Ramp,
    uni_blend: Ramp,
    lowcut: Ramp,
    glide: Ramp,
    cutoff: Ramp,
    res: Ramp,
    drive: Ramp,
    env_amt: Ramp,
    key_track: Ramp,
    vel_filter: Ramp,
    pitch_env_amt: Ramp,
    drift: Ramp,
    lfo_rate: Ramp,
    lfo_depth: Ramp,
    gain: Ramp,
    uni_spread: f32,
    noise_decay: f32,
    pitch_env_decay: f32,
    // Runtime state.
    lfo: Lfo,
    lfo_dt: f32,
    lfo_value: f32,
    last_key: Option<f32>,
    out_gain: f32,
    trem_sm: f32,
    trem_k: f32,
    dc: [DcBlock; 2],
    stack: [Held; MONO_STACK],
    stack_len: usize,
}

impl Poly {
    pub(crate) fn new() -> Poly {
        Poly {
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            notes: 0,
            pool: Pool::new(20, 16, StealPolicy::Oldest, 48_000.0, || {
                PolyVoice::new(48_000.0)
            }),
            m: Mod {
                sr: 48_000.0,
                w1: Waveform::Saw,
                w2: Waveform::Saw,
                n_uni: 3,
                sup: false,
                mix1: 0.4,
                mix2: 0.4,
                pw: 0.5,
                semi: 0.0,
                detune2: 9.0,
                uni_detune: 14.0,
                lowcut: 0.0,
                sub_gain: 0.0,
                noise_gain: 0.0,
                noise_pink: false,
                noise_coef: 1.0,
                ftype: 1,
                cutoff: 1_800.0,
                res: 0.15,
                drive: 1.0,
                env_amt: 1.5,
                key_track: 0.5,
                vel_filter: 0.3,
                lfo_pitch: 0.0,
                lfo_filter: 0.0,
                lfo_pw: 0.0,
                pe_amt: 0.0,
                drift: 0.3,
                drift_slew: 0.0004,
                drift_hold: 3_000,
                glide_k: 1.0,
            },
            unison: 3,
            unison_super: false,
            voice_mode: 0,
            lfo_sync: 0,
            lfo_shape: LfoShape::Sine,
            lfo_target: 0,
            amp_adsr: [0.06, 0.5, 0.8, 0.7],
            flt_adsr: [0.04, 0.8, 0.35, 0.6],
            curve: 0.5,
            vel_amp: 0.6,
            semi: Ramp::new(0.0),
            detune2: Ramp::new(9.0),
            mix: Ramp::new(0.5),
            pw: Ramp::new(0.5),
            sub: Ramp::new(0.0),
            noise: Ramp::new(0.0),
            uni_detune: Ramp::new(14.0),
            uni_blend: Ramp::new(0.7),
            lowcut: Ramp::new(0.0),
            glide: Ramp::new(0.0),
            cutoff: Ramp::new(1_800.0f32.ln()),
            res: Ramp::new(0.15),
            drive: Ramp::new(0.0),
            env_amt: Ramp::new(1.5),
            key_track: Ramp::new(0.5),
            vel_filter: Ramp::new(0.3),
            pitch_env_amt: Ramp::new(0.0),
            drift: Ramp::new(0.3),
            lfo_rate: Ramp::new(4.5f32.ln()),
            lfo_depth: Ramp::new(0.0),
            gain: Ramp::new(0.0),
            uni_spread: 0.8,
            noise_decay: 0.0,
            pitch_env_decay: 0.15,
            lfo: Lfo::new(0x1F0),
            lfo_dt: 0.0,
            lfo_value: 0.0,
            last_key: None,
            out_gain: 1.0,
            trem_sm: 1.0,
            trem_k: 0.005,
            dc: [DcBlock::new(48_000.0); 2],
            stack: [Held { id: 0, key: 0 }; MONO_STACK],
            stack_len: 0,
        }
    }

    fn apply_envelopes(&mut self) {
        let a = self.amp_adsr;
        let f = self.flt_adsr;
        let c = self.curve;
        let pd = self.pitch_env_decay;
        let sr = self.sr;
        for v in &mut self.pool.voices {
            v.amp_env.set(a[0], a[1], a[2], a[3], c);
            v.flt_env.set(f[0], f[1], f[2], f[3], c);
            v.pitch_env.set_decay(pd, sr);
        }
    }

    /// Recomputes shared values from the smoothed parameters.
    fn derive(&mut self) {
        let depth = self.lfo_depth.get();
        let lfo = self.lfo_value;
        let m = &mut self.m;
        m.sr = self.sr;
        m.n_uni = self.unison;
        m.sup = self.unison_super && self.unison > 1;
        let mix = self.mix.get();
        m.mix1 = (1.0 - mix) * 0.8;
        m.mix2 = mix * 0.8;
        m.pw = self.pw.get();
        m.semi = self.semi.get();
        m.detune2 = self.detune2.get();
        let det_mod = if self.lfo_target == 4 {
            (1.0 + lfo * depth * 0.6).max(0.0)
        } else {
            1.0
        };
        let det = (self.uni_detune.get() * det_mod).clamp(0.0, 100.0);
        m.uni_detune = if m.sup {
            super_detune_curve(det * 0.01) * SUPER_MAX_OFFSET
        } else {
            det
        };
        m.lowcut = if m.sup { self.lowcut.get() } else { 0.0 };
        m.sub_gain = self.sub.get() * 0.5;
        m.noise_gain = self.noise.get() * 0.35;
        m.noise_coef = if self.noise_decay > 0.001 {
            (-6.907_755 / (self.noise_decay * self.sr)).exp()
        } else {
            1.0
        };
        m.cutoff = self.cutoff.get().exp();
        m.res = self.res.get();
        let d = self.drive.get();
        m.drive = 1.0 + 7.0 * d * d;
        m.env_amt = self.env_amt.get();
        m.key_track = self.key_track.get();
        m.vel_filter = self.vel_filter.get();
        m.lfo_pitch = if self.lfo_target == 0 {
            lfo * depth * 2.0
        } else {
            0.0
        };
        m.lfo_filter = if self.lfo_target == 1 {
            lfo * depth * 3.0
        } else {
            0.0
        };
        m.lfo_pw = if self.lfo_target == 3 {
            lfo * depth * 0.4
        } else {
            0.0
        };
        m.pe_amt = self.pitch_env_amt.get();
        m.drift = self.drift.get();
        m.drift_slew = 1.0 - (-(CTRL as f32) / (0.8 * self.sr)).exp();
        m.drift_hold = (1.3 * self.sr / CTRL as f32) as u32;
        let glide = self.glide.get();
        m.glide_k = if glide <= 0.001 {
            1.0
        } else {
            1.0 - (-3.0 * CTRL as f32 / (glide * self.sr)).exp()
        };
        self.out_gain = db_to_gain(self.gain.get());
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.semi.tick(k);
        self.detune2.tick(k);
        self.mix.tick(k);
        self.pw.tick(k);
        self.sub.tick(k);
        self.noise.tick(k);
        self.uni_detune.tick(k);
        self.uni_blend.tick(k);
        self.lowcut.tick(k);
        self.glide.tick(k);
        self.cutoff.tick(k);
        self.res.tick(k);
        self.drive.tick(k);
        self.env_amt.tick(k);
        self.key_track.tick(k);
        self.vel_filter.tick(k);
        self.pitch_env_amt.tick(k);
        self.drift.tick(k);
        self.lfo_rate.tick(k);
        self.lfo_depth.tick(k);
        self.gain.tick(k);
        self.derive();
        let m = self.m;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.control(&m);
        }
        self.pool.sync();
    }

    fn lfo_increment(&self, ctx: &ProcessContext) -> f32 {
        let beats = SYNC_BEATS[self.lfo_sync.min(SYNC_BEATS.len() - 1)];
        let hz = if beats > 0.0 {
            (ctx.tempo_bpm.clamp(1.0, 999.0) / 60.0 / beats) as f32
        } else {
            self.lfo_rate.get().exp()
        };
        hz / self.sr
    }

    /// Initializes `slot` for a new note. With `fresh` false the voice keeps
    /// sounding (legato): only pitch target and velocity change.
    fn start_voice(&mut self, slot: usize, key: u8, velocity: f32, fresh: bool, retrigger: bool) {
        self.notes = self.notes.wrapping_add(1);
        let target = key as f32;
        let glide_from = if self.m.glide_k < 1.0 {
            self.last_key.unwrap_or(target)
        } else {
            target
        };
        self.last_key = Some(target);
        let m = self.m;
        let n = self.unison;
        let seed = self.notes;
        let spread = self.uni_spread;
        let sup = m.sup;
        let blend = self.uni_blend.get();
        let vel_amp = vel_gain(velocity, self.vel_amp);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        if fresh {
            v.cur_key = glide_from;
        }
        v.target_key = target;
        v.velocity = velocity;
        v.vel_gain = vel_amp * 0.9;
        v.fade.reset();
        if fresh {
            v.noise.reseed(seed);
            v.pink.reset(seed ^ 0x5151);
            v.drift_pitch.reseed(seed.wrapping_mul(7) + 1);
            v.drift_cut.reseed(seed.wrapping_mul(13) + 2);
            for (i, d) in v.drift_uni.iter_mut().enumerate() {
                d.reseed(seed.wrapping_mul(31).wrapping_add(i as u32 * 977 + 3));
            }
            let mut pos = [0.0f32; MAX_UNISON];
            let mut gain = [1.0f32; MAX_UNISON];
            if sup {
                super_layout(n, blend, &mut pos, &mut gain);
            } else {
                let norm = 1.0 / (n as f32).sqrt();
                for u in 0..n {
                    pos[u] = if n > 1 {
                        -1.0 + 2.0 * u as f32 / (n - 1) as f32
                    } else {
                        0.0
                    };
                    gain[u] = norm;
                }
            }
            let mut rng = Noise::new(seed ^ 0xA5A5);
            for u in 0..MAX_UNISON {
                v.pos[u] = pos[u];
                let (gl, gr) = pan_gains(pos[u] * spread);
                // Unused unison slots stay silent even if the unison count
                // is raised while this note is still sounding.
                let used = if u < n { gain[u] } else { 0.0 };
                v.gl[u] = gl * used;
                v.gr[u] = gr * used;
                // Super mode is free running: every saw, the center
                // included, starts at a random phase. Classic stacks start
                // within a quarter cycle of each other, which keeps the
                // fundamentals in phase (a thin 2-3 voice stack never
                // cancels in mono) while the detune still decorrelates it.
                let spread_ph = if sup { 1.0 } else { 0.25 };
                let first_fixed = u == 0 && !sup;
                let (p1, p2) = (rng.uniform() * spread_ph, rng.uniform() * spread_ph);
                v.osc1[u].reset(if first_fixed { 0.0 } else { p1 });
                v.osc2[u].reset(if first_fixed { 0.0 } else { p2 });
            }
            v.sub_ph = 0.0;
            for ch in 0..2 {
                v.svf[ch].reset();
                v.lad[ch].reset();
                v.hp[ch].reset();
                v.up[ch].reset();
                v.down[ch].reset();
            }
            v.noise_env = 1.0;
            v.amp_env.reset();
            v.flt_env.reset();
            v.pitch_env.reset();
        }
        if fresh || retrigger {
            v.noise_env = 1.0;
            v.amp_env.note_on();
            v.flt_env.note_on();
            v.pitch_env.trigger();
        }
        v.control(&m);
        if fresh {
            // Start the filters at their first coefficients rather than
            // gliding in from stale ones.
            for ch in 0..2 {
                v.svf[ch].settle();
                v.lad[ch].settle();
                v.hp[ch].settle();
            }
        }
    }

    fn mono_note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let overlapping = self.stack_len > 0;
        if self.stack_len == MONO_STACK {
            self.stack.copy_within(1.., 0);
            self.stack_len -= 1;
        }
        self.stack[self.stack_len] = Held { id, key };
        self.stack_len += 1;
        let reuse = self.pool.voices[0].active;
        // Legato keeps the envelopes running when notes overlap; Mono
        // retriggers them every note.
        let retrigger = !(self.voice_mode == 2 && overlapping);
        self.start_voice(0, key, velocity, !reuse, retrigger);
    }
}

impl Synth for Poly {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(20, 16, StealPolicy::Oldest, sample_rate, || {
            PolyVoice::new(sample_rate)
        });
        self.dc = [DcBlock::new(sample_rate); 2];
        self.trem_k = 1.0 - (-1.0 / (0.004 * sample_rate)).exp();
        self.ctrl = 0;
        self.stack_len = 0;
        self.m.sr = sample_rate;
        self.apply_envelopes();
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.dc[0].reset();
        self.dc[1].reset();
        self.ctrl = 0;
        self.stack_len = 0;
        self.last_key = None;
        self.lfo.reset(0.0);
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            OSC1_WAVE => self.m.w1 = Waveform::from_index(v as usize),
            OSC2_WAVE => self.m.w2 = Waveform::from_index(v as usize),
            OSC2_SEMI => self.semi.set(v, imm),
            OSC2_DETUNE => self.detune2.set(v, imm),
            OSC_MIX => self.mix.set(v, imm),
            OSC_PW => self.pw.set(v, imm),
            OSC_SUB => self.sub.set(v, imm),
            OSC_NOISE => self.noise.set(v, imm),
            NOISE_COLOR => self.m.noise_pink = v >= 0.5,
            NOISE_DECAY => self.noise_decay = v,
            UNISON => self.unison = (v as usize + 1).clamp(1, MAX_UNISON),
            UNISON_TYPE => self.unison_super = v >= 0.5,
            UNISON_DETUNE => self.uni_detune.set(v, imm),
            UNISON_BLEND => self.uni_blend.set(v, imm),
            UNISON_SPREAD => self.uni_spread = v,
            UNISON_LOWCUT => self.lowcut.set(v, imm),
            VOICE_MODE => self.voice_mode = (v as usize).min(2),
            GLIDE => self.glide.set(v, imm),
            DRIFT => self.drift.set(v, imm),
            FILTER_TYPE => self.m.ftype = v as usize,
            CUTOFF => self.cutoff.set(v.ln(), imm),
            RESONANCE => self.res.set(v, imm),
            FILTER_DRIVE => self.drive.set(v, imm),
            ENV_AMOUNT => self.env_amt.set(v, imm),
            KEY_TRACK => self.key_track.set(v, imm),
            FILTER_VEL => self.vel_filter.set(v, imm),
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
            FLT_A => {
                self.flt_adsr[0] = v;
                self.apply_envelopes();
            }
            FLT_D => {
                self.flt_adsr[1] = v;
                self.apply_envelopes();
            }
            FLT_S => {
                self.flt_adsr[2] = v;
                self.apply_envelopes();
            }
            FLT_R => {
                self.flt_adsr[3] = v;
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
            LFO_RATE => self.lfo_rate.set(v.ln(), imm),
            LFO_SYNC => self.lfo_sync = v as usize,
            LFO_SHAPE => self.lfo_shape = LfoShape::from_index(v as usize),
            LFO_DEPTH => self.lfo_depth.set(v, imm),
            LFO_TARGET => self.lfo_target = v as usize,
            GAIN => self.gain.set(v, imm),
            VEL_AMP => self.vel_amp = v,
            _ => {}
        }
        if imm {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        if self.voice_mode != 0 {
            self.mono_note_on(key, velocity, id);
            return;
        }
        let slot = self.pool.begin(key, id);
        self.start_voice(slot, key, velocity, true, true);
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        if let Some(pos) = self.stack[..self.stack_len].iter().position(|h| h.id == id) {
            let was_top = pos + 1 == self.stack_len;
            self.stack.copy_within(pos + 1..self.stack_len, pos);
            self.stack_len -= 1;
            if self.stack_len == 0 {
                let v = &mut self.pool.voices[0];
                v.amp_env.note_off();
                v.flt_env.note_off();
            } else if was_top {
                // Fall back to the previous held key without retriggering.
                let back = self.stack[self.stack_len - 1];
                self.pool.voices[0].target_key = back.key as f32;
                self.last_key = Some(back.key as f32);
            }
        }
        self.pool.release_id(id, |v| {
            v.amp_env.note_off();
            v.flt_env.note_off();
        });
    }

    fn all_notes_off(&mut self) {
        if self.stack_len > 0 {
            self.stack_len = 0;
            let v = &mut self.pool.voices[0];
            v.amp_env.note_off();
            v.flt_env.note_off();
        }
        self.pool.release_all(|v| {
            v.amp_env.note_off();
            v.flt_env.note_off();
        });
    }

    fn all_sound_off(&mut self) {
        self.stack_len = 0;
        self.pool.kill_all();
        self.dc[0].reset();
        self.dc[1].reset();
    }

    fn render(&mut self, ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        let beats = SYNC_BEATS[self.lfo_sync.min(SYNC_BEATS.len() - 1)];
        if beats > 0.0 && ctx.playing {
            let cycles = ctx.beat_position / beats;
            self.lfo.reset((cycles - cycles.floor()) as f32);
        }
        self.lfo_dt = self.lfo_increment(ctx);
        for n in 0..left.len().min(right.len()) {
            self.lfo_value = self.lfo.tick(self.lfo_shape, self.lfo_dt);
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let (mut l, mut r) = (0.0, 0.0);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick(&self.m);
                l += a;
                r += b;
            }
            let trem = if self.lfo_target == 2 {
                // How far into the dip the LFO is (1 at its trough). A rising
                // saw is squared so the level snaps back fast and then
                // settles, the shape of a sidechain compressor releasing.
                let x = 0.5 - 0.5 * self.lfo_value;
                let x = if self.lfo_shape == LfoShape::Saw {
                    x * x
                } else {
                    x
                };
                1.0 - self.lfo_depth.get() * x
            } else {
                1.0
            };
            // A few milliseconds of slew so a stepping LFO never clicks.
            self.trem_sm += (trem - self.trem_sm) * self.trem_k;
            let g = self.out_gain * self.trem_sm;
            left[n] = soft_ceiling(self.dc[0].tick(l * g));
            right[n] = soft_ceiling(self.dc[1].tick(r * g));
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
