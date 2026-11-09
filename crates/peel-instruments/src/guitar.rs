//! `peel.guitar.electric`: an electric guitar played through an amp.
//!
//! String: every note is a pair of digital waveguide loops, the vertical and
//! the horizontal polarization of one string, slightly detuned so they beat
//! the way a real string does. Each loop is an integer delay line with a
//! first-order Thiran all-pass for the fractional part (solved exactly at the
//! fundamental, so pitch stays accurate up the neck and through vibrato), a
//! two-stage stiffness all-pass that stretches the partials like a real steel
//! string (about +20 cents at the 12th partial of a low E), and a one-pole
//! loss filter whose damping is derived from measured guitar decay rates
//! (partial n loses about `kappa * n^2` dB/s on top of the fundamental's
//! decay; the FreePats direct recordings give `kappa` = 0.03 + 0.0003 * f0).
//! The string starts from a plucked triangle (peak at the pick position, which
//! gives the pick-position comb), low-passed by the pick hardness.
//!
//! Pickup: a magnetic pickup senses string velocity (a first difference of the
//! summed strings), at a position that is a fixed distance from the bridge, so
//! its comb moves with the fret (`distance / sounding length`). A humbucker
//! averages two taps. The inductance and cable capacitance give the pickup its
//! electrical resonance (single coil about 4.4 kHz, humbucker about 2.9 kHz).
//!
//! Amp: pickup into a shared tube-style preamp: tight high-pass and mid push,
//! two asymmetric soft-clipping stages with AC coupling between them, all
//! running at 4x oversampling with an 8th-order anti-alias filter, then a
//! Fender/Marshall style bass/mid/treble stack with its mid scoop, and a
//! power stage whose headroom sags under load. The cabinet is a short cascade
//! of filters fitted to published speaker and microphone responses: a
//! 1x12 open back, a 2x12 and a 4x12 closed back, each with the box and
//! speaker resonance, the cone presence peaks, and the steep roll-off.
//!
//! Playing: ghost notes at very low velocity, palm mute (damped with a low
//! thump), pick noise, finger-lift squeaks, slides and hammer-ons through
//! legato, a few milliseconds of stagger when notes start together (a
//! strum), per-note variation, and a vibrato that fades in. Sensible range is
//! MIDI 28 (E1, drop tunings) to 88 (E6).

use std::f32::consts::{LN_2, TAU};

use peel_dsp::biquad::{Biquad, Coeffs, FilterKind, butterworth};
use peel_dsp::delay::fast_sin_turns;
use peel_dsp::filter::tanh_approx;
use peel_dsp::noise::Noise;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, Vx, lerp, ramp_coef,
    vel_gain,
};

const DECAY: ParamId = 1;
const DAMPING: ParamId = 2;
const PICK_POS: ParamId = 3;
const BRIGHT: ParamId = 4;
const PALM: ParamId = 5;
const VIB_DEPTH: ParamId = 6;
const VIB_RATE: ParamId = 7;
const LEGATO: ParamId = 8;
const GLIDE: ParamId = 9;
const PICKUP_TYPE: ParamId = 10;
const PICKUP_POS: ParamId = 11;
const STRUM: ParamId = 12;
const PICK_NOISE: ParamId = 13;
const AMP_GAIN: ParamId = 20;
const AMP_BASS: ParamId = 21;
const AMP_MID: ParamId = 22;
const AMP_TREBLE: ParamId = 23;
const CAB_TONE: ParamId = 24;
const CAB_TYPE: ParamId = 25;
const AMP_SAG: ParamId = 26;
const LEVEL: ParamId = 30;
const VELOCITY: ParamId = 31;
const DIRECT: ParamId = 32;

const PICKUPS: &[&str] = &["Single", "Humbucker"];
const CABS: &[&str] = &["1x12 Open", "2x12", "4x12 Closed"];
const OUTPUTS: &[&str] = &["Amp", "Direct"];

