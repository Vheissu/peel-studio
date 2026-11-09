//! `peel.synth.poly`: a 16-voice subtractive synthesizer.
//!
//! Two band-limited oscillators, a sine sub and noise feed up to four
//! detuned, stereo-spread unison copies per note, then a resonant filter
//! (12 dB state-variable or 24 dB ladder). Separate ADSR envelopes drive the
//! amplifier and the filter, and one LFO (free or tempo synced) can modulate
//! pitch, filter, amplitude or pulse width.

use peel_dsp::envelope::{Adsr, Stage};
use peel_dsp::filter::{Ladder, Svf, SvfMode};
use peel_dsp::noise::Noise;
use peel_dsp::osc::{Lfo, LfoShape, Osc, Waveform};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz, pan_gains};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

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
const FILTER_TYPE: ParamId = 20;
const CUTOFF: ParamId = 21;
const RESONANCE: ParamId = 22;
const ENV_AMOUNT: ParamId = 23;
const KEY_TRACK: ParamId = 24;
const FILTER_VEL: ParamId = 25;
const AMP_A: ParamId = 30;
const AMP_D: ParamId = 31;
const AMP_S: ParamId = 32;
const AMP_R: ParamId = 33;
const FLT_A: ParamId = 34;
const FLT_D: ParamId = 35;
const FLT_S: ParamId = 36;
const FLT_R: ParamId = 37;
const LFO_RATE: ParamId = 40;
const LFO_SYNC: ParamId = 41;
const LFO_SHAPE: ParamId = 42;
const LFO_DEPTH: ParamId = 43;
const LFO_TARGET: ParamId = 44;
const GAIN: ParamId = 50;
const VEL_AMP: ParamId = 51;

const WAVES: &[&str] = &["Sine", "Triangle", "Saw", "Square"];
const FILTERS: &[&str] = &["LP 12", "LP 24", "Band Pass", "High Pass", "Notch"];
const SYNCS: &[&str] = &[
    "Free", "2 Bars", "1 Bar", "1/2", "1/4", "1/8", "1/8T", "1/16", "1/16T",
];
/// Quarter notes per LFO cycle for each sync choice (0 means free-running).
const SYNC_BEATS: [f64; 9] = [0.0, 8.0, 4.0, 2.0, 1.0, 0.5, 1.0 / 3.0, 0.25, 1.0 / 6.0];
const SHAPES: &[&str] = &["Sine", "Triangle", "Saw", "Square", "Random"];
const TARGETS: &[&str] = &["Pitch", "Filter", "Amp", "Pulse Width"];

