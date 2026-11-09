//! `peel.keys.tine`: a physically informed tine electric piano (Rhodes-style).
//!
//! The model follows the instrument's signal path instead of imitating its
//! spectrum with FM:
//!
//! 1. **Tine and tonebar.** The vibrating tine is a small bank of damped
//!    modes: a strong fundamental (the tine and tonebar tuning fork, plus a
//!    slightly detuned partner mode that makes it shimmer), and two
//!    cantilever overtones at about 6.3 to 7.5 and 17.5 times the fundamental
//!    that are not harmonic, ring for a fraction of a second and are the
//!    "ping". Each mode is a two-state rotation, so displacement `u` and
//!    velocity `w` are both available without differentiating.
//! 2. **Hammer.** A half-sine felt pulse, shorter for hard or high notes.
//!    Its spectrum weights the overtones (a hard strike excites more ping)
//!    and a short lowpassed noise burst gives the key and hammer thump.
//! 3. **Pickup.** The magnet on the tine moves in front of a coil. Flux is a
//!    peaked function of displacement, here the Lorentzian
//!    `phi(u) = 1 / (1 + (u - s)^2)` with an offset `s` (the voicing), and the
//!    output is the voltage `d phi / dt = phi'(u) u'`. At low amplitude this
//!    is a clean sine; as the tine swings further the flux curve bends the
//!    wave and a harmonic series appears, so a hard note barks and the bark
//!    relaxes into the clean tone as the note decays. The curve and its
//!    amplitudes were fitted to harmonic levels measured from a recorded
//!    Rhodes Mark I.
//! 4. **Damper.** On release the felt mutes the tine; the modes decay quickly
//!    (the overtones faster) with a soft thud.
//! 5. **Output stage.** Suitcase style tremolo or stereo auto-pan (free or
//!    tempo synced), a preamp that saturates gently with a little asymmetry,
//!    a tone control, a coupling high pass and an optional stereo chorus.
//!
//! The fundamental's decay time follows the keyboard (long in the bass, short
//! in the treble), and every key gets a small deterministic tuning and
//! overtone variation so notes are not identical.

use std::f32::consts::TAU;

use peel_dsp::delay::DelayLine;
use peel_dsp::filter::tanh_approx;
use peel_dsp::osc::{Lfo, LfoShape, fast_sin};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz, pan_gains};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, Tilt, lerp,
    ramp_coef, vel_gain,
};

const BELL: ParamId = 3;
const BARK: ParamId = 4;
const BELL_DECAY: ParamId = 5;
const VELOCITY: ParamId = 6;
const PICKUP: ParamId = 7;
const HAMMER: ParamId = 8;
const BEAT: ParamId = 9;
const ATTACK: ParamId = 10;
const DECAY: ParamId = 11;
const RELEASE: ParamId = 12;
const THUMP: ParamId = 13;
const TREM_RATE: ParamId = 20;
const TREM_DEPTH: ParamId = 21;
const TREM_MODE: ParamId = 22;
const TREM_SYNC: ParamId = 23;
const WIDTH: ParamId = 30;
const LEVEL: ParamId = 31;
const DRIVE: ParamId = 32;
const TONE: ParamId = 33;
const CHORUS: ParamId = 34;

/// Tremolo cycle lengths in beats for the sync choices after "Free".
const SYNC_BEATS: [f64; 6] = [2.0, 1.0, 2.0 / 3.0, 0.5, 1.0 / 3.0, 0.25];