static PARAMS: [ParamDescriptor; 23] = [
    ParamDescriptor::log(
        DECAY,
        "string.decay",
        "Decay",
        "String",
        0.3,
        30.0,
        12.0,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        DAMPING,
        "string.damping",
        "Damping",
        "String",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        PICK_POS,
        "string.pick_pos",
        "Pick Position",
        "String",
        0.05,
        0.5,
        0.16,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BRIGHT,
        "string.brightness",
        "Pick Brightness",
        "String",
        0.0,
        1.0,
        0.55,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        PICK_NOISE,
        "string.noise",
        "Pick Noise",
        "String",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        PALM,
        "string.palm_mute",
        "Palm Mute",
        "String",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        STRUM,
        "string.strum",
        "Strum",
        "String",
        0.0,
        30.0,
        4.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::choice(
        LEGATO,
        "string.legato",
        "Legato",
        "String",
        &["Off", "On"],
        0,
    ),
    ParamDescriptor::linear(
        GLIDE,
        "string.glide",
        "Glide",
        "String",
        0.0,
        400.0,
        0.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::linear(
        VIB_DEPTH,
        "vibrato.depth",
        "Vibrato Depth",
        "String",
        0.0,
        60.0,
        0.0,
        Unit::Cents,
    ),
    ParamDescriptor::log(
        VIB_RATE,
        "vibrato.rate",
        "Vibrato Rate",
        "String",
        2.0,
        9.0,
        5.5,
        Unit::Hertz,
    ),
    ParamDescriptor::choice(PICKUP_TYPE, "pickup.type", "Pickup", "Pickup", PICKUPS, 0),
    ParamDescriptor::linear(
        PICKUP_POS,
        "pickup.position",
        "Position",
        "Pickup",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        AMP_GAIN,
        "amp.gain",
        "Gain",
        "Amp",
        0.0,
        1.0,
        0.12,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        AMP_BASS,
        "amp.bass",
        "Bass",
        "Amp",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        AMP_MID,
        "amp.mid",
        "Mid",
        "Amp",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        AMP_TREBLE,
        "amp.treble",
        "Treble",
        "Amp",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        AMP_SAG,
        "amp.sag",
        "Sag",
        "Amp",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::choice(CAB_TYPE, "cab.type", "Cabinet", "Amp", CABS, 0),
    ParamDescriptor::log(
        CAB_TONE,
        "cab.tone",
        "Cab Tone",
        "Amp",
        3_000.0,
        8_000.0,
        5_200.0,
        Unit::Hertz,
    ),
    ParamDescriptor::choice(DIRECT, "out.direct", "Output", "Output", OUTPUTS, 0),
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
    type_id: "peel.guitar.electric",
    name: "Guitar",
    description: "Electric guitar: plucked string, pickup and amp with cabinet",
    category: "Guitar",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 9] = [
    Preset {
        name: "Clean",
        params: &[
            ("string.decay", 12.0),
            ("string.damping", 0.35),
            ("string.pick_pos", 0.16),
            ("string.brightness", 0.55),
            ("string.palm_mute", 0.0),
            ("string.strum", 4.0),
            ("string.legato", 0.0),
            ("vibrato.depth", 0.0),
            ("vibrato.rate", 5.5),
            ("pickup.type", 0.0),
            ("pickup.position", 0.7),
            ("amp.gain", 0.12),
            ("amp.bass", 0.0),
            ("amp.mid", 0.0),
            ("amp.treble", 0.0),
            ("amp.sag", 0.4),
            ("cab.type", 0.0),
            ("cab.tone", 5_200.0),
            ("out.level", 0.0),
            ("out.velocity", 0.7),
        ],
    },
    Preset {
        name: "Crunch",
        params: &[
            ("string.decay", 6.0),
            ("string.damping", 0.3),
            ("string.brightness", 0.6),
            ("string.strum", 7.0),
            ("pickup.type", 1.0),
            ("pickup.position", 0.8),
            ("amp.gain", 0.52),
            ("amp.bass", 1.0),
            ("amp.mid", 2.0),
            ("amp.treble", 1.0),
            ("amp.sag", 0.5),
            ("cab.type", 2.0),
            ("cab.tone", 5_000.0),
        ],
    },
    Preset {
        name: "Lead",
        params: &[
            ("string.decay", 12.0),
            ("string.damping", 0.2),
            ("string.pick_pos", 0.14),
            ("string.brightness", 0.65),
            ("string.legato", 1.0),
            ("string.glide", 70.0),
            ("vibrato.depth", 16.0),
            ("vibrato.rate", 5.6),
            ("pickup.type", 1.0),
            ("pickup.position", 0.9),
            ("amp.gain", 0.86),
            ("amp.bass", -1.0),
            ("amp.mid", 3.0),
            ("amp.treble", 1.0),
            ("amp.sag", 0.55),
            ("cab.type", 2.0),
            ("cab.tone", 4_600.0),
            ("out.level", 0.0),
            ("out.velocity", 0.45),
        ],
    },
    Preset {
        name: "Palm Mute",
        params: &[
            ("string.decay", 2.0),
            ("string.damping", 0.45),
            ("string.brightness", 0.5),
            ("string.palm_mute", 0.8),
            ("string.strum", 2.0),
            ("pickup.type", 1.0),
            ("pickup.position", 0.95),
            ("amp.gain", 0.74),
            ("amp.bass", 3.0),
            ("amp.mid", -1.0),
            ("amp.treble", 0.0),
            ("amp.sag", 0.5),
            ("cab.type", 2.0),
            ("cab.tone", 4_400.0),
            ("out.velocity", 0.6),
        ],
    },
    Preset {
        name: "Funk",
        params: &[
            ("string.decay", 1.1),
            ("string.damping", 0.2),
            ("string.pick_pos", 0.11),
            ("string.brightness", 0.85),
            ("string.palm_mute", 0.25),
            ("string.strum", 3.0),
            ("pickup.type", 0.0),
            ("pickup.position", 0.45),
            ("amp.gain", 0.1),
            ("amp.bass", -3.0),
            ("amp.mid", 0.0),
            ("amp.treble", 3.0),
            ("amp.sag", 0.3),
            ("cab.type", 0.0),
            ("cab.tone", 6_500.0),
            ("out.level", 3.0),
            ("out.velocity", 0.85),
        ],
    },
    Preset {
        name: "Blues",
        params: &[
            ("string.decay", 8.0),
            ("string.damping", 0.4),
            ("string.pick_pos", 0.2),
            ("string.brightness", 0.45),
            ("string.strum", 6.0),
            ("vibrato.depth", 10.0),
            ("vibrato.rate", 5.0),
            ("pickup.type", 0.0),
            ("pickup.position", 0.3),
            ("amp.gain", 0.34),
            ("amp.bass", 1.0),
            ("amp.mid", 1.0),
            ("amp.treble", -2.0),
            ("amp.sag", 0.6),
            ("cab.type", 1.0),
            ("cab.tone", 4_600.0),
        ],
    },
    Preset {
        name: "Muted Pluck",
        params: &[
            ("string.decay", 1.4),
            ("string.damping", 0.5),
            ("string.pick_pos", 0.13),
            ("string.brightness", 0.75),
            ("string.palm_mute", 0.55),
            ("string.noise", 0.6),
            ("string.strum", 0.0),
            ("pickup.type", 0.0),
            ("pickup.position", 0.6),
            ("amp.gain", 0.2),
            ("amp.bass", -2.0),
            ("amp.mid", 1.0),
            ("amp.treble", 2.0),
            ("amp.sag", 0.2),
            ("cab.type", 0.0),
            ("cab.tone", 6_000.0),
            ("out.level", 2.0),
            ("out.velocity", 0.8),
        ],
    },
    Preset {
        name: "Chorus Clean",
        params: &[
            ("string.decay", 9.0),
            ("string.damping", 0.3),
            ("string.pick_pos", 0.15),
            ("string.brightness", 0.6),
            ("string.strum", 9.0),
            ("pickup.type", 0.0),
            ("pickup.position", 0.55),
            ("amp.gain", 0.08),
            ("amp.bass", -1.0),
            ("amp.mid", -1.0),
            ("amp.treble", 3.0),
            ("amp.sag", 0.25),
            ("cab.type", 1.0),
            ("cab.tone", 6_800.0),
            ("out.level", 1.0),
        ],
    },
    Preset {
        name: "High Gain",
        params: &[
            ("string.decay", 8.0),
            ("string.damping", 0.3),
            ("string.pick_pos", 0.14),
            ("string.brightness", 0.6),
            ("string.strum", 5.0),
            ("pickup.type", 1.0),
            ("pickup.position", 0.95),
            ("amp.gain", 0.95),
            ("amp.bass", 1.0),
            ("amp.mid", -2.0),
            ("amp.treble", 1.0),
            ("amp.sag", 0.45),
            ("cab.type", 2.0),
            ("cab.tone", 4_800.0),
            ("out.velocity", 0.55),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Guitar::new()))
}

/// Lowest fundamental the string model can reach.
const MIN_HZ: f32 = 16.0;
/// Seconds for the release envelope to fall to about -100 dB.
const RELEASE_S: f32 = 0.3;
/// Loop T60 (seconds) of a released string.
const RELEASE_T60: f32 = 0.09;
/// Voice level below which a string is considered finished.
const DEAD: f32 = 1.0e-6;
/// Seconds the amp keeps running after the last voice ends.
const AMP_TAIL_S: f32 = 0.12;
/// Open string notes, low E to high E.
const OPEN_STRINGS: [u8; 6] = [40, 45, 50, 55, 59, 64];
/// Level of the horizontal polarization relative to the vertical one.
const B_LEVEL: f32 = 0.32;
/// Oversampling factor of the preamp.
const OS: usize = 4;
/// Scale from the pickup's velocity signal to the amp input.
const PICKUP_SCALE: f32 = 1.0;
/// Level from the pickup into the amp.
const AMP_IN: f32 = 1.4;
/// Depth of the pickup comb: the finite width of the pickup and the pick fill
/// the notches (a depth of 0.88 limits them to about -18 dB).
const COMB_DEPTH: f32 = 0.88;
const PU_SINGLE: (f32, f32) = (3_400.0, 0.8);
const PU_HUMBUCKER: (f32, f32) = (2_700.0, 0.8);
/// Pole of the leaky differentiator that models the pickup (corner near
/// 230 Hz at 48 kHz): velocity sensing above it, a flat response below.
const PICKUP_POLE: f32 = 0.97;
/// Level of the direct output.
const DI_GAIN: f32 = 0.7;

#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < 1.0e-20 { 0.0 } else { x }
}

/// Power-of-two ring buffer with integer and Hermite reads.
struct Ring {
    buf: Vec<f32>,
    mask: usize,
    w: usize,
}

impl Ring {
    fn new(max_delay: usize) -> Ring {
        let cap = (max_delay + 8).next_power_of_two().max(16);
        Ring {
            buf: vec![0.0; cap],
            mask: cap - 1,
            w: 0,
        }
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.w = 0;
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.buf[self.w] = x;
        self.w = (self.w + 1) & self.mask;
    }

    /// The sample pushed `d` pushes ago (`d >= 1`).
    #[inline]
    fn read(&self, d: usize) -> f32 {
        self.buf[self.w.wrapping_sub(d) & self.mask]
    }

    /// Adds `x` to the sample pushed `d` pushes ago.
    #[inline]
    fn add(&mut self, d: usize, x: f32) {
        let i = self.w.wrapping_sub(d) & self.mask;
        self.buf[i] += x;
    }

    /// Cubic Hermite read at a fractional delay `2 <= d < capacity - 3`.
    #[inline]
    fn hermite(&self, d: f32) -> f32 {
        let d = d.clamp(2.0, (self.mask - 3) as f32);
        let i = d as usize;
        let f = d - i as f32;
        let p3 = self.buf[self.w.wrapping_sub(i - 1) & self.mask];
        let p2 = self.buf[self.w.wrapping_sub(i) & self.mask];
        let p1 = self.buf[self.w.wrapping_sub(i + 1) & self.mask];
        let p0 = self.buf[self.w.wrapping_sub(i + 2) & self.mask];
        let t = 1.0 - f;
        let c1 = 0.5 * (p2 - p0);
        let c2 = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
        let c3 = 0.5 * (p3 - p0) + 1.5 * (p1 - p2);
        ((c3 * t + c2) * t + c1) * t + p1
    }
}

/// Stiffness coefficient `B` of a steel guitar string at fundamental `f`:
/// wound low strings are stiffer. Fitted to the partial deviations of the
/// FreePats direct recordings (E2 about 2e-4, E4 about 2.5e-5).
fn stiffness(f: f32) -> f32 {
    1.3e-5 + 1.9e-4 * (-(f - 80.0).max(0.0) / 70.0).exp()
}

/// One-pole loss coefficient giving partial `n` an extra loss of `kappa * n^2`
/// dB per second at fundamental `f`.
fn loss_coeff(kappa: f32, f: f32, sr: f32) -> f32 {
    let wf = TAU / sr;
    let q = kappa / (4.34 * wf * wf * f * f * f).max(1.0e-12);
    (2.0 * q / ((2.0 * q + 1.0) + (4.0 * q + 1.0).sqrt())).clamp(0.0, 0.92)
}

/// One polarization of a string: a loop with fractional-delay, dispersion
/// and loss filters.
struct StringLoop {
    ring: Ring,
    n: usize,
    /// Thiran all-pass coefficient and state.
    af: f32,
    fx: f32,
    fy: f32,
    /// Dispersion all-pass coefficient (two identical stages) and states.
    ad: f32,
    dx: [f32; 2],
    dy: [f32; 2],
    /// Loss filter coefficient, state and loop gain.
    c: f32,
    lp: f32,
    g: f32,
    /// Loop delay in samples and the pickup tap delays.
    period: f32,
    tap1: f32,
    tap2: f32,
}

impl StringLoop {
    fn new(max_delay: usize) -> StringLoop {
        StringLoop {
            ring: Ring::new(max_delay),
            n: 100,
            af: 0.0,
            fx: 0.0,
            fy: 0.0,
            ad: 0.0,
            dx: [0.0; 2],
            dy: [0.0; 2],
            c: 0.1,
            lp: 0.0,
            g: 0.99,
            period: 100.0,
            tap1: 10.0,
            tap2: 10.0,
        }
    }

    fn clear(&mut self) {
        self.ring.clear();
        self.fx = 0.0;
        self.fy = 0.0;
        self.dx = [0.0; 2];
        self.dy = [0.0; 2];
        self.lp = 0.0;
    }

    /// Re-tunes the loop for fundamental `f`, fundamental T60 `t60` and loss
    /// coefficient `c`. Exact at `f`: the phase delays of the loss, dispersion
    /// and fractional filters are subtracted from the loop length.
    fn tune(&mut self, sr: f32, f: f32, t60: f32, c: f32) {
        let f = f.min(sr / 6.0);
        let period = (sr / f).max(6.0);
        let w = TAU * f / sr;
        let (sw, cw) = w.sin_cos();

        // Loss filter phase delay and magnitude at the fundamental.
        let tau_loss = (c * sw).atan2(1.0 - c * cw) / w;
        let mag = (1.0 - c) / (1.0 - 2.0 * c * cw + c * c).sqrt();
        self.c = c;
        self.g = ((-6.907_755 * (period / sr) / t60.max(0.01)).exp() / mag.max(0.05)).min(0.9995);

        // Stiffness all-pass: pick k with k^3 - k = 3 P^3 B / (4 pi^2) so the
        // delay deficit at partial n gives f_n = n f0 sqrt(1 + B n^2).
        let x =
            3.0 * period * period * period * stiffness(f) / (4.0 * std::f32::consts::PI.powi(2));
        let mut tau_disp = 0.0;
        if x < 0.03 {
            self.ad = 0.0;
        } else {
            let mut k = x.cbrt().max(1.05);
            for _ in 0..4 {
                k -= (k * k * k - k - x) / (3.0 * k * k - 1.0);
            }
            let k = k.clamp(1.0, 12.0);
            self.ad = (1.0 - k) / (1.0 + k);
            tau_disp = 2.0 * 2.0 * (k * (0.5 * w).tan()).atan() / w;
        }

        // Integer plus fractional delay, with hysteresis on the split.
        let remaining = (period - tau_loss - tau_disp).max(2.0);
        let mut tau_f = remaining - self.n as f32;
        if !(0.4..=1.8).contains(&tau_f) {
            self.n = ((remaining - 0.5).floor() as usize).max(1);
            tau_f = remaining - self.n as f32;
        }
        let tau_f = tau_f.clamp(0.1, 1.9);
        let kf = (0.5 * w * tau_f).tan() / (0.5 * w).tan();
        self.af = (1.0 - kf) / (1.0 + kf);
        self.period = period;
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        let x = self.ring.read(self.n);
        let y = self.af * x + self.fx - self.af * self.fy;
        self.fx = x;
        self.fy = y;
        let mut s = y;
        if self.ad != 0.0 {
            for i in 0..2 {
                let o = self.ad * s + self.dx[i] - self.ad * self.dy[i];
                self.dx[i] = s;
                self.dy[i] = o;
                s = o;
            }
        }
        self.lp = (1.0 - self.c) * s + self.c * self.lp;
        let out = flush(self.lp * self.g);
        self.ring.push(out);
        out
    }

    /// Displacement at the pickup(s), taken just after `tick`.
    #[inline]
    fn pickup(&self, y: f32, humbucker: bool) -> f32 {
        let a = self.ring.hermite(self.tap1 + 1.0);
        if humbucker {
            let b = self.ring.hermite(self.tap2 + 1.0);
            y - COMB_DEPTH * 0.5 * (a + b)
        } else {
            y - COMB_DEPTH * a
        }
    }

    /// Writes (or adds) the excitation into the loop. `shape` holds
    /// `shape.len()` samples in push order.
    fn load(&mut self, shape: &[f32], gain: f32, additive: bool) {
        let len = shape.len();
        if additive {
            for (i, &s) in shape.iter().enumerate() {
                self.ring.add(len - i, s * gain);
            }
        } else {
            self.clear();
            for &s in shape {
                self.ring.push(s * gain);
            }
        }
    }
}

/// Noise bursts that do not travel through the string: the pick hitting it,
/// the low thump of a muted string, and the squeak of a finger leaving it.
#[derive(Clone, Copy, Default)]
struct Click {
    pick_env: f32,
    pick_dec: f32,
    pick_amp: f32,
    thump_env: f32,
    thump_dec: f32,
    thump_amp: f32,
    squeak_env: f32,
    squeak_dec: f32,
    squeak_amp: f32,
    prev: f32,
    lp1: f32,
    lp2: f32,
    sq_lp: f32,
    sq_prev: f32,
    lp_k: f32,
}

impl Click {
    #[inline]
    fn tick(&mut self, noise: &mut Noise) -> f32 {
        if self.pick_env < 1.0e-4 && self.thump_env < 1.0e-4 && self.squeak_env < 1.0e-4 {
            return 0.0;
        }
        let w = noise.white();
        let mut out = 0.0;
        if self.pick_env >= 1.0e-4 {
            // High-passed noise, a brief "tick".
            let hp = w - self.prev;
            out += hp * self.pick_env * self.pick_amp;
            self.pick_env *= self.pick_dec;
        }
        self.prev = w;
        if self.thump_env >= 1.0e-4 {
            self.lp1 += self.lp_k * (w - self.lp1);
            self.lp2 += self.lp_k * (self.lp1 - self.lp2);
            out += self.lp2 * self.thump_env * self.thump_amp;
            self.thump_env *= self.thump_dec;
        }
        if self.squeak_env >= 1.0e-4 {
            self.sq_lp += 0.45 * (w - self.sq_lp);
            let band = self.sq_lp - self.sq_prev;
            self.sq_prev = self.sq_lp;
            out += band * self.squeak_env * self.squeak_amp;
            self.squeak_env *= self.squeak_dec;
        }
        out
    }
}

struct GuitarVoice {
    active: bool,
    released: bool,
    /// Vertical and horizontal polarizations.
    a: StringLoop,
    b: StringLoop,
    /// Samples left before the string is plucked (strum stagger).
    pending: usize,
    /// Target and current pitch in Hz (they differ while sliding).
    f0: f32,
    f_cur: f32,
    /// Pickup position as a fraction of the sounding length from the bridge.
    beta: f32,
    /// Spacing of the humbucker coils as a fraction of the sounding length.
    coil: f32,
    /// Peak follower of the string signal, release envelope, velocity gain.
    lev: f32,
    rel: f32,
    level: f32,
    key: u8,
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
    /// Runtime voice ID, for per-note expression.
    id: u32,
    age: usize,
    start: u64,
    fade: Fade,
    /// Per-note variation and performance state.
    decay_mul: f32,
    detune: f32,
    bend: f32,
    ghost: f32,
    click: Click,
    /// Per-sample release envelope coefficient of this voice.
    rel_k: f32,
}

impl GuitarVoice {
    fn new(max_delay: usize) -> GuitarVoice {
        GuitarVoice {
            active: false,
            released: false,
            a: StringLoop::new(max_delay),
            b: StringLoop::new(max_delay),
            pending: 0,
            f0: 110.0,
            f_cur: 110.0,
            beta: 0.1,
            coil: 0.03,
            lev: 0.0,
            rel: 1.0,
            level: 1.0,
            key: 60,
            xgain: 1.0,
            id: 0,
            age: 0,
            start: 0,
            fade: Fade::new(),
            decay_mul: 1.0,
            detune: 0.002,
            bend: 0.0,
            ghost: 0.0,
            click: Click::default(),
            rel_k: 0.999,
        }
    }

    /// Recomputes both loops from the current settings. Runs at control rate.
    fn tune(&mut self, sr: f32, p: &StringParams, lfo: f32) {
        let age = self.age as f32;
        let onset = ((age - 0.2 * sr) / (0.5 * sr)).clamp(0.0, 1.0);
        let cents = p.vib_depth * lfo * onset
            + self.bend * (-age / (0.18 * sr)).exp()
            + p.bend_cents;
        let f = (self.f_cur * (cents * (LN_2 / 1200.0)).exp()).max(MIN_HZ);
        let mute = p.palm.max(self.ghost);

        // T60 falls with pitch (about f^-0.55 up to the top of the neck's
        // usual range, more slowly above).
        let pitch = if f < 440.0 {
            (110.0 / f).powf(0.55)
        } else {
            0.4 * (440.0 / f).powf(0.3)
        };
        let mut t60 = p.decay * self.decay_mul * pitch.clamp(0.2, 2.2);
        t60 *= 1.0 - 0.93 * mute;
        // A ghost note is a string barely touched by the fretting hand: it
        // dies in about a tenth of a second whatever the decay setting.
        t60 = lerp(t60, t60.min(0.12), self.ghost);
        let kappa = (0.03 + 0.0003 * f) * (0.35 + 1.86 * p.damping) * (1.0 + 14.0 * mute);
        let mut c = loss_coeff(kappa, f, sr);
        if self.released {
            t60 = RELEASE_T60;
            c = 0.5;
        }
        self.a.tune(sr, f, t60, c);
        let fb = f * (1.0 + self.detune);
        self.b.tune(sr, fb, t60 * 1.35, c);

        // Pickup taps follow the sounding length of each loop.
        let reach = |l: &mut StringLoop, beta: f32, coil: f32, hum: bool| {
            let half = 0.5 * l.period;
            if hum {
                l.tap1 = ((beta - 0.5 * coil) * l.period).clamp(2.0, half);
                l.tap2 = ((beta + 0.5 * coil) * l.period).clamp(2.0, half);
            } else {
                l.tap1 = (beta * l.period).clamp(2.0, half);
                l.tap2 = l.tap1;
            }
        };
        reach(&mut self.a, self.beta, self.coil, p.humbucker);
        reach(&mut self.b, self.beta * 0.8, self.coil, p.humbucker);
    }
}

/// Settings every voice needs while it is tuned.
#[derive(Clone, Copy)]
struct StringParams {
    decay: f32,
    damping: f32,
    palm: f32,
    vib_depth: f32,
    humbucker: bool,
    /// Live pitch bend in cents.
    bend_cents: f32,
}

impl StringParams {
    /// The settings for one voice: its own bend, and timbre lightening or
    /// darkening the string loss (brighter above 0.5).
    fn with(&self, x: &Vx) -> StringParams {
        StringParams {
            bend_cents: x.bend * 100.0,
            damping: (self.damping - 0.4 * x.tone).clamp(0.0, 1.0),
            ..*self
        }
    }
}

impl PoolVoice for GuitarVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.lev * self.level * self.rel
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
        self.lev = 0.0;
    }
}

