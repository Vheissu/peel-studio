//! `peel.strings.violin`: a bowed (and plucked) violin, viola and cello built
//! on a digital waveguide with a modal body.
//!
//! Each string is two delay segments either side of the bow point, the
//! classic folded waveguide. The bow couples them through a hyperbolic
//! stick-slip friction law (static coefficient higher than dynamic, falling
//! with sliding speed) that is solved exactly each sample with a stick/slip
//! state, after McIntyre, Schumacher and Woodhouse. That gives real Helmholtz
//! motion with the Schelleng limits: too little force whistles and
//! surface-sounds, too much chokes, and a note starts with the scratchy,
//! multi-slip onset of a bow that is still gaining speed. Bow force follows
//! bow speed and the bow point, so the controls behave like a player's.
//!
//! The bridge signal feeds a violin body: A0 and the B1 pair, a run of
//! lower-middle plate and cavity modes, the broad bridge hill around 2-3 kHz
//! and dense weaker modes above. Because the body is fixed, vibrato sweeps
//! the string partials across its resonances and produces the amplitude and
//! colour shimmer of a real violin. The body scales down with the register
//! of the notes being played, so low chords read as viola and cello rather
//! than a violin played low. Left and right use slightly different bodies.
//!
//! Playing: vibrato starts late with a rate and depth that wander, pitch
//! drifts, and a small scoop on the attack keeps a note human.
//! `voice.ensemble` stacks up to eight independent players on each note:
//! different entry times (a few ms to 30 ms), tuning, vibrato phase, rate
//! and entry, bow force, bow noise and tremolo, spread across the stereo
//! field, with a small room around them. Legato mode plays a solo line on one
//! string with portamento. Pizzicato plucks the string. Tremolo reverses the
//! bow direction at the tremolo rate.

use peel_dsp::biquad::{Biquad, Coeffs, FilterKind};
use peel_dsp::delay::DelayLine;
use peel_dsp::noise::Noise;
use peel_dsp::osc::fast_sin;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::reverb::Fdn8;
use peel_dsp::util::{db_to_gain, key_to_hz, pan_gains, sanitize};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, Tilt, Vx, lerp, ramp_coef,
    vel_gain,
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
const BODY_REGISTER: ParamId = 13;
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
const ROOM: ParamId = 34;
const LEVEL: ParamId = 40;

static PARAMS: [ParamDescriptor; 25] = [
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
    ParamDescriptor::linear(
        BODY_REGISTER,
        "body.register",
        "Register",
        "Body",
        0.0,
        1.0,
        0.8,
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
        ROOM,
        "voice.room",
        "Room",
        "Voice",
        0.0,
        1.0,
        0.0,
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
    description: "Bowed-string waveguide strings with modal body, vibrato, section ensemble and pizzicato",
    category: "Strings",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 8] = [
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
            ("body.register", 0.8),
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
            ("voice.room", 0.0),
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
            ("body.size", 0.55),
            ("body.resonance", 0.55),
            ("body.brightness", 0.42),
            ("vibrato.depth", 18.0),
            ("vibrato.delay", 0.5),
            ("vibrato.fade", 0.8),
            ("vibrato.variation", 0.9),
            ("voice.ensemble", 0.85),
            ("voice.width", 0.9),
            ("voice.room", 0.3),
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
            ("voice.room", 0.25),
            ("out.level", 3.0),
        ],
    },
    Preset {
        name: "Section Swell",
        params: &[
            ("bow.pressure", 0.4),
            ("bow.velocity", 0.55),
            ("bow.attack", 0.45),
            ("bow.release", 0.9),
            ("bow.noise", 0.1),
            ("body.size", 0.55),
            ("body.resonance", 0.55),
            ("body.brightness", 0.4),
            ("vibrato.depth", 14.0),
            ("vibrato.delay", 0.7),
            ("vibrato.fade", 1.2),
            ("vibrato.variation", 1.0),
            ("voice.ensemble", 1.0),
            ("voice.width", 1.0),
            ("voice.room", 0.5),
            ("out.level", -2.0),
        ],
    },
    Preset {
        name: "Cinematic Low Strings",
        params: &[
            ("bow.pressure", 0.5),
            ("bow.position", 0.11),
            ("bow.attack", 0.22),
            ("bow.release", 0.6),
            ("bow.noise", 0.2),
            ("body.size", 0.4),
            ("body.resonance", 0.6),
            ("body.brightness", 0.38),
            ("body.register", 1.0),
            ("vibrato.rate", 5.0),
            ("vibrato.depth", 16.0),
            ("vibrato.delay", 0.6),
            ("vibrato.fade", 1.0),
            ("vibrato.variation", 0.9),
            ("voice.ensemble", 0.9),
            ("voice.width", 0.95),
            ("voice.room", 0.45),
            ("out.level", -1.0),
        ],
    },
    Preset {
        name: "Spiccato",
        params: &[
            ("bow.pressure", 0.7),
            ("bow.velocity", 0.8),
            ("bow.attack", 0.012),
            ("bow.release", 0.05),
            ("bow.noise", 0.4),
            ("body.brightness", 0.6),
            ("vibrato.depth", 0.0),
            ("voice.ensemble", 0.4),
            ("voice.width", 0.8),
            ("voice.room", 0.2),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Violin::new()))
}