static PARAMS: [ParamDescriptor; 20] = [
    ParamDescriptor::linear(
        BELL,
        "tone.bell",
        "Bell",
        "Tone",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BARK,
        "tone.growl",
        "Bark",
        "Tone",
        0.0,
        1.0,
        0.65,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        BELL_DECAY,
        "tone.bell_decay",
        "Bell Decay",
        "Tone",
        0.05,
        1.5,
        0.45,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        VELOCITY,
        "tone.velocity",
        "Velocity",
        "Tone",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        PICKUP,
        "tone.pickup",
        "Pickup",
        "Tone",
        0.0,
        1.0,
        0.2,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        HAMMER,
        "tone.hammer",
        "Hammer",
        "Tone",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BEAT,
        "tone.beat",
        "Beat",
        "Tone",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        THUMP,
        "tone.thump",
        "Thump",
        "Tone",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ATTACK,
        "amp.attack",
        "Attack",
        "Envelope",
        0.001,
        0.1,
        0.002,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        DECAY,
        "amp.decay",
        "Decay",
        "Envelope",
        0.3,
        20.0,
        8.0,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        RELEASE,
        "amp.release",
        "Release",
        "Envelope",
        0.03,
        2.0,
        0.25,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        TREM_RATE,
        "trem.rate",
        "Rate",
        "Tremolo",
        0.5,
        12.0,
        5.2,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        TREM_DEPTH,
        "trem.depth",
        "Depth",
        "Tremolo",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::choice(
        TREM_MODE,
        "trem.mode",
        "Mode",
        "Tremolo",
        &["Tremolo", "Auto-Pan"],
        1,
    ),
    ParamDescriptor::choice(
        TREM_SYNC,
        "trem.sync",
        "Sync",
        "Tremolo",
        &["Free", "1/2", "1/4", "1/4T", "1/8", "1/8T", "1/16"],
        0,
    ),
    ParamDescriptor::linear(
        WIDTH,
        "out.width",
        "Width",
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
    ParamDescriptor::linear(
        DRIVE,
        "out.drive",
        "Drive",
        "Output",
        0.0,
        1.0,
        0.25,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        TONE,
        "out.tone",
        "Tone",
        "Output",
        0.0,
        1.0,
        0.6,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        CHORUS,
        "out.chorus",
        "Chorus",
        "Output",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.keys.tine",
    name: "Tine",
    description: "Electric piano with a tine, hammer and pickup model, tremolo and auto-pan",
    category: "Keys",
    kind: DeviceKind::Instrument,
    version: 2,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 8] = [
    Preset {
        name: "Classic",
        params: &[
            ("tone.bell", 0.5),
            ("tone.growl", 0.65),
            ("tone.bell_decay", 0.45),
            ("tone.velocity", 0.7),
            ("tone.pickup", 0.2),
            ("tone.hammer", 0.5),
            ("tone.beat", 0.35),
            ("tone.thump", 0.35),
            ("amp.attack", 0.002),
            ("amp.decay", 8.0),
            ("amp.release", 0.25),
            ("trem.rate", 5.2),
            ("trem.depth", 0.3),
            ("trem.mode", 1.0),
            ("trem.sync", 0.0),
            ("out.width", 0.5),
            ("out.level", 0.0),
            ("out.drive", 0.25),
            ("out.tone", 0.6),
            ("out.chorus", 0.0),
        ],
    },
    Preset {
        name: "Mellow",
        params: &[
            ("tone.bell", 0.25),
            ("tone.growl", 0.25),
            ("tone.bell_decay", 0.3),
            ("tone.pickup", 0.35),
            ("tone.hammer", 0.2),
            ("tone.thump", 0.25),
            ("amp.decay", 10.0),
            ("amp.release", 0.35),
            ("trem.depth", 0.0),
            ("out.drive", 0.15),
            ("out.tone", 0.4),
        ],
    },
    Preset {
        name: "Dyno",
        params: &[
            ("tone.bell", 0.85),
            ("tone.growl", 0.4),
            ("tone.bell_decay", 0.7),
            ("tone.pickup", 0.1),
            ("tone.hammer", 0.85),
            ("tone.beat", 0.2),
            ("amp.decay", 6.0),
            ("amp.release", 0.2),
            ("trem.depth", 0.15),
            ("out.drive", 0.2),
            ("out.tone", 0.85),
        ],
    },
    Preset {
        name: "Suitcase",
        params: &[
            ("tone.bell", 0.5),
            ("tone.growl", 0.6),
            ("tone.beat", 0.5),
            ("trem.rate", 4.6),
            ("trem.depth", 0.65),
            ("trem.mode", 1.0),
            ("out.width", 0.8),
            ("out.drive", 0.35),
        ],
    },
    Preset {
        name: "Bark",
        params: &[
            ("tone.bell", 0.45),
            ("tone.growl", 1.0),
            ("tone.bell_decay", 0.3),
            ("tone.velocity", 0.9),
            ("tone.pickup", 0.1),
            ("tone.hammer", 0.7),
            ("amp.decay", 5.0),
            ("amp.release", 0.15),
            ("trem.rate", 5.8),
            ("trem.depth", 0.2),
            ("out.drive", 0.45),
            ("out.tone", 0.55),
        ],
    },
    Preset {
        name: "Dream",
        params: &[
            ("tone.bell", 0.4),
            ("tone.growl", 0.3),
            ("tone.bell_decay", 0.6),
            ("tone.pickup", 0.3),
            ("tone.hammer", 0.3),
            ("tone.beat", 0.6),
            ("tone.thump", 0.15),
            ("amp.attack", 0.012),
            ("amp.decay", 14.0),
            ("amp.release", 0.9),
            ("trem.rate", 3.2),
            ("trem.depth", 0.4),
            ("out.width", 1.0),
            ("out.drive", 0.15),
            ("out.tone", 0.5),
            ("out.chorus", 0.6),
        ],
    },
    Preset {
        name: "Pluck",
        params: &[
            ("tone.bell", 0.7),
            ("tone.growl", 0.65),
            ("tone.bell_decay", 0.2),
            ("tone.hammer", 0.8),
            ("amp.decay", 1.6),
            ("amp.release", 0.1),
            ("trem.depth", 0.0),
            ("out.drive", 0.3),
        ],
    },
    Preset {
        name: "Synced",
        params: &[
            ("tone.growl", 0.55),
            ("trem.depth", 0.55),
            ("trem.mode", 1.0),
            ("trem.sync", 4.0),
            ("out.width", 0.7),
            ("out.chorus", 0.2),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Tine::new()))
}

/// Number of modes per tine: fundamental, tonebar partner, two overtones.
const MODES: usize = 4;

/// Floor in `ln(1000)`-based T60 conversions: the time for -60 dB.
const LN_1000: f64 = 6.907_755_278_982_137;

/// Deterministic per-key pseudo random value in `0..1`.
fn key_rand(key: u8, salt: u32) -> f32 {
    let mut h = (key as u32 + 1).wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Spectrum of a half-sine contact pulse of length `t` at `f`, normalized to
/// one at zero frequency.
fn hammer_spectrum(f: f32, t: f32) -> f32 {
    let x = 2.0 * f * t;
    let den = 1.0 - x * x;
    if den.abs() < 1.0e-3 {
        std::f32::consts::FRAC_PI_4
    } else {
        ((std::f32::consts::FRAC_PI_2 * x).cos() / den).abs()
    }
}

/// Pickup flux slope `-phi'(u)` for `phi(u) = 1 / (1 + (u - s)^2)`.
#[inline]
fn pickup_slope(u: f64, s: f64) -> f64 {
    let z = u - s;
    let d = 1.0 + z * z;
    -2.0 * z / (d * d)
}

#[derive(Clone, Copy)]
struct TineVoice {
    active: bool,
    // Mode oscillators in double precision so low notes keep exact pitch.
    sn: [f64; MODES],
    cs: [f64; MODES],
    cw: [f64; MODES],
    sw: [f64; MODES],
    r: [f64; MODES],
    au: [f64; MODES],
    aw: [f64; MODES],
    offset: f64,
    gain: f32,
    env: f32,
    env0: f32,
    // Attack ramp.
    att_pos: f32,
    att_inv: f32,
    // Mechanical noise.
    rng: u32,
    th_env: f32,
    th_sm: f32,
    th_rise: f32,
    th_coef: f32,
    th_lp: f32,
    th_lp2: f32,
    th_k: f32,
    th_gain: f32,
    thud: f32,
    pan_l: f32,
    pan_r: f32,
    fade: Fade,
    key: u8,
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
    /// Runtime voice ID, for per-note expression.
    id: u32,
    /// Brightness from timbre.
    tilt: Tilt,
    /// Played mode angles and the pitch ratio currently applied to them.
    w0: [f64; MODES],
    ratio: f32,
}

impl TineVoice {
    fn new() -> TineVoice {
        TineVoice {
            active: false,
            sn: [0.0; MODES],
            cs: [0.0; MODES],
            cw: [1.0; MODES],
            sw: [0.0; MODES],
            r: [1.0; MODES],
            au: [0.0; MODES],
            aw: [0.0; MODES],
            offset: 0.3,
            gain: 0.0,
            env: 0.0,
            env0: 1.0,
            att_pos: 0.0,
            att_inv: 1.0,
            rng: 1,
            th_env: 0.0,
            th_sm: 0.0,
            th_rise: 0.05,
            th_coef: 0.0,
            th_lp: 0.0,
            th_lp2: 0.0,
            th_k: 0.1,
            th_gain: 0.0,
            thud: 0.0,
            pan_l: 1.0,
            pan_r: 1.0,
            fade: Fade::new(),
            key: 60,
            xgain: 1.0,
            id: 0,
            tilt: Tilt::new(),
            w0: [0.0; MODES],
            ratio: 1.0,
        }
    }

    /// Rotates each mode at `ratio` times its played rate. Phase and
    /// amplitude state are untouched, so the ring carries on smoothly.
    fn retune(&mut self, ratio: f32) {
        if ratio == self.ratio {
            return;
        }
        self.ratio = ratio;
        let top = 0.9 * std::f64::consts::PI;
        for k in 0..MODES {
            let w0 = self.w0[k];
            let w = (w0 * ratio as f64).min(top.max(w0));
            self.cw[k] = w.cos();
            self.sw[k] = w.sin();
        }
    }

    #[inline]
    fn tick(&mut self) -> (f32, f32) {
        // Tine: displacement u and (frequency normalized) velocity w.
        let mut u = 0.0f64;
        let mut w = 0.0f64;
        for k in 0..MODES {
            let (s, c) = (self.sn[k], self.cs[k]);
            u += self.au[k] * s;
            w += self.aw[k] * c;
            self.sn[k] = self.r[k] * (s * self.cw[k] + c * self.sw[k]);
            self.cs[k] = self.r[k] * (c * self.cw[k] - s * self.sw[k]);
        }
        // Pickup: voltage is the time derivative of the flux.
        let mut y = (pickup_slope(u, self.offset) * w) as f32 * self.gain * self.xgain;

        // Smooth hammer onset.
        if self.att_pos < 1.0 {
            let t = self.att_pos;
            self.att_pos += self.att_inv;
            y *= t * t * (3.0 - 2.0 * t);
        }

        // Key thump / damper thud: lowpassed noise with a fast decay.
        if self.th_env > 1.0e-5 || self.th_sm > 1.0e-5 {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 17;
            self.rng ^= self.rng << 5;
            let white = (self.rng as i32) as f32 * (1.0 / 2_147_483_648.0);
            self.th_lp += self.th_k * (white - self.th_lp);
            self.th_sm += self.th_rise * (self.th_env - self.th_sm);
            self.th_lp2 += self.th_k * (self.th_lp - self.th_lp2);
            y += self.th_lp2 * self.th_sm * self.th_gain;
            self.th_env *= self.th_coef;
        }

        let g = self.fade.tick();
        // A finished tine (and its thud) retires the voice.
        self.env *= self.r[0] as f32;
        if (self.env < 1.0e-6 && self.th_env < 1.0e-4 && self.th_sm < 1.0e-4) || self.fade.is_done()
        {
            self.active = false;
        }
        let y = self.tilt.tick_mono(y);
        (y * g * self.pan_l, y * g * self.pan_r)
    }
}

impl PoolVoice for TineVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.env
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

/// Stereo chorus for the "Dream" width: two modulated delays, one per side,
/// swept in opposite directions.
struct Chorus {
    dl: DelayLine,
    dr: DelayLine,
    p1: f32,
    p2: f32,
}

impl Chorus {
    fn new(sample_rate: f32) -> Chorus {
        let max = (0.04 * sample_rate) as usize;
        Chorus {
            dl: DelayLine::new(max),
            dr: DelayLine::new(max),
            p1: 0.0,
            p2: 0.33,
        }
    }

    fn reset(&mut self) {
        self.dl.clear();
        self.dr.clear();
        self.p1 = 0.0;
        self.p2 = 0.33;
    }

    /// Keeps the delay lines current while the chorus is off.
    #[inline]
    fn push(&mut self, l: f32, r: f32) {
        self.dl.push(l);
        self.dr.push(r);
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32, amount: f32, sr: f32) -> (f32, f32) {
        self.p1 += 0.37 / sr;
        self.p2 += 0.53 / sr;
        self.p1 -= self.p1.floor();
        self.p2 -= self.p2.floor();
        self.dl.push(l);
        self.dr.push(r);
        let base = 0.013 * sr;
        let depth = (0.0035 + 0.0035 * amount) * sr;
        let dl = base + depth * fast_sin(self.p1);
        let dr = base - depth * fast_sin(self.p2);
        let wl = self.dl.read_linear(dl);
        let wr = self.dr.read_linear(dr);
        // Cross a little of the other side's wet signal for extra spread.
        let wet = 0.7 * amount;
        let dry = 1.0 - 0.3 * amount;
        (
            l * dry + wet * (0.8 * wl + 0.2 * wr),
            r * dry + wet * (0.8 * wr + 0.2 * wl),
        )
    }
}

pub(crate) struct Tine {
    expr: Expression,
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    pool: Pool<TineVoice>,
    note_count: u32,
    bell: f32,
    bark: f32,
    bell_decay: f32,
    vel_sens: f32,
    pickup: f32,
    hammer: f32,
    beat: f32,
    thump: f32,
    attack: f32,
    decay: f32,
    release: f32,
    mode: usize,
    sync: usize,
    width: f32,
    trem_rate: Ramp,
    trem_depth: Ramp,
    level: Ramp,
    drive: Ramp,
    tone: Ramp,
    chorus: Ramp,
    lfo: Lfo,
    lfo_dt: f32,
    tempo: f64,
    out_gain: f32,
    drive_gain: f32,
    drive_bias: f32,
    drive_norm: f32,
    tone_k: f32,
    shelf_g: f32,
    top_k: f32,
    top: [f32; 2],
    lp: [f32; 2],
    hp: [f32; 2],
    hp_k: f32,
    chorus_fx: Chorus,
}

impl Tine {
    pub(crate) fn new() -> Tine {
        Tine {
            expr: Expression::new(),
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            pool: Pool::new(20, 16, StealPolicy::Quietest, 48_000.0, TineVoice::new),
            note_count: 0,
            bell: 0.5,
            bark: 0.65,
            bell_decay: 0.45,
            vel_sens: 0.7,
            pickup: 0.2,
            hammer: 0.5,
            beat: 0.35,
            thump: 0.35,
            attack: 0.002,
            decay: 8.0,
            release: 0.25,
            mode: 1,
            sync: 0,
            width: 0.5,
            trem_rate: Ramp::new(5.2f32.ln()),
            trem_depth: Ramp::new(0.3),
            level: Ramp::new(0.0),
            drive: Ramp::new(0.25),
            tone: Ramp::new(0.6),
            chorus: Ramp::new(0.0),
            lfo: Lfo::new(0x7117),
            lfo_dt: 0.0,
            tempo: 120.0,
            out_gain: 1.0,
            drive_gain: 1.0,
            drive_bias: 0.0,
            drive_norm: 1.0,
            tone_k: 0.2,
            shelf_g: 0.0,
            top_k: 0.5,
            top: [0.0; 2],
            lp: [0.0; 2],
            hp: [0.0; 2],
            hp_k: 0.003,
            chorus_fx: Chorus::new(48_000.0),
        }
    }

    fn derive(&mut self) {
        self.lfo_dt = if self.sync == 0 {
            self.trem_rate.get().exp() / self.sr
        } else {
            (self.tempo / 60.0 / SYNC_BEATS[self.sync - 1]) as f32 / self.sr
        };
        self.out_gain = db_to_gain(self.level.get()) * 0.9;
        let d = self.drive.get();
        self.drive_gain = 1.0 + 2.5 * d * d + 0.5 * d;
        self.drive_bias = 0.22 * d;
        let slope = 1.0 - tanh_approx(self.drive_bias).powi(2);
        self.drive_norm = 1.0 / (self.drive_gain * slope);
        // Tone: a treble shelf around 2.2 kHz from -3 dB to +9 dB.
        let shelf_db = -3.0 + 12.0 * self.tone.get();
        self.shelf_g = db_to_gain(shelf_db) - 1.0;
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.trem_rate.tick(k);
        self.trem_depth.tick(k);
        self.level.tick(k);
        self.drive.tick(k);
        self.tone.tick(k);
        self.chorus.tick(k);
        self.derive();
        self.expr.tick();
        let ex = &self.expr;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            let x = ex.voice(v.id, v.key);
            v.xgain = x.gain();
            v.retune(x.ratio);
            v.tilt.set(&x);
        }
        self.pool.sync();
        // With nothing sounding, let the output stage settle to exact zero.
        if self.pool.is_silent() {
            for st in self.lp.iter_mut().chain(&mut self.top).chain(&mut self.hp) {
                if st.abs() < 1.0e-9 {
                    *st = 0.0;
                }
            }
        }
    }

    /// Clears the output stage state (reset and sound-off).
    fn clear_stage(&mut self) {
        self.chorus_fx.reset();
        self.lp = [0.0; 2];
        self.top = [0.0; 2];
        self.hp = [0.0; 2];
    }

    /// Time in seconds for a -60 dB fundamental decay at `key`.
    fn fundamental_t60(&self, key: f32) -> f32 {
        (self.decay * 2f32.powf(-(key - 60.0) / 22.0)).max(0.12)
    }
}

impl Synth for Tine {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(20, 16, StealPolicy::Quietest, sample_rate, TineVoice::new);
        self.chorus_fx = Chorus::new(sample_rate);
        self.hp_k = 1.0 - (-TAU * 28.0 / sample_rate).exp();
        self.tone_k = 1.0 - (-TAU * 2_200.0 / sample_rate).exp();
        self.top_k = 1.0 - (-TAU * (11_000.0f32).min(sample_rate * 0.45) / sample_rate).exp();
        self.ctrl = 0;
        self.lp = [0.0; 2];
        self.top = [0.0; 2];
        self.hp = [0.0; 2];
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
        self.lfo.reset(0.0);
        self.clear_stage();
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            BELL => self.bell = v,
            BARK => self.bark = v,
            BELL_DECAY => self.bell_decay = v,
            VELOCITY => self.vel_sens = v,
            PICKUP => self.pickup = v,
            HAMMER => self.hammer = v,
            BEAT => self.beat = v,
            THUMP => self.thump = v,
            ATTACK => self.attack = v,
            DECAY => self.decay = v,
            RELEASE => self.release = v,
            TREM_RATE => self.trem_rate.set(v.ln(), imm),
            TREM_DEPTH => self.trem_depth.set(v, imm),
            TREM_MODE => self.mode = v as usize,
            TREM_SYNC => self.sync = (v as usize).min(SYNC_BEATS.len()),
            WIDTH => self.width = v,
            LEVEL => self.level.set(v, imm),
            DRIVE => self.drive.set(v, imm),
            TONE => self.tone.set(v, imm),
            CHORUS => self.chorus.set(v, imm),
            _ => {}
        }
        if imm {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.pool.begin(key, id);
        self.note_count = self.note_count.wrapping_add(1);
        let sr = self.sr;
        let keyf = key as f32;
        let nyquist_guard = 0.42 * sr;

        // Per-key character: a few cents of tuning, overtone ratios, bark.
        let cents = (key_rand(key, 1) - 0.5) * 4.0;
        let hz = key_to_hz(keyf) * 2f32.powf(cents / 1200.0);
        let t = key_rand(key, 2);
        let ratio2 = if t < 0.5 {
            6.3 + 0.55 * (t * 2.0)
        } else {
            7.15 + 0.35 * ((t - 0.5) * 2.0)
        };
        let ratio3 = 17.25 + 0.5 * key_rand(key, 3);
        let beat_sign = if key_rand(key, 4) < 0.5 { -1.0 } else { 1.0 };
        let beat_ratio = 1.0 + beat_sign * 0.0016 * (0.4 + 0.6 * key_rand(key, 5));
        let bark_var = 0.9 + 0.2 * key_rand(key, 6);

        // Hammer: harder or higher strikes make a shorter contact.
        let v_eff = lerp(0.65, velocity, self.vel_sens);
        let contact = (0.0035 - 0.0026 * self.hammer)
            * (1.25 - 0.6 * v_eff)
            * 2f32.powf(-(keyf - 60.0) / 36.0);
        let h0 = hammer_spectrum(hz, contact).max(0.3);

        // Pickup drive: velocity squared, less on high notes.
        let high = ((keyf - 60.0).max(0.0)) / 12.0;
        let key_drive =
            (1.0 / (1.0 + high * high)) * (1.0 + ((60.0 - keyf) / 30.0).clamp(0.0, 0.6));
        let a_hi = 0.2 + 1.5 * self.bark;
        let amp = (a_hi * v_eff.powf(1.8) * key_drive * bark_var).max(0.02);
        let offset = 0.12 + 0.9 * self.pickup;

        // Overtone strength relative to the fundamental (output terms).
        let ping = self.bell * (0.25 + 0.75 * v_eff);
        // The floor keeps a spectral null of the felt pulse from silencing a
        // key's ping altogether.
        let strike = |f: f32| (hammer_spectrum(f, contact) / h0).hypot(0.035);
        let rho2 = if hz * ratio2 < nyquist_guard {
            1.6 * ping * strike(hz * ratio2)
        } else {
            0.0
        };
        let rho3 = if hz * ratio3 < nyquist_guard {
            0.8 * ping * strike(hz * ratio3)
        } else {
            0.0
        };
        let rho_b = 0.3 * self.beat;
        let rho = [1.0, rho_b, rho2, rho3];
        let ratios = [1.0, beat_ratio, ratio2, ratio3];

        // Decay times.
        let t60 = self.fundamental_t60(keyf);
        let ping_t60 = self.bell_decay * (0.5 + 0.5 * 2f32.powf(-(keyf - 60.0) / 24.0));
        let t60s = [t60, t60 * 0.8, ping_t60, ping_t60 * 0.25];

        let sr64 = sr as f64;
        let x = self.expr.voice(id, key);
        let (bend_ratio, xgain) = (x.ratio, x.gain());
        let v = &mut self.pool.voices[slot];
        v.active = true;
        for k in 0..MODES {
            let w = std::f64::consts::TAU * hz as f64 * ratios[k] as f64 / sr64;
            v.sn[k] = 0.0;
            v.cs[k] = 1.0;
            v.cw[k] = w.cos();
            v.sw[k] = w.sin();
            v.w0[k] = w;
            v.r[k] = (-LN_1000 / (t60s[k] as f64 * sr64)).exp();
            v.au[k] = amp as f64 * rho[k] as f64 / ratios[k] as f64;
            v.aw[k] = amp as f64 * rho[k] as f64;
        }
        v.offset = offset as f64;
        v.key = key;
        v.ratio = 1.0;
        v.xgain = xgain;
        v.id = id;
        v.tilt.prepare(sr);
        v.tilt.reset();

        // Normalize the fundamental's pickup output to a fixed RMS so the
        // loudness follows velocity while the bark only changes color.
        let mut sum = 0.0f64;
        const N: usize = 32;
        for i in 0..N {
            let th = std::f64::consts::TAU * (i as f64 + 0.5) / N as f64;
            let y = pickup_slope(amp as f64 * th.sin(), offset as f64) * amp as f64 * th.cos();
            sum += y * y;
        }
        let rms = (sum / N as f64).sqrt().max(1.0e-9);
        let loud = 0.34 * vel_gain(velocity, 0.35 + 0.55 * self.vel_sens);
        v.gain = (loud as f64 / rms) as f32;

        // Attack ramp: the attack time plus the contact time.
        let att = self.attack + 0.35 * contact;
        v.att_pos = 0.0;
        v.att_inv = 1.0 / (att * sr).max(1.0);

        // Key thump: lowpassed noise, louder for hard strikes.
        v.rng = 0x1234_5679u32
            .wrapping_add(self.note_count.wrapping_mul(0x9E37_79B1))
            .wrapping_add(key as u32 * 7919)
            | 1;
        v.th_k = 1.0 - (-TAU * 750.0 / sr).exp();
        v.th_coef = (-1.0 / (0.012 * sr)).exp();
        v.th_env = 1.0;
        v.th_sm = 1.0;
        v.th_rise = 1.0 - (-1.0 / (0.002 * sr)).exp();
        v.th_lp = 0.0;
        v.th_lp2 = 0.0;
        v.th_gain = self.thump * 0.22 * velocity * velocity * loud / v.th_k;
        v.thud = self.thump * 0.3 * loud;
        v.env = v.gain * amp;
        v.env0 = v.env.max(1.0e-12);

        let key_pan = ((keyf - 60.0) / 48.0).clamp(-1.0, 1.0) * 0.6;
        let (pl, pr) = pan_gains((key_pan * self.width).clamp(-1.0, 1.0));
        v.pan_l = pl;
        v.pan_r = pr;
        v.fade.reset();
        v.retune(bend_ratio);
        v.tilt.set(&x);
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        let rel = self.release;
        let sr = self.sr;
        self.pool.release_id(id, |v| damp(v, rel, sr));
    }

    fn all_notes_off(&mut self) {
        let rel = self.release;
        let sr = self.sr;
        self.pool.release_all(|v| damp(v, rel, sr));
    }

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
        self.clear_stage();
    }

    fn render(&mut self, ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.tempo = ctx.tempo_bpm.clamp(20.0, 400.0);
        if self.sync != 0 {
            self.lfo_dt = (self.tempo / 60.0 / SYNC_BEATS[self.sync - 1]) as f32 / self.sr;
            if ctx.playing {
                let cycles = ctx.beat_position / SYNC_BEATS[self.sync - 1];
                self.lfo.reset((cycles - cycles.floor()) as f32);
            }
        }
        let sr = self.sr;
        for n in 0..left.len().min(right.len()) {
            let lfo = self.lfo.tick(LfoShape::Sine, self.lfo_dt);
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let depth = self.trem_depth.get();
            let (gl, gr) = if self.mode == 0 {
                let g = 1.0 - depth * (0.5 - 0.5 * lfo);
                (g, g)
            } else {
                (
                    1.0 - depth * (0.5 - 0.5 * lfo),
                    1.0 - depth * (0.5 + 0.5 * lfo),
                )
            };
            let (mut l, mut r) = (0.0, 0.0);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick();
                l += a;
                r += b;
            }
            // Tremolo, then the preamp.
            let (mut l, mut r) = (l * gl, r * gr);
            let (g, b, norm) = (self.drive_gain, self.drive_bias, self.drive_norm);
            let bias_out = tanh_approx(b);
            l = (tanh_approx(l * g + b) - bias_out) * norm;
            r = (tanh_approx(r * g + b) - bias_out) * norm;
            // Tone stack: treble shelf, a soft top end, coupling highpass.
            self.lp[0] += self.tone_k * (l - self.lp[0]);
            self.lp[1] += self.tone_k * (r - self.lp[1]);
            let sg = self.shelf_g;
            let (l, r) = (l + sg * (l - self.lp[0]), r + sg * (r - self.lp[1]));
            self.top[0] += self.top_k * (l - self.top[0]);
            self.top[1] += self.top_k * (r - self.top[1]);
            self.hp[0] += self.hp_k * (self.top[0] - self.hp[0]);
            self.hp[1] += self.hp_k * (self.top[1] - self.hp[1]);
            let (l, r) = (self.top[0] - self.hp[0], self.top[1] - self.hp[1]);
            let c = self.chorus.get();
            let (l, r) = if c > 0.001 {
                self.chorus_fx.process(l, r, c, sr)
            } else {
                self.chorus_fx.push(l, r);
                (l, r)
            };
            left[n] = l * self.out_gain;
            right[n] = r * self.out_gain;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}

/// The damper touches the tine: every mode decays quickly, the overtones
/// faster than the fundamental, and the felt thuds.
fn damp(v: &mut TineVoice, release: f32, sr: f32) {
    let sr64 = sr as f64;
    let scale = [1.0, 0.8, 0.25, 0.1];
    for (r, scale) in v.r.iter_mut().zip(scale) {
        let t = (release as f64 * scale).max(0.004);
        let target = (-LN_1000 / (t * sr64)).exp();
        *r = target.min(*r);
    }
    // Thud: stronger while the tine is still ringing.
    let ring = (v.env / v.env0).clamp(0.0, 1.0).sqrt();
    v.th_k = 1.0 - (-TAU * 380.0 / sr).exp();
    v.th_coef = (-1.0 / (0.02 * sr)).exp();
    v.th_gain = v.thud * 0.35 / v.th_k;
    if v.th_env < 1.0e-3 && v.th_sm < 1.0e-3 {
        // Start the noise from silence so the thud has no click.
        v.th_lp = 0.0;
        v.th_lp2 = 0.0;
    }
    v.th_env = v.th_env.max(ring);
}