/// Cabinet voicings: high-pass `(Hz, Q)`, seven peaks `(Hz, Q, dB)`, and the
/// ratio of the cab tone low-pass to the Cab Tone setting.
struct CabVoicing {
    hp: (f32, f32),
    peaks: [(f32, f32, f32); 7],
    lp_ratio: f32,
}

/// Fitted to published speaker and miked-cabinet responses: the closed-back
/// 4x12 with Celestion Vintage 30 style drivers and an SM57 near the cap edge
/// (resonance near 110 Hz, boxy dip near 400 Hz, presence at 2 and 3.5 kHz, a
/// dip before the steep roll-off above 5 kHz); the open-back 1x12 with a
/// Jensen style driver (less bass, resonance near 120 Hz, smoother, extends a
/// little higher); and a 2x12 in between.
const CABINETS: [CabVoicing; 3] = [
    CabVoicing {
        hp: (92.0, 0.65),
        peaks: [
            (125.0, 1.0, 2.8),
            (380.0, 1.0, -2.2),
            (1_150.0, 1.0, 1.2),
            (2_500.0, 1.1, 2.5),
            (3_900.0, 1.8, 1.5),
            (6_600.0, 2.5, -4.0),
            (9_000.0, 1.0, 0.0),
        ],
        lp_ratio: 1.12,
    },
    CabVoicing {
        hp: (84.0, 0.7),
        peaks: [
            (112.0, 1.1, 3.6),
            (350.0, 1.0, -2.4),
            (900.0, 1.0, 1.0),
            (2_200.0, 1.1, 3.4),
            (3_700.0, 1.9, 3.6),
            (6_200.0, 2.5, -4.5),
            (9_000.0, 1.0, 0.0),
        ],
        lp_ratio: 1.0,
    },
    CabVoicing {
        hp: (78.0, 0.75),
        peaks: [
            (108.0, 1.2, 4.6),
            (400.0, 1.1, -2.6),
            (800.0, 0.9, 1.2),
            (1_900.0, 1.1, 3.6),
            (3_500.0, 2.0, 4.4),
            (5_900.0, 3.0, -5.5),
            (7_800.0, 2.0, -3.0),
        ],
        lp_ratio: 0.96,
    },
];