static PARAMS: [ParamDescriptor; 33] = [
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
        UNISON,
        "unison.voices",
        "Unison",
        "Oscillators",
        &["1", "2", "3", "4"],
        1,
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
        UNISON_SPREAD,
        "unison.spread",
        "Stereo Spread",
        "Oscillators",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
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
    description: "Polyphonic subtractive synth with unison, filter and LFO",
    category: "Synth",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 12] = [
    Preset {
        name: "Warm Pad",
        params: &[
            ("osc1.wave", 2.0),
            ("osc2.wave", 2.0),
            ("osc2.semi", 0.0),
            ("osc2.detune", 9.0),
            ("osc.mix", 0.5),
            ("osc.pw", 0.5),
            ("osc.sub", 0.0),
            ("osc.noise", 0.0),
            ("unison.voices", 1.0),
            ("unison.detune", 14.0),
            ("unison.spread", 0.7),
            ("voice.glide", 0.0),
            ("filter.type", 1.0),
            ("filter.cutoff", 1_800.0),
            ("filter.resonance", 0.15),
            ("filter.env_amount", 1.5),
            ("filter.key_track", 0.5),
            ("filter.velocity", 0.3),
            ("amp.attack", 0.06),
            ("amp.decay", 0.5),
            ("amp.sustain", 0.8),
            ("amp.release", 0.7),
            ("filter.attack", 0.04),
            ("filter.decay", 0.8),
            ("filter.sustain", 0.35),
            ("filter.release", 0.6),
            ("lfo.rate", 4.5),
            ("lfo.sync", 0.0),
            ("lfo.shape", 0.0),
            ("lfo.depth", 0.0),
            ("lfo.target", 0.0),
            ("out.gain", 0.0),
            ("out.velocity", 0.6),
        ],
    },
    Preset {
        name: "Pluck",
        params: &[
            ("osc2.wave", 3.0),
            ("osc2.semi", 12.0),
            ("osc2.detune", 4.0),
            ("osc.mix", 0.3),
            ("unison.voices", 0.0),
            ("filter.cutoff", 650.0),
            ("filter.resonance", 0.25),
            ("filter.env_amount", 3.8),
            ("filter.velocity", 0.6),
            ("amp.attack", 0.002),
            ("amp.decay", 0.5),
            ("amp.sustain", 0.0),
            ("amp.release", 0.25),
            ("filter.attack", 0.002),
            ("filter.decay", 0.25),
            ("filter.sustain", 0.0),
            ("filter.release", 0.25),
            ("out.gain", 3.0),
        ],
    },
    Preset {
        name: "Supersaw Lead",
        params: &[
            ("osc2.detune", 14.0),
            ("unison.voices", 3.0),
            ("unison.detune", 26.0),
            ("unison.spread", 0.9),
            ("voice.glide", 0.05),
            ("filter.type", 0.0),
            ("filter.cutoff", 5_200.0),
            ("filter.resonance", 0.1),
            ("filter.env_amount", 1.0),
            ("filter.key_track", 0.7),
            ("amp.attack", 0.005),
            ("amp.decay", 0.3),
            ("amp.sustain", 0.85),
            ("amp.release", 0.3),
            ("filter.attack", 0.005),
            ("filter.decay", 0.5),
            ("filter.sustain", 0.6),
            ("filter.release", 0.3),
            ("lfo.rate", 5.5),
            ("lfo.depth", 0.18),
            ("lfo.target", 0.0),
            ("out.gain", -3.0),
        ],
    },
    Preset {
        name: "Brass",
        params: &[
            ("osc2.detune", 5.0),
            ("unison.voices", 1.0),
            ("unison.detune", 8.0),
            ("unison.spread", 0.4),
            ("filter.cutoff", 520.0),
            ("filter.resonance", 0.15),
            ("filter.env_amount", 3.2),
            ("filter.velocity", 0.5),
            ("amp.attack", 0.04),
            ("amp.decay", 0.25),
            ("amp.sustain", 0.85),
            ("amp.release", 0.12),
            ("filter.attack", 0.08),
            ("filter.decay", 0.35),
            ("filter.sustain", 0.5),
            ("filter.release", 0.15),
        ],
    },
    Preset {
        name: "Strings",
        params: &[
            ("osc2.detune", 11.0),
            ("unison.voices", 3.0),
            ("unison.detune", 16.0),
            ("unison.spread", 1.0),
            ("filter.type", 0.0),
            ("filter.cutoff", 3_400.0),
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
            ("unison.voices", 0.0),
            ("filter.cutoff", 420.0),
            ("filter.resonance", 0.55),
            ("filter.env_amount", 0.0),
            ("amp.attack", 0.005),
            ("amp.decay", 0.3),
            ("amp.sustain", 1.0),
            ("amp.release", 0.15),
            ("lfo.sync", 5.0),
            ("lfo.shape", 1.0),
            ("lfo.depth", 0.75),
            ("lfo.target", 1.0),
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
            ("lfo.rate", 5.0),
            ("lfo.depth", 0.0),
        ],
    },
    Preset {
        name: "Super Chord",
        params: &[
            ("osc1.wave", 2.0),
            ("osc2.wave", 2.0),
            ("osc2.detune", 12.0),
            ("unison.voices", 3.0),
            ("unison.detune", 30.0),
            ("unison.spread", 1.0),
            ("filter.type", 1.0),
            ("filter.cutoff", 2400.0),
            ("filter.resonance", 0.12),
            ("filter.env_amount", 2.4),
            ("filter.attack", 0.001),
            ("filter.decay", 0.35),
            ("filter.sustain", 0.25),
            ("amp.attack", 0.003),
            ("amp.decay", 0.4),
            ("amp.sustain", 0.55),
            ("amp.release", 0.35),
            ("out.gain", -4.0),
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
            ("unison.voices", 0.0),
            ("filter.type", 0.0),
            ("filter.cutoff", 6000.0),
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
            ("unison.voices", 0.0),
            ("filter.type", 2.0),
            ("filter.cutoff", 600.0),
            ("filter.resonance", 0.45),
            ("filter.env_amount", 0.0),
            ("amp.attack", 2.5),
            ("amp.sustain", 1.0),
            ("amp.release", 1.2),
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
            ("unison.voices", 0.0),
            ("voice.glide", 0.06),
            ("filter.type", 1.0),
            ("filter.cutoff", 3800.0),
            ("filter.resonance", 0.2),
            ("filter.env_amount", 1.2),
            ("amp.attack", 0.004),
            ("amp.sustain", 0.85),
            ("amp.release", 0.25),
            ("lfo.sync", 5.0),
            ("lfo.depth", 0.08),
            ("lfo.target", 0.0),
        ],
    },
    Preset {
        name: "Dark Pad",
        params: &[
            ("osc1.wave", 2.0),
            ("osc2.wave", 1.0),
            ("osc2.semi", -12.0),
            ("osc2.detune", 7.0),
            ("unison.voices", 2.0),
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
            ("out.gain", -3.0),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Poly::new()))
}

