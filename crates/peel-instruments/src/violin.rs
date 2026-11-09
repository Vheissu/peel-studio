//! `peel.strings.violin`: a bowed (and plucked) violin built on a digital
//! waveguide.
//!
//! Each string is two delay segments either side of the bow point. A
//! friction curve driven by bow velocity and pressure couples them, the
//! bridge end reflects through a one-pole loss filter (so high partials die
//! faster than low ones) and the nut end reflects with an inversion. The
//! bridge signal feeds a violin body model: a cascade of resonant filters at
//! the main body and air modes, the bridge hill around 3 kHz and a gentle
//! roll-off above.
//!
//! Playing: bow attack follows velocity, vibrato fades in after a delay with a
//! little rate and depth randomness per note, and a slow pitch drift plus a
//! small scoop on the attack keep a single note from sounding mechanical.
//! `voice.ensemble` stacks up to six detuned, slightly delayed strings per
//! note, spread in stereo, for a section sound. Legato mode plays a solo line
//! on one string with portamento, and pizzicato replaces the bow with a
//! plucked excitation and a short decay. Tremolo modulates the bow force.

use peel_dsp::biquad::{Biquad, Coeffs, FilterKind};
use peel_dsp::delay::DelayLine;
use peel_dsp::noise::Noise;
use peel_dsp::osc::fast_sin;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz, pan_gains, sanitize};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, lerp, ramp_coef, vel_gain,
};

const STYLE: ParamId = 1;
const PRESSURE: ParamId = 2;
const POSITION: ParamId = 3;
const VELOCITY: ParamId = 4;
const ATTACK: ParamId = 5;
const RELEASE: ParamId = 6;
const NOISE: ParamId = 7;
const PLUCK_DECAY: ParamId = 8;
const BODY_SIZE: ParamId = 10;
const BODY_RESONANCE: ParamId = 11;
const BODY_BRIGHT: ParamId = 12;
const VIB_RATE: ParamId = 20;
const VIB_DEPTH: ParamId = 21;
const VIB_DELAY: ParamId = 22;
const VIB_FADE: ParamId = 23;
const VIB_VARIATION: ParamId = 24;
const TREM_RATE: ParamId = 25;
const TREM_DEPTH: ParamId = 26;
const ENSEMBLE: ParamId = 30;
const MODE: ParamId = 31;
const GLIDE: ParamId = 32;
const WIDTH: ParamId = 33;
const LEVEL: ParamId = 40;