/// The shared amp and cabinet. Mono in, mono out.
struct Amp {
    sr: f32,
    pickup: Biquad,
    cable: Biquad,
    pre_hp: Biquad,
    pre_mid: Biquad,
    up: [Biquad; 4],
    down: [Biquad; 4],
    ic_hp: f32,
    ic_hp_k: f32,
    ic_lp: f32,
    ic_lp_k: f32,
    dc: Biquad,
    low: Biquad,
    mid: Biquad,
    scoop: Biquad,
    high: Biquad,
    cab_hp: Biquad,
    cab_peaks: [Biquad; 7],
    cab_lp: [Biquad; 2],
    pre: f32,
    k2: f32,
    bias: f32,
    bias_out: f32,
    power: f32,
    makeup: f32,
    pickup_gain: f32,
    sag_env: f32,
    sag_amt: f32,
    sag_att: f32,
    sag_rel: f32,
    sag_param: f32,
    gain: f32,
}

impl Amp {
    fn new() -> Amp {
        let id = Biquad::new(Coeffs::IDENTITY);
        Amp {
            sr: 48_000.0,
            pickup: id,
            cable: id,
            pre_hp: id,
            pre_mid: id,
            up: [id; 4],
            down: [id; 4],
            ic_hp: 0.0,
            ic_hp_k: 0.01,
            ic_lp: 0.0,
            ic_lp_k: 0.3,
            dc: id,
            low: id,
            mid: id,
            scoop: id,
            high: id,
            cab_hp: id,
            cab_peaks: [id; 7],
            cab_lp: [id; 2],
            pre: 1.0,
            k2: 1.0,
            bias: 0.1,
            bias_out: tanh_approx(0.1),
            power: 1.0,
            makeup: 1.0,
            pickup_gain: 1.0,
            sag_env: 0.0,
            sag_amt: 0.0,
            sag_att: 0.01,
            sag_rel: 0.0005,
            sag_param: 0.4,
            gain: 0.12,
        }
    }