const MAX_UNISON: usize = 4;

/// Values shared by all voices, recomputed every control interval.
#[derive(Clone, Copy)]
struct Mod {
    sr: f32,
    w1: Waveform,
    w2: Waveform,
    n_uni: usize,
    mix1: f32,
    mix2: f32,
    pw: f32,
    semi: f32,
    detune2: f32,
    uni_detune: f32,
    sub_gain: f32,
    noise_gain: f32,
    ftype: usize,
    cutoff: f32,
    res: f32,
    env_amt: f32,
    key_track: f32,
    vel_filter: f32,
    lfo_pitch: f32,
    lfo_filter: f32,
    lfo_pw: f32,
    /// Fraction of the remaining glide distance covered per control tick.
    glide_k: f32,
}

#[derive(Clone, Copy)]
struct PolyVoice {
    active: bool,
    cur_key: f32,
    target_key: f32,
    osc1: [Osc; MAX_UNISON],
    osc2: [Osc; MAX_UNISON],
    sub: Osc,
    dt1: [f32; MAX_UNISON],
    dt2: [f32; MAX_UNISON],
    sub_dt: f32,
    pos: [f32; MAX_UNISON],
    gl: [f32; MAX_UNISON],
    gr: [f32; MAX_UNISON],
    pw: f32,
    noise: Noise,
    svf_l: Svf,
    svf_r: Svf,
    lad_l: Ladder,
    lad_r: Ladder,
    amp_env: Adsr,
    flt_env: Adsr,
    velocity: f32,
    vel_gain: f32,
    fade: Fade,
}

impl PolyVoice {
    fn new(sr: f32) -> PolyVoice {
        PolyVoice {
            active: false,
            cur_key: 60.0,
            target_key: 60.0,
            osc1: [Osc::new(); MAX_UNISON],
            osc2: [Osc::new(); MAX_UNISON],
            sub: Osc::new(),
            dt1: [0.0; MAX_UNISON],
            dt2: [0.0; MAX_UNISON],
            sub_dt: 0.0,
            pos: [0.0; MAX_UNISON],
            gl: [1.0; MAX_UNISON],
            gr: [1.0; MAX_UNISON],
            pw: 0.5,
            noise: Noise::new(1),
            svf_l: Svf::new(),
            svf_r: Svf::new(),
            lad_l: Ladder::new(),
            lad_r: Ladder::new(),
            amp_env: Adsr::new(sr),
            flt_env: Adsr::new(sr),
            velocity: 0.0,
            vel_gain: 1.0,
            fade: Fade::new(),
        }
    }

    /// Recomputes pitches and filter coefficients from the shared state.
    fn control(&mut self, m: &Mod) {
        self.cur_key += (self.target_key - self.cur_key) * m.glide_k;
        let base = self.cur_key + m.lfo_pitch;
        let inv_sr = 1.0 / m.sr;
        for u in 0..m.n_uni {
            let off = self.pos[u] * m.uni_detune * 0.01;
            self.dt1[u] = key_to_hz(base + off) * inv_sr;
            self.dt2[u] = key_to_hz(base + m.semi + m.detune2 * 0.01 + off) * inv_sr;
        }
        self.sub_dt = key_to_hz(base - 12.0) * inv_sr;
        self.pw = (m.pw + m.lfo_pw).clamp(0.05, 0.95);

        let oct = m.env_amt * self.flt_env.level()
            + m.key_track * (self.cur_key - 60.0) / 12.0
            + m.vel_filter * (self.velocity - 0.6) * 4.0
            + m.lfo_filter;
        let cutoff = (m.cutoff * oct.clamp(-12.0, 12.0).exp2()).clamp(20.0, 0.45 * m.sr);
        if m.ftype == 1 {
            self.lad_l.set(cutoff, m.res, 1.0, m.sr);
            self.lad_r.copy_coeffs(&self.lad_l);
        } else {
            self.svf_l.set(cutoff, m.res, m.sr);
            self.svf_r.copy_coeffs(&self.svf_l);
        }
    }