/// Most players stacked on one note by the ensemble control.
const MAX_STRINGS: usize = 8;
/// Notes sounding at once and spare voices for releasing tails.
const POLYPHONY: usize = 8;
const CAPACITY: usize = 12;
/// Lowest string frequency the waveguide is built for; sizes the delay lines.
/// Reaches the cello low C (65 Hz) and the octave below.
const MIN_HZ: f32 = 30.0;
/// Output scaling of the waveguide.
const ARCO_TRIM: f32 = 0.3;
const PIZZ_TRIM: f32 = 0.6;
/// Loop length correction (samples) for the read/write order and the bow.
const LOOP_TRIM: f32 = 0.0;
/// Ring-down of a bowed string once the bow lifts, seconds to -60 dB.
const ARCO_T60: f32 = 3.0;

/// Static and dynamic friction coefficients of the rosin.
const MU_S: f32 = 0.9;
const MU_D: f32 = 0.35;

/// Per-player pan positions by index (scaled by width).
const PAN: [f32; MAX_STRINGS] = [0.0, -0.7, 0.7, -0.35, 0.35, -0.95, 0.95, 0.0];
/// Sign of each player's tuning offset.
const DETUNE_SIGN: [f32; MAX_STRINGS] = [1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0];

/// Body modes: (frequency Hz, Q, gain dB at full resonance). A0, the
/// corpus-bending mode, the B1 pair, the plate and cavity modes of the lower
/// middle, the bridge hill and the dense modes above.
const BODY_MODES: [(f32, f32, f32); 17] = [
    (275.0, 6.0, 10.0),
    (405.0, 8.0, 3.0),
    (470.0, 9.0, 6.0),
    (545.0, 9.0, 4.0),
    (650.0, 7.0, 1.0),
    (740.0, 10.0, -3.0),
    (860.0, 8.0, 6.0),
    (1020.0, 7.0, 2.0),
    (1260.0, 6.0, -5.0),
    (1500.0, 5.0, -1.0),
    (1850.0, 4.0, 0.0),
    (2400.0, 1.6, 4.5),
    (3100.0, 1.6, 2.5),
    (4000.0, 3.0, -4.0),
    (5200.0, 4.0, 0.0),
    (6700.0, 5.0, 1.5),
    (8500.0, 5.0, 0.5),
];
const BODY_FILTERS: usize = BODY_MODES.len() + 2;
/// Left and right bodies differ slightly in tuning, like two instruments.
const BODY_SPLIT: f32 = 0.012;

/// Values derived from parameters once per control interval.
#[derive(Clone, Copy)]
struct Ctl {
    sr: f32,
    dt: f32,
    pressure: f32,
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
    trem_depth: f32,
    trem_inc: f32,
    walk_a: f32,
    walk_g: f32,
    /// Live pitch bend in cents.
    bend_cents: f32,
}

impl Ctl {
    /// The settings for one voice: its own bend.
    fn with(&self, x: &Vx) -> Ctl {
        Ctl {
            bend_cents: x.bend * 100.0,
            ..*self
        }
    }
}

/// Bow settings shared by the strings of one note.
#[derive(Clone, Copy)]
struct Drive {
    /// Nominal bow speed.
    bow: f32,
    /// Nominal normal force in wave units (twice the bow's impedance share).
    force: f32,
    /// Velocity scale of the friction curve.
    v0: f32,
}