    fn reset(&mut self) {
        for b in [
            &mut self.pickup,
            &mut self.cable,
            &mut self.pre_hp,
            &mut self.pre_mid,
            &mut self.dc,
            &mut self.low,
            &mut self.mid,
            &mut self.scoop,
            &mut self.high,
            &mut self.cab_hp,
        ] {
            b.reset();
        }
        for b in self
            .up
            .iter_mut()
            .chain(self.down.iter_mut())
            .chain(self.cab_peaks.iter_mut())
            .chain(self.cab_lp.iter_mut())
        {
            b.reset();
        }
        self.ic_hp = 0.0;
        self.ic_lp = 0.0;
        self.sag_env = 0.0;
    }

    /// Fixed filters; called from `prepare`.
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        let sr_os = sr * OS as f32;
        let mut sec = [Coeffs::IDENTITY; 4];
        butterworth(false, 8, sr_os, 0.44 * sr, &mut sec);
        for (i, c) in sec.iter().enumerate() {
            self.up[i].coeffs = *c;
            self.down[i].coeffs = *c;
        }
        self.ic_hp_k = 1.0 - (-TAU * 70.0 / sr_os).exp();
        self.dc.coeffs = Coeffs::design(FilterKind::HighPass, sr, 22.0, 0.707, 0.0);
        self.sag_att = 1.0 - (-1.0 / (0.004 * sr)).exp();
        self.sag_rel = 1.0 - (-1.0 / (0.15 * sr)).exp();
        self.set_pickup(0);
        self.set_cab_type(0, 5_200.0);
        self.reset();
    }

    /// Pickup electrical resonance (inductance against cable capacitance).
    fn set_pickup(&mut self, kind: usize) {
        let (f, q, gain) = if kind == 0 {
            (PU_SINGLE.0, PU_SINGLE.1, 1.0)
        } else {
            (PU_HUMBUCKER.0, PU_HUMBUCKER.1, 1.5)
        };
        self.pickup.coeffs = Coeffs::design(FilterKind::LowPass, self.sr, f, q, 0.0);
        let fc = if kind == 0 { 2_900.0 } else { 2_400.0 };
        self.cable.coeffs = Coeffs::design(FilterKind::LowPass, self.sr, fc, 0.707, 0.0);
        self.pickup_gain = gain;
    }

    fn set_cab_type(&mut self, kind: usize, tone: f32) {
        let v = &CABINETS[kind.min(CABINETS.len() - 1)];
        self.cab_hp.coeffs = Coeffs::design(FilterKind::HighPass, self.sr, v.hp.0, v.hp.1, 0.0);
        for (b, &(f, q, db)) in self.cab_peaks.iter_mut().zip(v.peaks.iter()) {
            b.coeffs = Coeffs::design(FilterKind::Peak, self.sr, f, q, db);
        }
        let mut sec = [Coeffs::IDENTITY; 4];
        butterworth(false, 4, self.sr, tone * v.lp_ratio, &mut sec);
        self.cab_lp[0].coeffs = sec[0];
        self.cab_lp[1].coeffs = sec[1];
    }

    /// Bass, mid and treble in dB. The stack has the fixed mid scoop of a
    /// Fender style circuit at low gain, moving up and shallower towards a
    /// Marshall style voicing as gain rises.
    fn set_tone(&mut self, bass: f32, mid: f32, treble: f32) {
        let g = self.gain;
        self.low.coeffs = Coeffs::design(FilterKind::LowShelf, self.sr, 110.0, 0.7, bass);
        self.mid.coeffs = Coeffs::design(FilterKind::Peak, self.sr, 650.0, 0.7, mid);
        self.scoop.coeffs = Coeffs::design(
            FilterKind::Peak,
            self.sr,
            lerp(460.0, 700.0, g),
            0.55,
            lerp(-5.0, -3.0, g),
        );
        self.high.coeffs = Coeffs::design(FilterKind::HighShelf, self.sr, 3_200.0, 0.7, treble);
    }

    /// Gain-dependent staging for `gain` in 0..1.
    fn set_gain(&mut self, gain: f32) {
        let g = gain.clamp(0.0, 1.0);
        self.gain = g;
        self.pre = db_to_gain(-6.0 + 54.0 * g);
        self.k2 = 1.0 + 3.0 * g * g;
        self.bias = 0.12 + 0.1 * g;
        self.bias_out = tanh_approx(self.bias);
        self.power = 1.0 + 0.8 * g;
        // Rough loudness matching: hot settings are compressed, so they
        // need less make-up than clean ones.
        self.makeup = 0.9 / (1.0 + 1.6 * g);
        let hp = 70.0 + 280.0 * g * g;
        self.pre_hp.coeffs = Coeffs::design(FilterKind::HighPass, self.sr, hp, 0.707, 0.0);
        self.pre_mid.coeffs = Coeffs::design(FilterKind::Peak, self.sr, 750.0, 0.7, 7.0 * g);
        let lp = lerp(8_000.0, 4_500.0, g);
        self.ic_lp_k = 1.0 - (-TAU * lp / (self.sr * OS as f32)).exp();
        self.sag_amt = self.sag_param * (0.2 + 0.5 * g);
    }

    fn set_sag(&mut self, sag: f32) {
        self.sag_param = sag;
        self.sag_amt = sag * (0.2 + 0.5 * self.gain);
    }

    #[inline]
    fn stage1(&self, u: f32) -> f32 {
        tanh_approx(u + self.bias) - self.bias_out
    }

    #[inline]
    fn stage2(&self, u: f32) -> f32 {
        // Slightly harder on the negative swing, like a grid-biased triode.
        let p = u * self.k2;
        if p >= 0.0 {
            tanh_approx(p)
        } else {
            tanh_approx(p * 1.25) * 0.8
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let mut s = self.cable.process(self.pickup.process(x)) * self.pickup_gain * self.pre;
        s = self.pre_hp.process(s);
        s = self.pre_mid.process(s);
        // 4x oversampled preamp: zero-stuff, filter, shape, filter, decimate.
        let mut out = 0.0;
        for phase in 0..OS {
            let mut u = if phase == 0 { OS as f32 * s } else { 0.0 };
            for b in self.up.iter_mut() {
                u = b.process(u);
            }
            let a = self.stage1(u);
            // AC coupling between the stages and the Miller roll-off.
            self.ic_hp += self.ic_hp_k * (a - self.ic_hp);
            let a = a - self.ic_hp;
            self.ic_lp += self.ic_lp_k * (a - self.ic_lp);
            let mut z = self.stage2(self.ic_lp);
            for b in self.down.iter_mut() {
                z = b.process(z);
            }
            if phase == OS - 1 {
                out = z;
            }
        }
        let mut s = self.dc.process(out);
        s = self.low.process(s);
        s = self.mid.process(s);
        s = self.scoop.process(s);
        s = self.high.process(s);
        // Power stage: the supply sags under load, lowering the headroom.
        let h = (1.0 - self.sag_amt * self.sag_env).clamp(0.5, 1.0);
        let y = h * tanh_approx(s * self.power / h) / self.power;
        let a = y.abs();
        let k = if a > self.sag_env {
            self.sag_att
        } else {
            self.sag_rel
        };
        self.sag_env += k * (a - self.sag_env);
        s = self.cab_hp.process(y);
        for b in self.cab_peaks.iter_mut() {
            s = b.process(s);
        }
        s = self.cab_lp[0].process(s);
        s = self.cab_lp[1].process(s);
        tanh_approx(s * self.makeup)
    }
}