    #[inline]
    fn tick(&mut self, m: &Mod) -> (f32, f32) {
        let amp = self.amp_env.tick();
        self.flt_env.tick();
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for u in 0..m.n_uni {
            let mut s = self.osc1[u].tick(m.w1, self.dt1[u], self.pw) * m.mix1;
            s += self.osc2[u].tick(m.w2, self.dt2[u], self.pw) * m.mix2;
            l += s * self.gl[u];
            r += s * self.gr[u];
        }
        if m.sub_gain > 0.0 {
            let s = self.sub.tick(Waveform::Sine, self.sub_dt, 0.5) * m.sub_gain;
            l += s;
            r += s;
        }
        if m.noise_gain > 0.0 {
            let n = self.noise.white() * m.noise_gain;
            l += n;
            r += n;
        }
        let (fl, fr) = match m.ftype {
            0 => (
                self.svf_l.process(l, SvfMode::LowPass),
                self.svf_r.process(r, SvfMode::LowPass),
            ),
            1 => (self.lad_l.tick(l), self.lad_r.tick(r)),
            t => {
                let mode = SvfMode::from_index(t - 1);
                (self.svf_l.process(l, mode), self.svf_r.process(r, mode))
            }
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
    }
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
    lfo_sync: usize,
    lfo_shape: LfoShape,
    lfo_target: usize,
    amp_adsr: [f32; 4],
    flt_adsr: [f32; 4],
    vel_amp: f32,
    // Smoothed settings.
    semi: Ramp,
    detune2: Ramp,
    mix: Ramp,
    pw: Ramp,
    sub: Ramp,
    noise: Ramp,
    uni_detune: Ramp,
    glide: Ramp,
    cutoff: Ramp,
    res: Ramp,
    env_amt: Ramp,
    key_track: Ramp,
    vel_filter: Ramp,
    lfo_rate: Ramp,
    lfo_depth: Ramp,
    gain: Ramp,
    uni_spread: f32,
    // Runtime state.
    lfo: Lfo,
    lfo_dt: f32,
    lfo_value: f32,
    last_key: Option<f32>,
    out_gain: f32,
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
                n_uni: 1,
                mix1: 0.5,
                mix2: 0.5,
                pw: 0.5,
                semi: 0.0,
                detune2: 9.0,
                uni_detune: 14.0,
                sub_gain: 0.0,
                noise_gain: 0.0,
                ftype: 1,
                cutoff: 1_800.0,
                res: 0.15,
                env_amt: 1.5,
                key_track: 0.5,
                vel_filter: 0.3,
                lfo_pitch: 0.0,
                lfo_filter: 0.0,
                lfo_pw: 0.0,
                glide_k: 1.0,
            },
            unison: 1,
            lfo_sync: 0,
            lfo_shape: LfoShape::Sine,
            lfo_target: 0,
            amp_adsr: [0.06, 0.5, 0.8, 0.7],
            flt_adsr: [0.04, 0.8, 0.35, 0.6],
            vel_amp: 0.6,
            semi: Ramp::new(0.0),
            detune2: Ramp::new(9.0),
            mix: Ramp::new(0.5),
            pw: Ramp::new(0.5),
            sub: Ramp::new(0.0),
            noise: Ramp::new(0.0),
            uni_detune: Ramp::new(14.0),
            glide: Ramp::new(0.0),
            cutoff: Ramp::new(1_800.0f32.ln()),
            res: Ramp::new(0.15),
            env_amt: Ramp::new(1.5),
            key_track: Ramp::new(0.5),
            vel_filter: Ramp::new(0.3),
            lfo_rate: Ramp::new(4.5f32.ln()),
            lfo_depth: Ramp::new(0.0),
            gain: Ramp::new(0.0),
            uni_spread: 0.7,
            lfo: Lfo::new(0x1F0),
            lfo_dt: 0.0,
            lfo_value: 0.0,
            last_key: None,
            out_gain: 1.0,
        }
    }

    fn apply_envelopes(&mut self) {
        let a = self.amp_adsr;
        let f = self.flt_adsr;
        for v in &mut self.pool.voices {
            v.amp_env.set(a[0], a[1], a[2], a[3], false);
            v.flt_env.set(f[0], f[1], f[2], f[3], false);
        }
    }

    /// Recomputes shared values from the smoothed parameters.
    fn derive(&mut self) {
        let depth = self.lfo_depth.get();
        let lfo = self.lfo_value;
        let m = &mut self.m;
        m.sr = self.sr;
        m.n_uni = self.unison;
        let mix = self.mix.get();
        m.mix1 = (1.0 - mix) * 0.8;
        m.mix2 = mix * 0.8;
        m.pw = self.pw.get();
        m.semi = self.semi.get();
        m.detune2 = self.detune2.get();
        m.uni_detune = self.uni_detune.get();
        m.sub_gain = self.sub.get() * 0.5;
        m.noise_gain = self.noise.get() * 0.35;
        m.cutoff = self.cutoff.get().exp();
        m.res = self.res.get();
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
        self.glide.tick(k);
        self.cutoff.tick(k);
        self.res.tick(k);
        self.env_amt.tick(k);
        self.key_track.tick(k);
        self.vel_filter.tick(k);
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
}