static PARAMS: [ParamDescriptor; 23] = [
    ParamDescriptor::choice(
        STYLE,
        "bow.style",
        "Style",
        "Bow",
        &["Arco", "Pizzicato"],
        0,
    ),
    ParamDescriptor::linear(
        PRESSURE,
        "bow.pressure",
        "Pressure",
        "Bow",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        POSITION,
        "bow.position",
        "Position",
        "Bow",
        0.05,
        0.3,
        0.13,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        VELOCITY,
        "bow.velocity",
        "Velocity",
        "Bow",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ATTACK,
        "bow.attack",
        "Attack",
        "Bow",
        0.01,
        0.5,
        0.08,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        RELEASE,
        "bow.release",
        "Release",
        "Bow",
        0.03,
        2.0,
        0.15,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        NOISE,
        "bow.noise",
        "Noise",
        "Bow",
        0.0,
        1.0,
        0.25,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        PLUCK_DECAY,
        "pluck.decay",
        "Pluck Decay",
        "Bow",
        0.1,
        4.0,
        0.9,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        BODY_SIZE,
        "body.size",
        "Size",
        "Body",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BODY_RESONANCE,
        "body.resonance",
        "Resonance",
        "Body",
        0.0,
        1.0,
        0.6,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BODY_BRIGHT,
        "body.brightness",
        "Brightness",
        "Body",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        VIB_RATE,
        "vibrato.rate",
        "Rate",
        "Vibrato",
        3.0,
        9.0,
        5.5,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        VIB_DEPTH,
        "vibrato.depth",
        "Depth",
        "Vibrato",
        0.0,
        100.0,
        25.0,
        Unit::Cents,
    ),
    ParamDescriptor::linear(
        VIB_DELAY,
        "vibrato.delay",
        "Delay",
        "Vibrato",
        0.0,
        2.0,
        0.35,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        VIB_FADE,
        "vibrato.fade",
        "Fade In",
        "Vibrato",
        0.05,
        2.0,
        0.5,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        VIB_VARIATION,
        "vibrato.variation",
        "Variation",
        "Vibrato",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        TREM_RATE,
        "trem.rate",
        "Tremolo Rate",
        "Vibrato",
        2.0,
        16.0,
        9.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        TREM_DEPTH,
        "trem.depth",
        "Tremolo Depth",
        "Vibrato",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ENSEMBLE,
        "voice.ensemble",
        "Ensemble",
        "Voice",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::choice(MODE, "voice.mode", "Mode", "Voice", &["Poly", "Legato"], 0),
    ParamDescriptor::linear(
        GLIDE,
        "voice.glide",
        "Glide",
        "Voice",
        0.0,
        1.0,
        0.08,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        WIDTH,
        "voice.width",
        "Width",
        "Voice",
        0.0,
        1.0,
        0.7,
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
    type_id: "peel.strings.violin",
    name: "Violin",
    description: "Bowed-string waveguide violin with body, vibrato, ensemble and pizzicato",
    category: "Strings",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Solo",
        params: &[
            ("bow.style", 0.0),
            ("bow.pressure", 0.5),
            ("bow.position", 0.13),
            ("bow.velocity", 0.7),
            ("bow.attack", 0.08),
            ("bow.release", 0.15),
            ("bow.noise", 0.25),
            ("pluck.decay", 0.9),
            ("body.size", 0.5),
            ("body.resonance", 0.6),
            ("body.brightness", 0.5),
            ("vibrato.rate", 5.5),
            ("vibrato.depth", 25.0),
            ("vibrato.delay", 0.35),
            ("vibrato.fade", 0.5),
            ("vibrato.variation", 0.5),
            ("trem.rate", 9.0),
            ("trem.depth", 0.0),
            ("voice.ensemble", 0.0),
            ("voice.mode", 0.0),
            ("voice.glide", 0.08),
            ("voice.width", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Ensemble",
        params: &[
            ("bow.pressure", 0.45),
            ("bow.attack", 0.16),
            ("bow.release", 0.3),
            ("bow.noise", 0.15),
            ("body.size", 0.6),
            ("body.resonance", 0.55),
            ("body.brightness", 0.42),
            ("vibrato.depth", 18.0),
            ("vibrato.delay", 0.5),
            ("vibrato.fade", 0.8),
            ("vibrato.variation", 0.9),
            ("voice.ensemble", 0.85),
            ("voice.width", 0.9),
            ("out.level", -2.0),
        ],
    },
    Preset {
        name: "Pizzicato",
        params: &[
            ("bow.style", 1.0),
            ("bow.position", 0.18),
            ("bow.velocity", 0.8),
            ("pluck.decay", 0.7),
            ("body.resonance", 0.8),
            ("body.brightness", 0.6),
            ("vibrato.depth", 0.0),
            ("out.level", 1.0),
        ],
    },
    Preset {
        name: "Legato Lead",
        params: &[
            ("bow.pressure", 0.55),
            ("bow.attack", 0.06),
            ("bow.release", 0.2),
            ("body.brightness", 0.6),
            ("vibrato.rate", 5.8),
            ("vibrato.depth", 38.0),
            ("vibrato.delay", 0.15),
            ("vibrato.fade", 0.35),
            ("vibrato.variation", 0.7),
            ("voice.ensemble", 0.1),
            ("voice.mode", 1.0),
            ("voice.glide", 0.12),
            ("voice.width", 0.4),
        ],
    },
    Preset {
        name: "Tremolo",
        params: &[
            ("bow.pressure", 0.6),
            ("bow.position", 0.1),
            ("bow.attack", 0.03),
            ("bow.release", 0.25),
            ("bow.noise", 0.4),
            ("body.brightness", 0.6),
            ("vibrato.depth", 0.0),
            ("trem.rate", 10.5),
            ("trem.depth", 0.85),
            ("voice.ensemble", 0.5),
            ("voice.width", 0.8),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Violin::new()))
}

/// Most strings stacked on one note by the ensemble control.
const MAX_STRINGS: usize = 6;
/// Notes sounding at once and spare voices for releasing tails.
const POLYPHONY: usize = 8;
const CAPACITY: usize = 10;
/// Lowest string frequency the waveguide is built for; sizes the delay lines.
const MIN_HZ: f32 = 30.0;
/// Output scaling of the waveguide.
const ARCO_TRIM: f32 = 0.3;
const PIZZ_TRIM: f32 = 0.45;
/// Loop length correction (samples) for the read/write order and the bow.
const LOOP_TRIM: f32 = -0.17;
/// Ring-down of a bowed string once the bow lifts, seconds to -60 dB.
const ARCO_T60: f32 = 3.0;

/// Per-string detune (cents), onset delay (ms) and pan, by string index.
const DETUNE: [f32; MAX_STRINGS] = [0.0, 7.0, -8.0, 13.0, -12.0, 4.0];
const ONSET_MS: [f32; MAX_STRINGS] = [0.0, 9.0, 17.0, 26.0, 35.0, 44.0];
const PAN: [f32; MAX_STRINGS] = [0.0, -0.7, 0.7, -0.35, 0.35, 0.0];

/// Body modes: (frequency Hz, Q, gain dB at full resonance).
const BODY_MODES: [(f32, f32, f32); 7] = [
    (275.0, 5.0, 6.0),
    (450.0, 7.0, 5.0),
    (560.0, 8.0, 3.0),
    (1000.0, 3.0, 4.0),
    (1800.0, 2.5, 2.0),
    (2900.0, 1.5, 6.0),
    (4200.0, 2.0, 2.0),
];
const BODY_FILTERS: usize = BODY_MODES.len() + 2;

/// Values derived from parameters once per control interval.
#[derive(Clone, Copy)]
struct Ctl {
    sr: f32,
    dt: f32,
    slope: f32,
    position: f32,
    pole: f32,
    vib_rate: f32,
    vib_depth: f32,
    vib_delay: f32,
    vib_fade: f32,
    variation: f32,
    pluck_t60: f32,
    glide_k: f32,
    noise: f32,
}

/// One waveguide string (a member of the ensemble for a note).
struct Str {
    nut: DelayLine,
    bridge: DelayLine,
    d_nut: f32,
    d_bridge: f32,
    pole: f32,
    loop_gain: f32,
    lp: f32,
    dc_x: f32,
    dc_y: f32,
    wait: u32,
    e: f32,
    e_inc: f32,
    og: f32,
    onset: f32,
    nlp: f32,
    rng: Noise,
    detune: f32,
    gl: f32,
    gr: f32,
    vib_ph: f32,
    vib_rate: f32,
    vib_depth: f32,
    drift: f32,
    pluck_n: u32,
    pluck_len: u32,
    pluck_amp: f32,
}

impl Str {
    fn new(max_delay: usize, seed: u32) -> Str {
        Str {
            nut: DelayLine::new(max_delay),
            bridge: DelayLine::new(max_delay),
            d_nut: 10.0,
            d_bridge: 3.0,
            pole: 0.5,
            loop_gain: 0.99,
            lp: 0.0,
            dc_x: 0.0,
            dc_y: 0.0,
            wait: 0,
            e: 0.0,
            e_inc: 0.001,
            og: 1.0,
            onset: 1.0,
            nlp: 0.0,
            rng: Noise::new(seed),
            detune: 0.0,
            gl: 1.0,
            gr: 1.0,
            vib_ph: 0.0,
            vib_rate: 1.0,
            vib_depth: 1.0,
            drift: 0.0,
            pluck_n: u32::MAX,
            pluck_len: 1,
            pluck_amp: 0.0,
        }
    }

    fn clear(&mut self) {
        self.nut.clear();
        self.bridge.clear();
        self.lp = 0.0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
    }

    /// One sample. `bow` is the bow velocity before this string's envelope;
    /// `rel` is the release coefficient once the bow has lifted.
    #[inline]
    fn tick(&mut self, arco: bool, slope: f32, bow: f32, noise: f32, rel: Option<f32>) -> f32 {
        let nut_out = self.nut.read_hermite(self.d_nut);
        let br_out = self.bridge.read_hermite(self.d_bridge);
        let filt = (1.0 - self.pole) * self.loop_gain * br_out + self.pole * self.lp;
        self.lp = sanitize(filt);
        // Strings cannot carry DC: block it inside the loop.
        let dc = filt - self.dc_x + 0.9995 * self.dc_y;
        self.dc_x = filt;
        self.dc_y = sanitize(dc);
        let bridge_refl = -dc;
        let nut_refl = -nut_out;
        let mut inject = 0.0;
        if arco {
            match rel {
                Some(c) => {
                    self.e *= c;
                    self.og *= c;
                }
                None => self.e = (self.e + self.e_inc).min(1.0),
            }
            self.onset *= 0.9995;
            let n = self.rng.white();
            self.nlp += 0.35 * (n - self.nlp);
            let v = bow * self.e * (1.0 + noise * 0.45 * (1.0 + 4.0 * self.onset) * self.nlp);
            let string_vel = bridge_refl + nut_refl;
            let diff = v - string_vel;
            let x = (diff * slope).abs() + 0.75;
            let x2 = x * x;
            let table = (1.0 / (x2 * x2)).min(1.0);
            inject = diff * table;
        } else if self.pluck_n < self.pluck_len {
            let ph = self.pluck_n as f32 / self.pluck_len as f32;
            inject = self.pluck_amp * fast_sin(ph);
            self.pluck_n += 1;
        }
        self.nut.push(bridge_refl + inject);
        self.bridge.push(nut_refl + inject);
        br_out
    }
}

struct VioVoice {
    active: bool,
    releasing: bool,
    id: u32,
    pizz: bool,
    n: usize,
    strings: [Str; MAX_STRINGS],
    st: f32,
    tgt_st: f32,
    age_s: f32,
    age_samples: u32,
    bow_vel: f32,
    slope_adj: f32,
    rel_coef: f32,
    follow: f32,
    fade: Fade,
}

impl VioVoice {
    fn new(max_delay: usize, seed: u32) -> VioVoice {
        VioVoice {
            active: false,
            releasing: false,
            id: 0,
            pizz: false,
            n: 1,
            strings: std::array::from_fn(|i| {
                Str::new(max_delay, seed.wrapping_mul(8).wrapping_add(i as u32))
            }),
            st: 60.0,
            tgt_st: 60.0,
            age_s: 0.0,
            age_samples: 0,
            bow_vel: 0.1,
            slope_adj: 0.0,
            rel_coef: 0.999,
            follow: 0.0,
            fade: Fade::new(),
        }
    }

    /// Control-rate update: pitch, vibrato and per-string delay lengths.
    fn update(&mut self, c: &Ctl) {
        self.st += (self.tgt_st - self.st) * c.glide_k;
        if (self.tgt_st - self.st).abs() < 1.0e-3 {
            self.st = self.tgt_st;
        }
        self.age_s += c.dt;
        let t = ((self.age_s - c.vib_delay) / c.vib_fade).clamp(0.0, 1.0);
        let vib_env = t * t * (3.0 - 2.0 * t);
        let base_hz = key_to_hz(self.st);
        let scoop = if self.pizz {
            0.0
        } else {
            -14.0 * c.variation * (-self.age_s / 0.045).exp()
        };
        for s in &mut self.strings[..self.n] {
            s.vib_ph += s.vib_rate * c.vib_rate * (1.0 + 0.08 * c.variation * s.drift) * c.dt;
            s.vib_ph -= s.vib_ph.floor();
            s.drift += (s.rng.white() - s.drift) * 0.03;
            let cents = s.detune
                + fast_sin(s.vib_ph) * c.vib_depth * vib_env * s.vib_depth
                + s.drift * (0.6 + 1.4 * c.variation)
                + scoop;
            let hz = (base_hz * (cents * (std::f32::consts::LN_2 / 1200.0)).exp())
                .clamp(MIN_HZ, c.sr / 6.0);
            let w = std::f32::consts::TAU * hz / c.sr;
            let (sw, cw) = w.sin_cos();
            s.pole = c.pole;
            // Phase delay of the bridge loss filter at the fundamental.
            let pd = (s.pole * sw).atan2(1.0 - s.pole * cw) / w;
            let total = (c.sr / hz - pd - LOOP_TRIM).max(4.0);
            let d_br = (c.position * total).max(2.0).min(total - 2.0);
            s.d_bridge = d_br;
            s.d_nut = total - d_br;
            let t60 = if self.pizz { c.pluck_t60 } else { ARCO_T60 };
            s.loop_gain = (-6.9078 / (t60 * hz)).exp();
        }
    }

    #[inline]
    fn tick(&mut self, c: &Ctl, trem: f32, trem_out: f32) -> (f32, f32) {
        let rel = if self.releasing && !self.pizz {
            Some(self.rel_coef)
        } else {
            None
        };
        let bow = self.bow_vel * trem;
        let (mut l, mut r) = (0.0, 0.0);
        let mut max_og = 0.0f32;
        for s in &mut self.strings[..self.n] {
            if s.wait > 0 {
                s.wait -= 1;
                max_og = max_og.max(1.0);
                continue;
            }
            let o = s.tick(
                !self.pizz,
                (c.slope - self.slope_adj).max(1.0),
                bow,
                c.noise,
                rel,
            );
            max_og = max_og.max(s.og);
            l += o * s.gl * s.og;
            r += o * s.gr * s.og;
        }
        let g = self.fade.tick() * if self.pizz { 1.0 } else { trem_out };
        let (l, r) = (l * g, r * g);
        let a = l.abs().max(r.abs());
        self.follow = if a > self.follow {
            a
        } else {
            self.follow * 0.9997
        };
        self.age_samples = self.age_samples.saturating_add(1);
        let done = if self.pizz {
            self.age_samples > 2_400 && self.follow < 2.0e-6
        } else {
            self.releasing && max_og < 1.0e-4
        };
        if done || self.fade.is_done() {
            self.active = false;
        }
        (l, r)
    }
}

impl PoolVoice for VioVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.follow
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

#[derive(Clone, Copy)]
struct Held {
    key: u8,
    id: u32,
}

pub(crate) struct Violin {
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    pool: Pool<VioVoice>,
    seed: u32,
    // Parameters.
    style: usize,
    pressure: Ramp,
    position: Ramp,
    vel_sens: f32,
    attack: f32,
    release: f32,
    noise: f32,
    pluck_decay: f32,
    size: f32,
    resonance: f32,
    bright: Ramp,
    vib_rate: f32,
    vib_depth: Ramp,
    vib_delay: f32,
    vib_fade: f32,
    variation: f32,
    trem_rate: f32,
    trem_depth: Ramp,
    ensemble: f32,
    mode: usize,
    glide: f32,
    width: f32,
    level: Ramp,
    // Derived.
    ctl: Ctl,
    out_gain: f32,
    trem_ph: f32,
    // Legato bookkeeping.
    stack: [Held; 16],
    stack_len: usize,
    leg_slot: usize,
    leg_id: u32,
    // Body.
    body_l: [Biquad; BODY_FILTERS],
    body_r: [Biquad; BODY_FILTERS],
    tail: u32,
}

fn max_delay_for(sr: f32) -> usize {
    (sr / MIN_HZ) as usize + 8
}

impl Violin {
    pub(crate) fn new() -> Violin {
        let mut v = Violin {
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            pool: Self::make_pool(48_000.0),
            seed: 0x2545_F491,
            style: 0,
            pressure: Ramp::new(0.5),
            position: Ramp::new(0.13),
            vel_sens: 0.7,
            attack: 0.08,
            release: 0.15,
            noise: 0.25,
            pluck_decay: 0.9,
            size: 0.5,
            resonance: 0.6,
            bright: Ramp::new(0.5),
            vib_rate: 5.5,
            vib_depth: Ramp::new(25.0),
            vib_delay: 0.35,
            vib_fade: 0.5,
            variation: 0.5,
            trem_rate: 9.0,
            trem_depth: Ramp::new(0.0),
            ensemble: 0.0,
            mode: 0,
            glide: 0.08,
            width: 0.7,
            level: Ramp::new(0.0),
            ctl: Ctl {
                sr: 48_000.0,
                dt: CTRL as f32 / 48_000.0,
                slope: 3.0,
                position: 0.13,
                pole: 0.5,
                vib_rate: 5.5,
                vib_depth: 25.0,
                vib_delay: 0.35,
                vib_fade: 0.5,
                variation: 0.5,
                pluck_t60: 0.9,
                glide_k: 1.0,
                noise: 0.25,
            },
            out_gain: ARCO_TRIM,
            trem_ph: 0.0,
            stack: [Held { key: 0, id: 0 }; 16],
            stack_len: 0,
            leg_slot: 0,
            leg_id: 0,
            body_l: [Biquad::default(); BODY_FILTERS],
            body_r: [Biquad::default(); BODY_FILTERS],
            tail: u32::MAX,
        };
        v.design_body();
        v.derive();
        v
    }

    fn make_pool(sr: f32) -> Pool<VioVoice> {
        let max = max_delay_for(sr);
        let n = std::cell::Cell::new(0u32);
        Pool::new(CAPACITY, POLYPHONY, StealPolicy::Quietest, sr, || {
            n.set(n.get() + 1);
            VioVoice::new(max, n.get())
        })
    }

    fn design_body(&mut self) {
        let scale = 2f32.powf((0.5 - self.size) * 0.5);
        let sr = self.sr;
        let mut set = |i: usize, c: Coeffs| {
            self.body_l[i].coeffs = c;
            self.body_r[i].coeffs = c;
        };
        set(0, Coeffs::design(FilterKind::HighPass, sr, 90.0, 0.7, 0.0));
        for (i, (f, q, db)) in BODY_MODES.iter().enumerate() {
            let g = db * self.resonance * 1.5;
            set(
                i + 1,
                Coeffs::design(FilterKind::Peak, sr, f * scale, *q, g),
            );
        }
        let lp = 4_000.0 + 5_000.0 * self.bright.get().clamp(0.0, 1.0);
        set(
            BODY_FILTERS - 1,
            Coeffs::design(FilterKind::LowPass, sr, lp, 0.6, 0.0),
        );
    }

    fn derive(&mut self) {
        let sr = self.sr;
        let arco = self.style == 0;
        self.ctl = Ctl {
            sr,
            dt: CTRL as f32 / sr,
            slope: 5.0 - 4.0 * self.pressure.get(),
            position: self.position.get(),
            pole: lerp(0.8, 0.25, self.bright.get()).powf(48_000.0 / sr),
            vib_rate: self.vib_rate,
            vib_depth: self.vib_depth.get(),
            vib_delay: self.vib_delay,
            vib_fade: self.vib_fade,
            variation: self.variation,
            pluck_t60: self.pluck_decay,
            glide_k: if self.glide < 0.002 {
                1.0
            } else {
                1.0 - (-(CTRL as f32) / (self.glide * 0.3 * sr)).exp()
            },
            noise: self.noise,
        };
        self.out_gain = db_to_gain(self.level.get()) * if arco { ARCO_TRIM } else { PIZZ_TRIM };
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.pressure.tick(k);
        self.position.tick(k);
        self.bright.tick(k);
        self.vib_depth.tick(k);
        self.trem_depth.tick(k);
        self.level.tick(k);
        self.derive();
        let ctl = self.ctl;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.update(&ctl);
        }
        self.pool.sync();
    }

    fn rand(&mut self) -> f32 {
        // Cheap xorshift for per-note variation; deterministic per instance.
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x >> 8) as f32 / 16_777_216.0
    }

    fn start_voice(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.pool.begin(key, id);
        let sr = self.sr;
        let ens = self.ensemble;
        let n = if ens < 0.05 {
            1
        } else {
            (2.0 + ens * 4.99) as usize
        }
        .min(MAX_STRINGS);
        let pizz = self.style == 1;
        let weight = 1.0 / (n as f32).sqrt();
        let amount = ens.powf(0.7);
        let hz0 = key_to_hz(key as f32).clamp(MIN_HZ, sr / 6.0);
        let mut rnd = [[0.0f32; 4]; MAX_STRINGS];
        for r in rnd.iter_mut() {
            for x in r.iter_mut() {
                *x = self.rand();
            }
        }
        let bow_vel = bow_speed(key, velocity, self.vel_sens);
        let rel_coef = (-6.9078 / (self.release.max(0.005) * sr)).exp();
        let attack = self.attack * lerp(1.7, 0.4, velocity);
        let (variation, width) = (self.variation, self.width);
        let pole = self.ctl.pole;
        let pluck_amp = 0.9 * vel_gain(velocity, self.vel_sens);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.releasing = false;
        v.id = id;
        v.pizz = pizz;
        v.n = n;
        v.st = key as f32;
        v.tgt_st = key as f32;
        v.age_s = 0.0;
        v.age_samples = 0;
        v.bow_vel = bow_vel;
        v.slope_adj = slope_adjust(key);
        v.rel_coef = rel_coef;
        v.follow = 0.0;
        v.fade.reset();
        for (i, s) in v.strings[..n].iter_mut().enumerate() {
            let r = rnd[i];
            s.clear();
            s.og = 1.0;
            s.e = 0.0;
            s.onset = 1.0;
            s.nlp = 0.0;
            s.drift = 0.0;
            s.wait = (ONSET_MS[i] * amount * (0.5 + 0.5 * r[0]) * 0.001 * sr) as u32;
            let att = attack * (1.0 + 0.6 * amount * i as f32 / MAX_STRINGS as f32);
            s.e_inc = 1.0 / (att * sr).max(1.0);
            s.detune = if i == 0 {
                (r[1] - 0.5) * 4.0 * amount
            } else {
                DETUNE[i] * amount * (0.7 + 0.6 * r[1])
            };
            let (pl, pr) = pan_gains(PAN[i] * width);
            s.gl = pl * weight;
            s.gr = pr * weight;
            s.vib_ph = r[2];
            s.vib_rate = 1.0 + (r[3] - 0.5) * 0.3 * variation;
            s.vib_depth = 1.0 + (r[0] - 0.5) * 0.6 * variation;
            s.pole = pole;
            let period = sr / hz0;
            s.pluck_len = (period * (0.3 - 0.2 * velocity)).max(3.0) as u32;
            s.pluck_amp = pluck_amp;
            s.pluck_n = if pizz { 0 } else { u32::MAX };
        }
        // Make the delay lengths valid before the first sample.
        let ctl = self.ctl;
        v.update(&ctl);
        v.age_s = 0.0;
        self.leg_slot = slot;
        self.leg_id = id;
    }

    fn legato_slot(&self) -> Option<usize> {
        if self.stack_len == 0 {
            return None;
        }
        let v = self.pool.voices.get(self.leg_slot)?;
        (v.active && !v.releasing && v.id == self.leg_id && !v.pizz).then_some(self.leg_slot)
    }

    fn release_voice(&mut self, id: u32) {
        self.pool.release_id(id, |v| v.releasing = true);
    }
}

impl Synth for Violin {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Self::make_pool(sample_rate);
        self.ctrl = 0;
        self.stack_len = 0;
        self.tail = u32::MAX;
        self.design_body();
        self.derive();
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
        self.stack_len = 0;
        self.tail = u32::MAX;
        for b in self.body_l.iter_mut().chain(self.body_r.iter_mut()) {
            b.reset();
        }
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            STYLE => self.style = v as usize,
            PRESSURE => self.pressure.set(v, imm),
            POSITION => self.position.set(v, imm),
            VELOCITY => self.vel_sens = v,
            ATTACK => self.attack = v,
            RELEASE => {
                self.release = v;
                let c = (-6.9078 / (v.max(0.005) * self.sr)).exp();
                for voice in &mut self.pool.voices {
                    voice.rel_coef = c;
                }
            }
            NOISE => self.noise = v,
            PLUCK_DECAY => self.pluck_decay = v,
            BODY_SIZE => {
                self.size = v;
                self.design_body();
            }
            BODY_RESONANCE => {
                self.resonance = v;
                self.design_body();
            }
            BODY_BRIGHT => {
                self.bright.set(v, imm);
                if imm {
                    self.bright.tick(1.0);
                }
                self.design_body();
            }
            VIB_RATE => self.vib_rate = v,
            VIB_DEPTH => self.vib_depth.set(v, imm),
            VIB_DELAY => self.vib_delay = v,
            VIB_FADE => self.vib_fade = v,
            VIB_VARIATION => self.variation = v,
            TREM_RATE => self.trem_rate = v,
            TREM_DEPTH => self.trem_depth.set(v, imm),
            ENSEMBLE => self.ensemble = v,
            MODE => self.mode = v as usize,
            GLIDE => self.glide = v,
            WIDTH => self.width = v,
            LEVEL => self.level.set(v, imm),
            _ => {}
        }
        if imm {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        if self.mode == 1 && self.style == 0 {
            if let Some(slot) = self.legato_slot() {
                if self.stack_len == self.stack.len() {
                    self.stack.copy_within(1.., 0);
                    self.stack_len -= 1;
                }
                self.stack[self.stack_len] = Held { key, id };
                self.stack_len += 1;
                let bow = bow_speed(key, velocity, self.vel_sens);
                let snap = self.glide < 0.002;
                let v = &mut self.pool.voices[slot];
                v.tgt_st = key as f32;
                v.bow_vel = bow;
                v.slope_adj = slope_adjust(key);
                if snap {
                    v.st = v.tgt_st;
                }
                return;
            }
            self.stack[0] = Held { key, id };
            self.stack_len = 1;
        }
        self.start_voice(key, velocity, id);
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        if let Some(pos) = self.stack[..self.stack_len].iter().position(|h| h.id == id) {
            let was_top = pos + 1 == self.stack_len;
            self.stack.copy_within(pos + 1..self.stack_len, pos);
            self.stack_len -= 1;
            if self.stack_len == 0 {
                let leg = self.leg_id;
                self.release_voice(leg);
            } else if was_top {
                let key = self.stack[self.stack_len - 1].key;
                let snap = self.glide < 0.002;
                if let Some(v) = self.pool.voices.get_mut(self.leg_slot)
                    && v.active
                    && v.id == self.leg_id
                {
                    v.tgt_st = key as f32;
                    if snap {
                        v.st = v.tgt_st;
                    }
                }
            }
            return;
        }
        self.release_voice(id);
    }

    fn all_notes_off(&mut self) {
        self.stack_len = 0;
        self.pool.release_all(|v| v.releasing = true);
    }

    fn all_sound_off(&mut self) {
        self.stack_len = 0;
        self.pool.kill_all();
        self.tail = u32::MAX;
        for b in self.body_l.iter_mut().chain(self.body_r.iter_mut()) {
            b.reset();
        }
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let tail_len = (0.25 * self.sr) as u32;
        let trem_inc = self.trem_rate / self.sr;
        for i in 0..n {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            if self.pool.is_silent() {
                if self.tail < tail_len {
                    self.tail += 1;
                    if self.tail == tail_len {
                        for b in self.body_l.iter_mut().chain(self.body_r.iter_mut()) {
                            b.reset();
                        }
                    }
                }
                if self.tail >= tail_len {
                    left[i] = 0.0;
                    right[i] = 0.0;
                    continue;
                }
            } else {
                self.tail = 0;
            }
            self.trem_ph += trem_inc;
            self.trem_ph -= self.trem_ph.floor();
            let u = 0.5 + 0.5 * fast_sin(self.trem_ph - 0.25);
            let td = self.trem_depth.get();
            let trem = 1.0 - td * 0.9 * (1.0 - u);
            let trem_out = 1.0 - td * 0.35 * (1.0 - u);
            let ctl = self.ctl;
            let (mut l, mut r) = (0.0, 0.0);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick(&ctl, trem, trem_out);
                l += a;
                r += b;
            }
            for b in &mut self.body_l {
                l = b.process(l);
            }
            for b in &mut self.body_r {
                r = b.process(r);
            }
            left[i] = l * self.out_gain;
            right[i] = r * self.out_gain;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent() && self.tail >= (0.25 * self.sr) as u32
    }
}

/// Bow speed for a note. High notes need a slower bow or the string jumps to
/// a harmonic, so the speed falls with pitch.
fn bow_speed(key: u8, velocity: f32, sens: f32) -> f32 {
    let hz = key_to_hz(key as f32);
    let pitch = (hz / 330.0).powf(-0.45).clamp(0.3, 1.0);
    (0.03 + 0.17 * vel_gain(velocity, sens)) * pitch
}

/// Extra bow pressure for high notes (a flatter friction curve), as a
/// reduction of the friction slope.
fn slope_adjust(key: u8) -> f32 {
    let hz = key_to_hz(key as f32);
    (hz / 900.0).log2().clamp(0.0, 1.6) * 0.75
}