pub(crate) struct Guitar {
    expr: Expression,
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    pool: Pool<GuitarVoice>,
    amp: Amp,
    noise: Noise,
    tail: usize,
    rel_k: f32,
    lfo_phase: f32,
    lfo: f32,
    lfo_jit: f32,
    clock: u64,
    last_on: u64,
    have_last: bool,
    chord_idx: u32,
    prev_mix: f32,
    prev_x: f32,
    scratch: Vec<f32>,
    // Unsmoothed settings.
    pick_pos: f32,
    brightness: f32,
    vib_rate: f32,
    vel_sens: f32,
    pick_noise: f32,
    strum_ms: f32,
    legato: bool,
    glide_s: f32,
    glide_k: f32,
    pickup_type: usize,
    pickup_pos: f32,
    cab_type: usize,
    direct: bool,
    // Smoothed settings.
    decay: Ramp,
    damping: Ramp,
    palm: Ramp,
    vib_depth: Ramp,
    gain: Ramp,
    bass: Ramp,
    mid: Ramp,
    treble: Ramp,
    cab: Ramp,
    level: Ramp,
    // Last values the amp filters were designed for.
    designed: [f32; 5],
    designed_cab: (usize, f32),
    out_gain: f32,
}

fn max_delay(sr: f32) -> usize {
    (sr / MIN_HZ) as usize + 8
}

fn scratch_len(sr: f32) -> usize {
    3 * max_delay(sr) + 64
}

impl Guitar {
    pub(crate) fn new() -> Guitar {
        let md = max_delay(48_000.0);
        Guitar {
            expr: Expression::new(),
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            pool: Pool::new(14, 12, StealPolicy::Quietest, 48_000.0, || {
                GuitarVoice::new(md)
            }),
            amp: Amp::new(),
            noise: Noise::new(0x6717),
            tail: 0,
            rel_k: 0.9999,
            lfo_phase: 0.0,
            lfo: 0.0,
            lfo_jit: 0.0,
            clock: 0,
            last_on: 0,
            have_last: false,
            chord_idx: 0,
            prev_mix: 0.0,
            prev_x: 0.0,
            scratch: vec![0.0; scratch_len(48_000.0)],
            pick_pos: 0.16,
            brightness: 0.55,
            vib_rate: 5.5,
            vel_sens: 0.7,
            pick_noise: 0.5,
            strum_ms: 4.0,
            legato: false,
            glide_s: 0.0,
            glide_k: 1.0,
            pickup_type: 0,
            pickup_pos: 0.7,
            cab_type: 0,
            direct: false,
            decay: Ramp::new(12.0f32.ln()),
            damping: Ramp::new(0.35),
            palm: Ramp::new(0.0),
            vib_depth: Ramp::new(0.0),
            gain: Ramp::new(0.12),
            bass: Ramp::new(0.0),
            mid: Ramp::new(0.0),
            treble: Ramp::new(0.0),
            cab: Ramp::new(5_200.0f32.ln()),
            level: Ramp::new(0.0),
            designed: [f32::NAN; 5],
            designed_cab: (usize::MAX, f32::NAN),
            out_gain: 1.0,
        }
    }

    fn string_params(&self) -> StringParams {
        StringParams {
            decay: self.decay.get().exp(),
            damping: self.damping.get(),
            palm: self.palm.get(),
            vib_depth: self.vib_depth.get(),
            humbucker: self.pickup_type == 1,
            bend_cents: self.expr.bend_st() * 100.0,
        }
    }

    /// Base pickup distance from the bridge (fraction of the open string).
    fn pickup_beta(&self) -> f32 {
        lerp(0.27, 0.08, self.pickup_pos)
    }