impl Synth for Poly {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(20, 16, StealPolicy::Oldest, sample_rate, || {
            PolyVoice::new(sample_rate)
        });
        self.ctrl = 0;
        self.m.sr = sample_rate;
        self.apply_envelopes();
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
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
            UNISON => self.unison = (v as usize + 1).clamp(1, MAX_UNISON),
            UNISON_DETUNE => self.uni_detune.set(v, imm),
            UNISON_SPREAD => self.uni_spread = v,
            GLIDE => self.glide.set(v, imm),
            FILTER_TYPE => self.m.ftype = v as usize,
            CUTOFF => self.cutoff.set(v.ln(), imm),
            RESONANCE => self.res.set(v, imm),
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
        let slot = self.pool.begin(key, id);
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
        let vel_amp = vel_gain(velocity, self.vel_amp);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.cur_key = glide_from;
        v.target_key = target;
        v.velocity = velocity;
        v.vel_gain = vel_amp * 0.9;
        v.noise.reseed(seed);
        v.fade.reset();
        let norm = 1.0 / (n as f32).sqrt();
        let mut rng = Noise::new(seed ^ 0xA5A5);
        for u in 0..MAX_UNISON {
            let pos = if n > 1 && u < n {
                -1.0 + 2.0 * u as f32 / (n - 1) as f32
            } else {
                0.0
            };
            v.pos[u] = pos;
            let (gl, gr) = pan_gains(pos * spread);
            // Unused unison slots stay silent even if the unison count is
            // raised while this note is still sounding.
            let used = if u < n { norm } else { 0.0 };
            v.gl[u] = gl * used;
            v.gr[u] = gr * used;
            let phase = if u == 0 { 0.0 } else { rng.uniform() };
            v.osc1[u] = Osc::with_phase(phase);
            v.osc2[u] = Osc::with_phase(if u == 0 { 0.0 } else { rng.uniform() });
        }
        v.sub = Osc::new();
        v.svf_l.reset();
        v.svf_r.reset();
        v.lad_l.reset();
        v.lad_r.reset();
        v.amp_env.reset();
        v.flt_env.reset();
        v.amp_env.note_on();
        v.flt_env.note_on();
        v.control(&m);
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        self.pool.release_id(id, |v| {
            v.amp_env.note_off();
            v.flt_env.note_off();
        });
    }

    fn all_notes_off(&mut self) {
        self.pool.release_all(|v| {
            v.amp_env.note_off();
            v.flt_env.note_off();
        });
    }

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
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
            let m = self.m;
            let (mut l, mut r) = (0.0, 0.0);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick(&m);
                l += a;
                r += b;
            }
            let trem = if self.lfo_target == 2 {
                1.0 - self.lfo_depth.get() * (0.5 - 0.5 * self.lfo_value)
            } else {
                1.0
            };
            let g = self.out_gain * trem;
            left[n] = l * g;
            right[n] = r * g;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