/// Exact solution of the hyperbolic friction law for one sample.
///
/// `diff` is bow speed minus string speed with no friction, `f2` is the bow
/// force in wave units. Returns the wave added to both directions. The string
/// sticks while friction can hold the bow and slips until the sliding speed
/// reaches zero again, so the switch has hysteresis (`slip` carries it).
#[inline]
fn friction(diff: f32, f2: f32, v0: f32, slip: &mut bool) -> f32 {
    let a = diff.abs();
    let stick = f2 * MU_S;
    if !*slip {
        if a <= stick {
            return diff;
        }
        *slip = true;
    }
    // Sliding: u + f2 * mu(u) = a with mu(u) = mu_d + (mu_s - mu_d) v0 / (v0 + u).
    let b = v0 + f2 * MU_D - a;
    let c = v0 * (stick - a);
    let disc = b * b - 4.0 * c;
    if disc < 0.0 || (c > 0.0 && b > 0.0) {
        *slip = false;
        return diff;
    }
    let u = 0.5 * (-b + disc.sqrt());
    if u <= 1.0e-7 {
        *slip = false;
        return diff;
    }
    (a - u) * diff.signum()
}

/// Normal force for a bow speed, in units of that speed: Schelleng's minimum
/// force is inversely proportional to the distance from the bridge, so the
/// force scales with bow speed over bow position, and high notes need more.
fn force_ratio(pressure: f32, position: f32, hz: f32, vel: f32) -> f32 {
    // Centre of the Helmholtz window, measured on the model: lower strings
    // want proportionally more force per unit bow speed.
    let centre = 4.8 * (hz / 440.0).powf(-0.5).clamp(0.45, 40.0);
    let p = centre * 12f32.powf(pressure - 0.5);
    let dyn_boost = lerp(0.85, 1.2, vel);
    p * dyn_boost * (0.13 / position.clamp(0.05, 0.3))
}

/// One waveguide string (one player of the section on a note).
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
    nlp2: f32,
    slip: bool,
    rng: Noise,
    detune: f32,
    gl: f32,
    gr: f32,
    vib_ph: f32,
    vib_rate: f32,
    vib_depth: f32,
    vib_late: f32,
    drift: f32,
    walk_r: f32,
    walk_d: f32,
    walk_p: f32,
    scoop: f32,
    force_mul: f32,
    bow_mul: f32,
    pole_off: f32,
    trem_ph: f32,
    trem_mul: f32,
    pluck_n: u32,
    pluck_len: u32,
    pluck_amp: f32,
    thump_n: u32,
    tail_mul: f32,
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
            nlp2: 0.0,
            slip: false,
            rng: Noise::new(seed),
            detune: 0.0,
            gl: 1.0,
            gr: 1.0,
            vib_ph: 0.0,
            vib_rate: 1.0,
            vib_depth: 1.0,
            vib_late: 0.0,
            drift: 0.0,
            walk_r: 0.0,
            walk_d: 0.0,
            walk_p: 0.0,
            scoop: 1.0,
            force_mul: 1.0,
            bow_mul: 1.0,
            pole_off: 0.0,
            trem_ph: 0.0,
            trem_mul: 1.0,
            pluck_n: u32::MAX,
            pluck_len: 1,
            pluck_amp: 0.0,
            thump_n: u32::MAX,
            tail_mul: 1.0,
        }
    }

    fn clear(&mut self) {
        self.nut.clear();
        self.bridge.clear();
        self.lp = 0.0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        self.slip = false;
    }

    /// One sample. `rel` is the release coefficient once the bow has lifted.
    #[inline]
    fn tick(&mut self, c: &Ctl, arco: bool, d: &Drive, rel: Option<f32>) -> f32 {
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
        let mut hiss = 0.0;
        if arco {
            match rel {
                Some(k) => {
                    self.e *= k;
                    self.og *= k;
                }
                None => self.e = (self.e + self.e_inc).min(1.0),
            }
            self.onset *= 0.9995;
            // Bow-hair noise: broadband, strongest while the bow bites.
            let n = self.rng.white();
            self.nlp += 0.35 * (n - self.nlp);
            self.nlp2 += 0.05 * (n - self.nlp2);
            let rough = c.noise * (0.035 + 1.4 * self.onset);
            // Rosin hiss: wideband noise from the hair, outside the string loop.
            hiss = (n - self.nlp) * 22.0 * d.bow * self.e * c.noise * (0.1 + 1.0 * self.onset);
            let fmod = 1.0 + rough * (self.nlp - 0.5 * self.nlp2);
            let mut shape = 1.0;
            if c.trem_depth > 0.001 {
                self.trem_ph += c.trem_inc * self.trem_mul;
                self.trem_ph -= self.trem_ph.floor();
                let s = fast_sin(self.trem_ph);
                // Short strokes: quick reversal, flat in between.
                shape = lerp(1.0, s.signum() * s.abs().sqrt(), c.trem_depth);
            }
            let vb = d.bow * self.bow_mul * self.e * shape * (1.0 + 0.3 * rough * self.nlp);
            // Force starts light and builds with the stroke.
            let f2 = d.force * self.force_mul * (0.05 + 0.95 * self.e) * fmod;
            let string_vel = bridge_refl + nut_refl;
            inject = friction(vb - string_vel, f2, d.v0, &mut self.slip);
        } else if self.pluck_n < self.pluck_len {
            let ph = self.pluck_n as f32 / self.pluck_len as f32;
            inject = self.pluck_amp * (0.5 - 0.5 * fast_sin(ph + 0.25));
            self.pluck_n += 1;
        }
        if !arco && self.thump_n < 120 {
            // Fingertip and nail noise at the start of a pluck.
            let n = self.rng.white();
            self.nlp += 0.3 * (n - self.nlp);
            let env = 1.0 - self.thump_n as f32 / 120.0;
            inject += 0.35 * self.pluck_amp * self.nlp * env * env;
            self.thump_n += 1;
        }
        self.nut.push(bridge_refl + inject);
        self.bridge.push(nut_refl + inject);
        br_out + hiss
    }
}