    /// Ramps, LFO, amp design and string tuning, once per control interval.
    fn control(&mut self) {
        let k = self.ramp_k;
        self.decay.tick(k);
        self.damping.tick(k);
        self.palm.tick(k);
        self.vib_depth.tick(k);
        self.gain.tick(k);
        self.bass.tick(k);
        self.mid.tick(k);
        self.treble.tick(k);
        self.cab.tick(k);
        self.level.tick(k);
        self.out_gain = db_to_gain(self.level.get());
        self.expr.tick();

        // Vibrato: a sine whose rate wanders a few percent, as a hand does.
        self.lfo_jit += 0.004 * (self.noise.white() - 6.0 * self.lfo_jit);
        let rate = self.vib_rate * (1.0 + 0.1 * self.lfo_jit.clamp(-1.0, 1.0));
        self.lfo_phase += rate * CTRL as f32 / self.sr;
        self.lfo_phase -= self.lfo_phase.floor();
        self.lfo = fast_sin_turns(self.lfo_phase);

        let now = [
            self.gain.get(),
            self.bass.get(),
            self.mid.get(),
            self.treble.get(),
            self.cab.get(),
        ];
        if now[0] != self.designed[0] {
            self.amp.set_gain(now[0]);
        }
        if now[0] != self.designed[0]
            || now[1] != self.designed[1]
            || now[2] != self.designed[2]
            || now[3] != self.designed[3]
        {
            self.amp.set_tone(now[1], now[2], now[3]);
        }
        if now[4] != self.designed_cab.1 || self.cab_type != self.designed_cab.0 {
            self.amp.set_cab_type(self.cab_type, now[4].exp());
            self.designed_cab = (self.cab_type, now[4]);
        }
        self.designed = now;

        let p = self.string_params();
        let sr = self.sr;
        let lfo = self.lfo;
        let gk = self.glide_k;
        let ex = &self.expr;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            let x = ex.voice(v.id, v.key);
            v.xgain = x.gain();
            if v.pending > 0 {
                continue;
            }
            v.age += CTRL;
            if v.f_cur != v.f0 {
                let r = (v.f0 / v.f_cur).ln();
                v.f_cur = if r.abs() < 1.0e-4 || gk >= 1.0 {
                    v.f0
                } else {
                    v.f_cur * (r * gk).exp()
                };
            }
            v.tune(sr, &p.with(&x), lfo);
        }
    }

    /// Writes the pluck shape for a loop of delay `period` into `scratch` and
    /// returns its length: a triangle peaking at `pp`, low-passed at about
    /// `harmonic` times the fundamental by two one-pole passes.
    fn pluck_shape(&mut self, period: f32, pp: f32, harmonic: f32, sharp: f32) -> usize {
        let len = period.ceil() as usize + 4;
        let lap = period.ceil() as usize + 2;
        let total = (len + 2 * lap).min(self.scratch.len());
        let s = (-TAU * harmonic / period).exp().clamp(0.0, 0.98);
        let norm = 1.0 / (0.5 + 1.1 * sharp);
        let buf = &mut self.scratch[..total];
        for (j, x) in buf.iter_mut().enumerate() {
            let u = (j as f32 / period).fract();
            let tri = if u < pp {
                u / pp
            } else {
                (1.0 - u) / (1.0 - pp)
            } - 0.5;
            // The rectangle is the sharp release of a hard pick.
            let rect = if u < pp { 1.0 } else { -pp / (1.0 - pp) };
            *x = (tri + sharp * rect) * norm;
        }
        for _ in 0..2 {
            let mut y = buf[0];
            for x in buf.iter_mut() {
                y += (1.0 - s) * (*x - y);
                *x = y;
            }
        }
        // Keep the tail: it is the settled periodic part.
        let off = total - len;
        self.scratch.copy_within(off..total, 0);
        len
    }
}

impl Synth for Guitar {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        let md = max_delay(sample_rate);
        self.pool = Pool::new(14, 12, StealPolicy::Quietest, sample_rate, || {
            GuitarVoice::new(md)
        });
        self.scratch = vec![0.0; scratch_len(sample_rate)];
        self.amp.prepare(sample_rate);
        self.designed = [f32::NAN; 5];
        self.designed_cab = (usize::MAX, f32::NAN);
        self.rel_k = (-11.512_925 / (RELEASE_S * sample_rate)).exp();
        self.tail = 0;
        self.ctrl = 0;
        self.clock = 0;
        self.last_on = 0;
        self.have_last = false;
        self.prev_mix = 0.0;
        self.prev_x = 0.0;
        self.set_glide();
        self.amp.set_pickup(self.pickup_type);
        self.amp.set_sag(self.amp.sag_param);
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.amp.reset();
        self.noise.reseed(0x6717);
        self.tail = 0;
        self.lfo_phase = 0.0;
        self.lfo_jit = 0.0;
        self.ctrl = 0;
        self.prev_mix = 0.0;
        self.prev_x = 0.0;
        self.chord_idx = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            DECAY => self.decay.set(v.ln(), imm),
            DAMPING => self.damping.set(v, imm),
            PICK_POS => self.pick_pos = v,
            BRIGHT => self.brightness = v,
            PICK_NOISE => self.pick_noise = v,
            PALM => self.palm.set(v, imm),
            STRUM => self.strum_ms = v,
            LEGATO => self.legato = v >= 0.5,
            GLIDE => {
                self.glide_s = v * 1.0e-3;
                self.set_glide();
            }
            VIB_DEPTH => self.vib_depth.set(v, imm),
            VIB_RATE => self.vib_rate = v,
            PICKUP_TYPE => {
                self.pickup_type = v as usize;
                self.amp.set_pickup(self.pickup_type);
            }
            PICKUP_POS => self.pickup_pos = v,
            AMP_GAIN => self.gain.set(v, imm),
            AMP_BASS => self.bass.set(v, imm),
            AMP_MID => self.mid.set(v, imm),
            AMP_TREBLE => self.treble.set(v, imm),
            AMP_SAG => self.amp.set_sag(v),
            CAB_TYPE => self.cab_type = v as usize,
            CAB_TONE => self.cab.set(v.ln(), imm),
            DIRECT => self.direct = v >= 0.5,
            LEVEL => self.level.set(v, imm),
            VELOCITY => self.vel_sens = v,
            _ => {}
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let sr = self.sr;
        let chordy = self.have_last && self.clock - self.last_on <= 3;
        self.chord_idx = if chordy { self.chord_idx + 1 } else { 0 };
        self.last_on = self.clock;
        self.have_last = true;

        // Legato: a new note while another is held (and is not part of a
        // chord) continues that string instead of plucking a new one.
        let mut src = None;
        if self.legato && !chordy {
            let min_age = (0.04 * sr) as usize;
            // Only in a single-note context: with several strings held this
            // is a chord or a double stop, not a legato line.
            let held = |v: &&GuitarVoice| v.active && !v.released;
            if self.pool.voices.iter().filter(held).count() == 1 {
                src = self
                    .pool
                    .voices
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| held(v) && v.pending == 0 && v.age > min_age)
                    .map(|(i, _)| i)
                    .next();
            }
        }

        let slot = self.pool.begin(key, id);
        let mut legato_from = None;
        if let Some(s) = src {
            if s == slot {
                legato_from = Some(slot);
            } else if self.pool.voices[s].active {
                let (a, b) = if s < slot {
                    let (l, r) = self.pool.voices.split_at_mut(slot);
                    (&mut l[s], &mut r[0])
                } else {
                    let (l, r) = self.pool.voices.split_at_mut(s);
                    (&mut r[0], &mut l[slot])
                };
                // `a` is the held string, `b` the new voice.
                std::mem::swap(&mut a.a, &mut b.a);
                std::mem::swap(&mut a.b, &mut b.b);
                b.active = true;
                b.f_cur = a.f_cur;
                b.age = a.age;
                b.lev = a.lev.max(0.05);
                a.kill();
                legato_from = Some(slot);
                self.pool.sync();
            }
        }
        let legato = legato_from.is_some();

        // Per-note variation.
        let r1 = self.noise.white();
        let r2 = self.noise.white();
        let r3 = self.noise.uniform();
        let r4 = self.noise.uniform();

        let p = self.string_params();
        let f0 = key_to_hz(key as f32).max(MIN_HZ);
        let ghost = ((0.22 - velocity) / 0.14).clamp(0.0, 1.0);
        let vel_adj = velocity * (1.0 - 0.025 * self.chord_idx.min(5) as f32);
        let bright =
            (self.brightness + 0.6 * (vel_adj - 0.6) * self.vel_sens + 0.03 * r2).clamp(0.0, 1.0);
        // Pick hardness: the highest harmonic the pluck excites well.
        let harmonic = lerp(12.0, 60.0, bright);
        let sharp = 1.9 * bright.powf(1.2);
        let pp = (self.pick_pos * (1.0 + 0.04 * r1)).clamp(0.04, 0.5);

        // Which string and fret: the highest open string at or below the key.
        // A fixed pickup distance means the pickup sits further along a
        // fretted (shorter) string.
        let open = OPEN_STRINGS
            .iter()
            .rev()
            .copied()
            .find(|&o| o <= key)
            .unwrap_or(OPEN_STRINGS[0]);
        let fret = (key as f32 - open as f32).max(0.0);
        let stretch = (fret / 12.0).exp2();

        let strum_samples = self.chord_idx.min(7) as f32
            * self.strum_ms
            * 1.0e-3
            * sr
            * (0.75 + 0.5 * self.noise.uniform());
        let pending = if legato { 0 } else { strum_samples as usize };

        let sens = self.vel_sens;
        let pick_noise = self.pick_noise;
        let palm = p.palm;
        let beta = self.pickup_beta();
        let x = self.expr.voice(id, key);
        let xgain = x.gain();

        {
            let v = &mut self.pool.voices[slot];
            v.active = true;
            v.released = false;
            v.f0 = f0;
            v.key = key;
            v.id = id;
            v.xgain = xgain;
            if !legato {
                v.f_cur = f0;
                v.age = 0;
                v.lev = 1.0;
            }
            v.level = vel_gain(vel_adj, sens) * (1.0 + 0.06 * r2);
            v.rel = 1.0;
            v.start = self.clock;
            v.fade.reset();
            v.pending = pending;
            v.beta = (beta * stretch).min(0.45);
            v.coil = 0.028 * stretch;
            v.decay_mul = 1.0 + 0.07 * r1;
            v.detune = 0.0012 + 0.0035 * r3;
            // Plucking tension raises the pitch briefly, more for hard
            // plucks on thick strings.
            v.bend = if legato {
                0.0
            } else {
                (1.5 + 3.5 * vel_adj * vel_adj) * (110.0 / f0).clamp(0.4, 1.5).sqrt()
            };
            v.ghost = ghost;
            v.click = Click::default();
        }

        // Excite the string.
        let tilt = (110.0 / f0).powf(0.25).clamp(0.4, 1.6);
        let amp = tilt * 0.5;
        let inject = if legato {
            // Hammer-on or pull-off: a light re-excitation.
            0.45 * vel_adj
        } else {
            1.0
        };
        // Tune once so the loops have their lengths before loading.
        {
            let lfo = self.lfo;
            let v = &mut self.pool.voices[slot];
            if !legato {
                v.a.clear();
                v.b.clear();
            }
            v.tune(sr, &p.with(&x), lfo);
        }
        let (pa, pb) = {
            let v = &self.pool.voices[slot];
            (v.a.period, v.b.period)
        };
        let la = self.pluck_shape(pa, pp, harmonic, sharp);
        let (scratch, voices) = (&self.scratch, &mut self.pool.voices);
        voices[slot].a.load(&scratch[..la], amp * inject, legato);
        let lb = self.pluck_shape(pb, (pp * 1.4).min(0.5), harmonic, sharp);
        let v = &mut self.pool.voices[slot];
        v.b.load(&self.scratch[..lb], amp * inject * B_LEVEL, legato);

        // Clicks: pick tick, and the thump of a muted string.
        let mute = palm.max(ghost);
        let c = &mut v.click;
        let nz = pick_noise;
        c.pick_env = 1.0;
        c.pick_dec = (-1.0 / (0.0018 * sr)).exp();
        c.pick_amp = if legato {
            0.0
        } else {
            nz * 0.22 * (0.3 + vel_adj) * (1.0 + 2.5 * ghost)
        };
        c.thump_env = 1.0;
        c.thump_dec = (-1.0 / (0.014 * sr)).exp();
        c.thump_amp = nz
            * (0.35 * mute * (1.0 + ghost) + 0.08 * r4 * ((110.0 / f0).clamp(0.3, 1.0)))
            * (0.4 + vel_adj);
        c.lp_k = 1.0 - (-TAU * 380.0 / sr).exp();
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        let sr = self.sr;
        let squeak_p = 0.4 * self.pick_noise;
        let r = self.noise.uniform();
        let amp = 0.5 + self.noise.uniform();
        let palm = self.palm.get();
        let rel_k = self.rel_k;
        self.pool.release_id(id, |v| {
            v.released = true;
            v.rel_k = rel_k.powf(1.0 + 3.0 * palm.max(v.ghost));
            // The finger leaving a string squeaks now and then.
            if v.age as f32 > 0.15 * sr && r < squeak_p && palm < 0.4 && v.ghost == 0.0 {
                v.click.squeak_env = 1.0;
                v.click.squeak_dec = (-1.0 / (0.03 * sr)).exp();
                v.click.squeak_amp = 0.03 * amp;
            }
        });
    }

    fn all_notes_off(&mut self) {
        let rel_k = self.rel_k;
        self.pool.release_all(|v| {
            v.released = true;
            v.rel_k = rel_k;
        });
    }

    fn all_sound_off(&mut self) {
        self.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        let tail_len = (AMP_TAIL_S * self.sr) as usize;
        let hum = self.pickup_type == 1;
        for n in 0..left.len().min(right.len()) {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            self.clock += 1;
            let mut mix = 0.0;
            let mut clicks = 0.0;
            let mut any = false;
            let noise = &mut self.noise;
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                any = true;
                if v.pending > 0 {
                    v.pending -= 1;
                    continue;
                }
                let ya = v.a.tick();
                let yb = v.b.tick();
                if !(ya + yb).is_finite() {
                    v.active = false;
                    v.lev = 0.0;
                    continue;
                }
                let pa = v.a.pickup(ya, hum);
                let pb = v.b.pickup(yb, hum);
                v.lev = (ya.abs() + yb.abs()).max(v.lev * 0.9995);
                if v.released {
                    v.rel *= v.rel_k;
                }
                let g = v.level * v.rel * v.fade.tick() * v.xgain;
                mix += (pa + pb) * g;
                clicks += v.click.tick(noise) * v.level * v.rel;
                if v.fade.is_done() || v.rel < 1.0e-5 || v.lev < DEAD {
                    v.active = false;
                }
            }
            if any {
                self.tail = tail_len;
            }
            // The pickup senses string velocity.
            let vel = (mix - self.prev_x) * PICKUP_SCALE + PICKUP_POLE * self.prev_mix;
            self.prev_x = mix;
            let vel = flush(vel);
            self.prev_mix = vel;
            let out = if self.tail > 0 {
                if !any {
                    self.tail -= 1;
                    if self.tail == 0 {
                        self.amp.reset();
                    }
                }
                let x = vel + clicks;
                if self.direct {
                    self.amp.cable.process(self.amp.pickup.process(x))
                        * self.amp.pickup_gain
                        * DI_GAIN
                        * self.out_gain
                } else {
                    self.amp.process(x * AMP_IN) * self.out_gain
                }
            } else {
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
        self.pool.is_silent() && self.tail == 0
    }
}

impl Guitar {
    fn set_glide(&mut self) {
        // Time constant of a third of the glide time (95% at the full time).
        self.glide_k = if self.glide_s <= 0.0 {
            1.0
        } else {
            1.0 - (-(CTRL as f32) / (self.sr * self.glide_s / 3.0)).exp()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cab_db(amp: &Amp, f: f32) -> f32 {
        let sr = amp.sr;
        let mut db = amp.cab_hp.coeffs.magnitude_db(sr, f);
        for b in amp.cab_peaks.iter().chain(amp.cab_lp.iter()) {
            db += b.coeffs.magnitude_db(sr, f);
        }
        db
    }

    /// Prints the cabinet responses (run with `--ignored --nocapture`).
    #[test]
    #[ignore]
    fn print_cab_responses() {
        let mut amp = Amp::new();
        amp.prepare(48_000.0);
        let freqs: Vec<f32> = (0..30)
            .map(|i| 50.0 * 2f32.powf(i as f32 / 3.0))
            .filter(|f| *f < 20_000.0)
            .collect();
        for kind in 0..3 {
            amp.set_cab_type(kind, 5_200.0);
            let row: Vec<String> = freqs
                .iter()
                .map(|&f| format!("{:.0}:{:.1}", f, cab_db(&amp, f)))
                .collect();
            println!("CAB{kind} {}", row.join(" "));
        }
    }
}