struct VioVoice {
    active: bool,
    releasing: bool,
    id: u32,
    /// Runtime voice ID whose per-note expression this voice plays: the
    /// note on top of a legato phrase, otherwise `id`.
    xid: u32,
    /// Brightness from timbre. A shelf rather than the bridge loss, which
    /// would change how the bow grips the string.
    tilt: Tilt,
    pizz: bool,
    n: usize,
    strings: [Str; MAX_STRINGS],
    st: f32,
    tgt_st: f32,
    age_s: f32,
    age_samples: u32,
    drive: Drive,
    vel: f32,
    rel_coef: f32,
    follow: f32,
    fade: Fade,
    key: u8,
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
}

impl VioVoice {
    fn new(max_delay: usize, seed: u32) -> VioVoice {
        VioVoice {
            active: false,
            releasing: false,
            id: 0,
            xid: 0,
            tilt: Tilt::new(),
            pizz: false,
            n: 1,
            strings: std::array::from_fn(|i| {
                Str::new(max_delay, seed.wrapping_mul(16).wrapping_add(i as u32))
            }),
            st: 60.0,
            tgt_st: 60.0,
            age_s: 0.0,
            age_samples: 0,
            drive: Drive {
                bow: 0.1,
                force: 1.0,
                v0: 0.02,
            },
            vel: 0.7,
            rel_coef: 0.999,
            follow: 0.0,
            fade: Fade::new(),
            key: 60,
            xgain: 1.0,
        }
    }

    /// Control-rate update: pitch, vibrato, bow force and delay lengths.
    fn update(&mut self, c: &Ctl) {
        self.st += (self.tgt_st - self.st) * c.glide_k;
        if (self.tgt_st - self.st).abs() < 1.0e-3 {
            self.st = self.tgt_st;
        }
        self.age_s += c.dt;
        let base_hz = key_to_hz(self.st);
        self.drive.force = self.drive.bow * force_ratio(c.pressure, c.position, base_hz, self.vel);
        self.drive.v0 = 0.12 * self.drive.bow;
        let pizz = self.pizz;
        let age = self.age_s;
        for s in &mut self.strings[..self.n] {
            let t = ((age - c.vib_delay - s.vib_late) / c.vib_fade).clamp(0.0, 1.0);
            let vib_env = t * t * (3.0 - 2.0 * t);
            // Slow random walks: vibrato rate, depth and intonation wander.
            s.walk_r =
                (s.walk_r + (s.rng.white() * c.walk_g - s.walk_r) * c.walk_a).clamp(-2.0, 2.0);
            s.walk_d =
                (s.walk_d + (s.rng.white() * c.walk_g - s.walk_d) * c.walk_a).clamp(-2.0, 2.0);
            s.walk_p =
                (s.walk_p + (s.rng.white() * c.walk_g - s.walk_p) * c.walk_a).clamp(-2.0, 2.0);
            let rate = s.vib_rate
                * c.vib_rate
                * (0.88 + 0.12 * vib_env)
                * (1.0 + 0.1 * c.variation * s.walk_r);
            s.vib_ph += rate * c.dt;
            s.vib_ph -= s.vib_ph.floor();
            s.drift += (s.rng.white() - s.drift) * 0.03;
            // Not a pure sine: a violinist's vibrato is a little lopsided.
            let wave = fast_sin(s.vib_ph) + 0.1 * fast_sin(2.0 * s.vib_ph + 0.2);
            let depth = c.vib_depth * s.vib_depth * (1.0 + 0.2 * c.variation * s.walk_d);
            let scoop = if pizz {
                0.0
            } else {
                -14.0 * c.variation * s.scoop * (-age / 0.045).exp()
            };
            let cents = s.detune
                + wave * depth * vib_env
                + s.drift * (0.6 + 1.4 * c.variation)
                + s.walk_p * 2.5 * c.variation
                + scoop
                + c.bend_cents;
            let hz = (base_hz * (cents * (std::f32::consts::LN_2 / 1200.0)).exp())
                .clamp(MIN_HZ, c.sr / 6.0);
            let w = std::f32::consts::TAU * hz / c.sr;
            let (sw, cw) = w.sin_cos();
            // Plucked strings lose their highs quickly.
            s.pole = if pizz {
                c.pole + (1.0 - c.pole) * 0.1
            } else {
                (c.pole + s.pole_off).clamp(0.05, 0.95)
            };
            // Phase delay of the bridge loss filter at the fundamental.
            let pd = (s.pole * sw).atan2(1.0 - s.pole * cw) / w;
            let total = (c.sr / hz - pd - LOOP_TRIM).max(4.0);
            let d_br = (c.position * total).max(2.0).min(total - 2.0);
            s.d_bridge = d_br;
            s.d_nut = total - d_br;
            let t60 = if pizz {
                c.pluck_t60 * s.tail_mul * (440.0 / hz).powf(0.3).clamp(0.6, 2.2)
            } else {
                ARCO_T60
            };
            s.loop_gain = (-6.9078 / (t60 * hz)).exp();
        }
    }

    #[inline]
    fn tick(&mut self, c: &Ctl) -> (f32, f32) {
        let rel = if self.releasing && !self.pizz {
            Some(self.rel_coef)
        } else {
            None
        };
        let (mut l, mut r) = (0.0, 0.0);
        let mut max_og = 0.0f32;
        let drive = self.drive;
        for s in &mut self.strings[..self.n] {
            if s.wait > 0 {
                s.wait -= 1;
                max_og = max_og.max(1.0);
                continue;
            }
            let o = s.tick(c, !self.pizz, &drive, rel);
            max_og = max_og.max(s.og);
            l += o * s.gl * s.og;
            r += o * s.gr * s.og;
        }
        let (l, r) = self.tilt.tick(l, r);
        let g = self.fade.tick() * self.xgain;
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

/// A small room: early reflections plus a short feedback delay network.
struct Room {
    pre_l: DelayLine,
    pre_r: DelayLine,
    fdn: Fdn8,
    sr: f32,
}

/// Early reflection taps in ms for the left and right channels.
const ER_L: [(f32, f32); 4] = [(9.0, 0.5), (21.0, -0.4), (34.0, 0.3), (51.0, -0.22)];
const ER_R: [(f32, f32); 4] = [(13.0, 0.5), (27.0, -0.4), (42.0, 0.3), (58.0, -0.22)];
const FDN_SIGN: [f32; 8] = [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0];

impl Room {
    fn new(sr: f32) -> Room {
        let mut fdn = Fdn8::new(sr, (sr * 0.06) as usize);
        let base = [
            0.0213, 0.0239, 0.0271, 0.0297, 0.0331, 0.0359, 0.0389, 0.0421,
        ];
        let mut lens = [0.0f32; 8];
        for (l, b) in lens.iter_mut().zip(base) {
            *l = b * sr;
        }
        fdn.set_lengths(lens, true);
        fdn.set_t60(1.3);
        fdn.set_damping_hz(4_500.0);
        fdn.set_mod_depth(3.0);
        Room {
            pre_l: DelayLine::new((sr * 0.08) as usize),
            pre_r: DelayLine::new((sr * 0.08) as usize),
            fdn,
            sr,
        }
    }

    fn clear(&mut self) {
        self.pre_l.clear();
        self.pre_r.clear();
        self.fdn.clear();
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.pre_l.push(l);
        self.pre_r.push(r);
        let ms = self.sr * 0.001;
        let mut el = 0.0;
        let mut er = 0.0;
        for ((tl, gl), (tr, gr)) in ER_L.iter().zip(ER_R.iter()) {
            el += gl * self.pre_l.read((tl * ms) as usize);
            er += gr * self.pre_r.read((tr * ms) as usize);
        }
        let x =
            0.5 * (self.pre_l.read((16.0 * ms) as usize) + self.pre_r.read((19.0 * ms) as usize));
        let mut input = [0.0f32; 8];
        for (i, v) in input.iter_mut().enumerate() {
            *v = x * FDN_SIGN[i] * 0.35;
        }
        let o = self.fdn.process(&input);
        let wl = o[0] + o[2] + o[4] + o[6];
        let wr = o[1] + o[3] + o[5] + o[7];
        (el + 0.4 * wl, er + 0.4 * wr)
    }
}

pub(crate) struct Violin {
    expr: Expression,
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
    register: f32,
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
    room_amt: Ramp,
    level: Ramp,
    // Derived.
    ctl: Ctl,
    out_gain: f32,
    // Legato bookkeeping.
    stack: [Held; 16],
    stack_len: usize,
    leg_slot: usize,
    leg_id: u32,
    // Body.
    body_l: [Biquad; BODY_FILTERS],
    body_r: [Biquad; BODY_FILTERS],
    body_scale: f32,
    body_designed: f32,
    room: Room,
    tail: u32,
}

fn max_delay_for(sr: f32) -> usize {
    (sr / MIN_HZ) as usize + 8
}

impl Violin {
    pub(crate) fn new() -> Violin {
        let mut v = Violin {
            expr: Expression::new(),
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
            register: 0.8,
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
            room_amt: Ramp::new(0.0),
            level: Ramp::new(0.0),
            ctl: Ctl {
                sr: 48_000.0,
                dt: CTRL as f32 / 48_000.0,
                pressure: 0.5,
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
                trem_depth: 0.0,
                trem_inc: 0.0,
                walk_a: 0.0004,
                walk_g: 60.0,
                bend_cents: 0.0,
            },
            out_gain: ARCO_TRIM,
            stack: [Held { key: 0, id: 0 }; 16],
            stack_len: 0,
            leg_slot: 0,
            leg_id: 0,
            body_l: [Biquad::default(); BODY_FILTERS],
            body_r: [Biquad::default(); BODY_FILTERS],
            body_scale: 1.0,
            body_designed: 0.0,
            room: Room::new(48_000.0),
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
        let size = 2f32.powf((0.5 - self.size) * 0.5) * self.body_scale;
        self.body_designed = self.body_scale;
        let sr = self.sr;
        let bright = self.bright.get().clamp(0.0, 1.0);
        let res = self.resonance * 1.1;
        for (bank, split) in [
            (&mut self.body_l, 1.0 - BODY_SPLIT),
            (&mut self.body_r, 1.0 + BODY_SPLIT),
        ] {
            let scale = size * split;
            bank[0].coeffs =
                Coeffs::design(FilterKind::HighPass, sr, 110.0 * scale.min(1.2), 0.7, 0.0);
            for (i, (f, q, db)) in BODY_MODES.iter().enumerate() {
                // The bridge hill and above follow the brightness control.
                let hill = if *f >= 2_000.0 { 0.5 + bright } else { 1.0 };
                let g = db * res * hill;
                bank[i + 1].coeffs =
                    Coeffs::design(FilterKind::Peak, sr, (f * scale).min(sr * 0.45), *q, g);
            }
            let lp = (2_400.0 + 4_800.0 * bright) * scale.clamp(0.8, 1.2);
            bank[BODY_FILTERS - 1].coeffs =
                Coeffs::design(FilterKind::LowPass, sr, lp.min(sr * 0.45), 0.6, 0.0);
        }
    }

    fn derive(&mut self) {
        let sr = self.sr;
        let arco = self.style == 0;
        let walk_a = 1.0 - (-(CTRL as f32) / (0.8 * sr)).exp();
        self.ctl = Ctl {
            sr,
            dt: CTRL as f32 / sr,
            pressure: self.pressure.get(),
            position: self.position.get(),
            pole: lerp(0.35, 0.05, self.bright.get()).powf(48_000.0 / sr),
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
            trem_depth: self.trem_depth.get(),
            trem_inc: 0.5 * self.trem_rate / sr,
            walk_a,
            walk_g: 1.225 / walk_a.sqrt(),
            bend_cents: self.expr.bend_st() * 100.0,
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
        self.room_amt.tick(k);
        self.level.tick(k);
        self.expr.tick();
        self.derive();
        let ctl = self.ctl;
        let ex = &self.expr;
        // The body follows the register being played.
        let (mut sum, mut cnt) = (0.0f32, 0.0f32);
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            let x = ex.voice(v.xid, v.key);
            v.xgain = x.gain();
            v.tilt.set(&x);
            v.update(&ctl.with(&x));
            if !v.releasing {
                sum += v.tgt_st;
                cnt += 1.0;
            }
        }
        if cnt > 0.0 {
            let mean = sum / cnt;
            let low = 2f32.powf(-(55.0 - mean).max(0.0) / 12.0 * 0.8).max(0.3);
            let target = lerp(1.0, low, self.register);
            self.body_scale += (target - self.body_scale) * 0.01;
            if (self.body_scale / self.body_designed - 1.0).abs() > 0.004 {
                self.design_body();
            }
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
            (2.0 + ens * 6.99) as usize
        }
        .min(MAX_STRINGS);
        let pizz = self.style == 1;
        // A plucked note gets a partner string per player: the other
        // polarisation, which rings on long after the body has drunk the first.
        let n = if pizz { n.min(MAX_STRINGS / 2) } else { n };
        let slots = if pizz { 2 * n } else { n };
        let weight = 1.0 / (n as f32).powf(0.62);
        let amount = ens.powf(0.7);
        let hz0 = key_to_hz(key as f32).clamp(MIN_HZ, sr / 6.0);
        let mut rnd = [[0.0f32; 8]; MAX_STRINGS];
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
        let x = self.expr.voice(id, key);
        let xgain = x.gain();
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.releasing = false;
        v.id = id;
        v.xid = id;
        v.tilt.prepare(sr);
        v.tilt.reset();
        v.tilt.set(&x);
        v.key = key;
        v.xgain = xgain;
        v.pizz = pizz;
        v.n = slots;
        v.st = key as f32;
        v.tgt_st = key as f32;
        v.key = key;
        v.age_s = 0.0;
        v.age_samples = 0;
        v.drive.bow = bow_vel;
        v.vel = velocity;
        v.rel_coef = rel_coef;
        v.follow = 0.0;
        v.fade.reset();
        for (slot_i, s) in v.strings[..slots].iter_mut().enumerate() {
            let tail = pizz && slot_i % 2 == 1;
            let i = if pizz { slot_i / 2 } else { slot_i };
            let r = rnd[i];
            s.clear();
            s.og = 1.0;
            s.e = 0.0;
            s.onset = 1.0;
            s.nlp = 0.0;
            s.nlp2 = 0.0;
            s.drift = 0.0;
            s.walk_r = (r[0] - 0.5) * 2.0;
            s.walk_d = (r[1] - 0.5) * 2.0;
            s.walk_p = (r[2] - 0.5) * 2.0;
            // Entry times: the first player leads, the rest follow by a few
            // ms up to about 30 ms.
            let entry_ms = if i == 0 { 0.0 } else { 3.0 + 27.0 * r[5] };
            s.wait = (entry_ms * amount * 0.001 * sr) as u32;
            let att = attack * (1.0 + 0.5 * amount * (r[6] - 0.3));
            s.e_inc = 1.0 / (att * sr).max(1.0);
            // Tuning: a few to ten cents either way.
            s.detune = if i == 0 {
                (r[1] - 0.5) * 4.0 * amount
            } else {
                DETUNE_SIGN[i] * (3.0 + 7.0 * r[3]) * amount
            };
            let pan = (PAN[i] + (r[4] - 0.5) * 0.2) * width;
            let (pl, pr) = pan_gains(pan);
            let lvl = if tail { 0.16 } else { 1.0 };
            s.gl = pl * weight * lvl;
            s.gr = pr * weight * lvl;
            s.tail_mul = if tail { 4.5 } else { 1.0 };
            if tail {
                s.detune += 0.7;
            }
            s.vib_ph = r[2];
            s.vib_rate = 1.0 + (r[3] - 0.5) * 0.3 * variation + (r[7] - 0.5) * 0.2 * amount;
            s.vib_depth = 1.0 + (r[0] - 0.5) * 0.6 * variation;
            s.vib_late = r[6] * 0.35 * amount;
            s.scoop = 0.4 + 1.2 * r[4];
            s.force_mul = 1.0 + (r[7] - 0.5) * 0.3 * amount;
            s.bow_mul = 1.0 + (r[5] - 0.5) * 0.14 * amount;
            s.pole_off = (r[1] - 0.5) * 0.12 * amount;
            s.trem_ph = r[3];
            s.trem_mul = 1.0 + (r[2] - 0.5) * 0.25 * amount.max(0.3);
            s.pole = pole;
            let period = sr / hz0;
            s.pluck_len = (period * (0.34 - 0.22 * velocity)).max(3.0) as u32;
            s.pluck_amp = pluck_amp;
            s.pluck_n = if pizz { 0 } else { u32::MAX };
            s.thump_n = if pizz { 0 } else { u32::MAX };
        }
        // Make the delay lengths valid before the first sample.
        v.update(&self.ctl.with(&x));
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

    fn tail_len(&self) -> u32 {
        ((0.25 + 1.6 * self.room_amt.get()) * self.sr) as u32
    }

    fn reset_audio(&mut self) {
        for b in self.body_l.iter_mut().chain(self.body_r.iter_mut()) {
            b.reset();
        }
        self.room.clear();
    }
}

impl Synth for Violin {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Self::make_pool(sample_rate);
        self.room = Room::new(sample_rate);
        self.ctrl = 0;
        self.stack_len = 0;
        self.tail = u32::MAX;
        self.body_scale = 1.0;
        self.design_body();
        self.derive();
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
        self.stack_len = 0;
        self.tail = u32::MAX;
        self.reset_audio();
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
            BODY_REGISTER => self.register = v,
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
            ROOM => self.room_amt.set(v, imm),
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
                v.key = key;
                v.xid = id;
                v.drive.bow = bow;
                v.vel = velocity;
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
                    v.key = key;
                    v.xid = self.stack[self.stack_len - 1].id;
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
        self.reset_audio();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        for i in 0..n {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let tail_len = self.tail_len();
            if self.pool.is_silent() {
                if self.tail < tail_len {
                    self.tail += 1;
                    if self.tail == tail_len {
                        self.reset_audio();
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
            let ctl = self.ctl;
            let (mut l, mut r) = (0.0, 0.0);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick(&ctl);
                l += a;
                r += b;
            }
            for b in &mut self.body_l {
                l = b.process(l);
            }
            for b in &mut self.body_r {
                r = b.process(r);
            }
            let room = self.room_amt.get();
            if room > 0.001 {
                let (wl, wr) = self.room.process(l, r);
                l += wl * room;
                r += wr * room;
            }
            left[i] = l * self.out_gain;
            right[i] = r * self.out_gain;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent() && self.tail >= self.tail_len()
    }
}

/// Bow speed for a note. High notes need a slower bow or the string jumps to
/// a harmonic, so the speed falls with pitch; low strings take a faster bow.
fn bow_speed(key: u8, velocity: f32, sens: f32) -> f32 {
    let hz = key_to_hz(key as f32);
    let pitch = (hz / 330.0).powf(-0.2).clamp(0.3, 1.0);
    (0.03 + 0.17 * vel_gain(velocity, sens)) * pitch
}
