//! `peel.guitar.electric`: an electric guitar played through an amp.
//!
//! Each note is a plucked string, a Karplus-Strong style waveguide: a delay
//! loop with a one-pole loss filter and a Hermite fractional delay. The loop
//! delay is corrected for the filter's phase delay at the fundamental, so
//! pitch stays accurate up the neck. The string starts from a triangular
//! pluck shape (peak at the pick position, which gives the pick-position
//! comb) blended with a sharper, brighter component and a little pick noise;
//! velocity opens the brightness. A magnetic pickup is modelled as a comb at
//! the pickup position on each string, followed by the pickup's electrical
//! resonance on the mixed signal.
//!
//! The summed strings feed one shared amp, so chords intermodulate the way
//! they do on a real amp: pre-gain, a tightening high-pass and mid push, an
//! oversampled two-stage asymmetric clipper, a bass/mid/treble tone stack, a
//! gentle power stage, and a 4x12 style cabinet (low resonance, upper-mid
//! bump, steep roll-off above a few kHz).
//!
//! Palm mute shortens the decay and darkens the loss filter. Vibrato is a
//! shared LFO that fades in on each note. Sensible range is MIDI 28 (E1,
//! drop tunings) to 88 (E6); lower and higher keys still sound but are
//! clamped to the string model's limits.

use peel_dsp::biquad::{Biquad, Coeffs, FilterKind, butterworth};
use peel_dsp::delay::{DelayLine, fast_sin_turns};
use peel_dsp::filter::tanh_approx;
use peel_dsp::noise::Noise;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, lerp, ramp_coef, vel_gain,
};

const DECAY: ParamId = 1;
const DAMPING: ParamId = 2;
const PICK_POS: ParamId = 3;
const BRIGHT: ParamId = 4;
const PALM: ParamId = 5;
const VIB_DEPTH: ParamId = 6;
const VIB_RATE: ParamId = 7;
const PICKUP_TYPE: ParamId = 10;
const PICKUP_POS: ParamId = 11;
const AMP_GAIN: ParamId = 20;
const AMP_BASS: ParamId = 21;
const AMP_MID: ParamId = 22;
const AMP_TREBLE: ParamId = 23;
const CAB_TONE: ParamId = 24;
const LEVEL: ParamId = 30;
const VELOCITY: ParamId = 31;

const PICKUPS: &[&str] = &["Single", "Humbucker"];

static PARAMS: [ParamDescriptor; 16] = [
    ParamDescriptor::log(
        DECAY,
        "string.decay",
        "Decay",
        "String",
        0.3,
        12.0,
        4.0,
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
    ParamDescriptor::log(
        CAB_TONE,
        "cab.tone",
        "Cabinet",
        "Amp",
        3_000.0,
        8_000.0,
        5_200.0,
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
    type_id: "peel.guitar.electric",
    name: "Guitar",
    description: "Electric guitar: plucked string, pickup and amp with cabinet",
    category: "Guitar",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Clean",
        params: &[
            ("string.decay", 4.0),
            ("string.damping", 0.35),
            ("string.pick_pos", 0.16),
            ("string.brightness", 0.55),
            ("string.palm_mute", 0.0),
            ("vibrato.depth", 0.0),
            ("vibrato.rate", 5.5),
            ("pickup.type", 0.0),
            ("pickup.position", 0.7),
            ("amp.gain", 0.12),
            ("amp.bass", 0.0),
            ("amp.mid", 0.0),
            ("amp.treble", 0.0),
            ("cab.tone", 5_200.0),
            ("out.level", 0.0),
            ("out.velocity", 0.7),
        ],
    },
    Preset {
        name: "Crunch",
        params: &[
            ("string.decay", 4.5),
            ("string.damping", 0.3),
            ("string.brightness", 0.6),
            ("pickup.type", 1.0),
            ("pickup.position", 0.8),
            ("amp.gain", 0.52),
            ("amp.bass", 1.0),
            ("amp.mid", 2.0),
            ("amp.treble", 1.0),
            ("cab.tone", 5_000.0),
        ],
    },
    Preset {
        name: "Lead",
        params: &[
            ("string.decay", 10.0),
            ("string.damping", 0.2),
            ("string.pick_pos", 0.14),
            ("string.brightness", 0.65),
            ("vibrato.depth", 16.0),
            ("vibrato.rate", 5.6),
            ("pickup.type", 1.0),
            ("pickup.position", 0.9),
            ("amp.gain", 0.86),
            ("amp.bass", -1.0),
            ("amp.mid", 3.0),
            ("amp.treble", 1.0),
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
            ("pickup.type", 1.0),
            ("pickup.position", 0.95),
            ("amp.gain", 0.74),
            ("amp.bass", 3.0),
            ("amp.mid", -1.0),
            ("amp.treble", 0.0),
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
            ("pickup.type", 0.0),
            ("pickup.position", 0.45),
            ("amp.gain", 0.1),
            ("amp.bass", -3.0),
            ("amp.mid", 0.0),
            ("amp.treble", 3.0),
            ("cab.tone", 6_500.0),
            ("out.level", 2.0),
            ("out.velocity", 0.85),
        ],
    },
    Preset {
        name: "Blues",
        params: &[
            ("string.decay", 5.0),
            ("string.damping", 0.4),
            ("string.pick_pos", 0.2),
            ("string.brightness", 0.45),
            ("vibrato.depth", 10.0),
            ("vibrato.rate", 5.0),
            ("pickup.type", 0.0),
            ("pickup.position", 0.3),
            ("amp.gain", 0.34),
            ("amp.bass", 1.0),
            ("amp.mid", 1.0),
            ("amp.treble", -2.0),
            ("cab.tone", 4_600.0),
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

#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < 1.0e-20 { 0.0 } else { x }
}

/// Settings every voice needs while it is tuned.
#[derive(Clone, Copy)]
struct StringParams {
    decay: f32,
    damping: f32,
    palm: f32,
    vib_depth: f32,
    /// Pickup position as a fraction of the string length from the bridge.
    pickup: f32,
}

struct GuitarVoice {
    active: bool,
    released: bool,
    line: DelayLine,
    f0: f32,
    /// Loop delay, pickup tap delay, loop gain, loss coefficient, loss state.
    d: f32,
    dpick: f32,
    g: f32,
    c: f32,
    lp: f32,
    /// Peak follower of the string signal, release envelope, velocity gain.
    lev: f32,
    rel: f32,
    level: f32,
    age: usize,
    fade: Fade,
}

impl GuitarVoice {
    fn new(max_delay: usize) -> GuitarVoice {
        GuitarVoice {
            active: false,
            released: false,
            line: DelayLine::new(max_delay),
            f0: 110.0,
            d: 100.0,
            dpick: 10.0,
            g: 0.99,
            c: 0.1,
            lp: 0.0,
            lev: 0.0,
            rel: 1.0,
            level: 1.0,
            age: 0,
            fade: Fade::new(),
        }
    }

    /// Recomputes the loop from the current settings. Runs at control rate.
    fn tune(&mut self, sr: f32, p: &StringParams, lfo: f32) {
        let onset = ((self.age as f32 - 0.2 * sr) / (0.5 * sr)).clamp(0.0, 1.0);
        let cents = p.vib_depth * lfo * onset;
        let f = (self.f0 * (cents * (std::f32::consts::LN_2 / 1200.0)).exp()).max(MIN_HZ);
        let period = (sr / f).max(6.0);

        let mut t60 = p.decay * (110.0 / f).powf(0.25).clamp(0.5, 1.6);
        t60 *= 1.0 - 0.93 * p.palm;
        let mut c = (0.03 + 0.55 * p.damping) * (150.0 / f).clamp(0.25, 1.5);
        c += 0.4 * p.palm * (1.0 - c);
        if self.released {
            t60 = RELEASE_T60;
            c = 0.5;
        }
        let c = c.clamp(0.0, 0.92);

        // One-pole loss filter: phase delay and magnitude at the fundamental.
        let w = std::f32::consts::TAU * f / sr;
        let (sw, cw) = w.sin_cos();
        let tau = (c * sw).atan2(1.0 - c * cw) / w;
        let mag = (1.0 - c) / (1.0 - 2.0 * c * cw + c * c).sqrt();
        let g = (-6.907_755 * (period / sr) / t60.max(0.01)).exp() / mag.max(0.05);

        self.c = c;
        self.g = g.min(0.9995);
        self.d = (period - tau).max(2.5);
        self.dpick = (p.pickup * period).clamp(2.0, period * 0.5);
    }

    /// Fills the loop with the pluck. `bright` is 0..1.
    fn pluck(&mut self, sr: f32, p: &StringParams, pick_pos: f32, bright: f32, noise: &mut Noise) {
        self.line.clear();
        self.lp = 0.0;
        let period = (sr / self.f0.max(MIN_HZ)).max(6.0);
        let pp = pick_pos.clamp(0.04, 0.5);
        let b = bright * bright;
        let n = (period + 3.0) as usize + 1;
        let mut prev_noise = 0.0;
        // Triangle (+-0.5) plus a rectangle component plus pick noise.
        let scale = 1.0 / (0.5 + 1.1 * b + 0.2 * bright);
        for j in 0..n {
            let mut u = j as f32 / period;
            if u >= 1.0 {
                u -= 1.0;
            }
            let tri = if u < pp {
                u / pp
            } else {
                (1.0 - u) / (1.0 - pp)
            } - 0.5;
            let rect = if u < pp { 1.0 } else { -pp / (1.0 - pp) };
            let w = noise.white();
            let hp_noise = w - prev_noise;
            prev_noise = w;
            let x = tri + 1.1 * b * rect + 0.1 * bright * hp_noise;
            self.line.push(x * scale);
        }
        self.tune(sr, p, 0.0);
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

/// The shared amp and cabinet. Mono in, mono out.
struct Amp {
    sr: f32,
    pickup: Biquad,
    pre_hp: Biquad,
    pre_mid: Biquad,
    up: [Biquad; 2],
    down: [Biquad; 2],
    dc: Biquad,
    low: Biquad,
    mid: Biquad,
    high: Biquad,
    cab_hp: Biquad,
    cab_low: Biquad,
    cab_mid: Biquad,
    cab_lp: [Biquad; 2],
    pre: f32,
    k2: f32,
    bias: f32,
    bias_out: f32,
    power: f32,
    makeup: f32,
    pickup_gain: f32,
}

impl Amp {
    fn new() -> Amp {
        let id = Biquad::new(Coeffs::IDENTITY);
        Amp {
            sr: 48_000.0,
            pickup: id,
            pre_hp: id,
            pre_mid: id,
            up: [id; 2],
            down: [id; 2],
            dc: id,
            low: id,
            mid: id,
            high: id,
            cab_hp: id,
            cab_low: id,
            cab_mid: id,
            cab_lp: [id; 2],
            pre: 1.0,
            k2: 1.0,
            bias: 0.1,
            bias_out: tanh_approx(0.1),
            power: 1.0,
            makeup: 1.0,
            pickup_gain: 1.0,
        }
    }

    fn reset(&mut self) {
        for b in [
            &mut self.pickup,
            &mut self.pre_hp,
            &mut self.pre_mid,
            &mut self.dc,
            &mut self.low,
            &mut self.mid,
            &mut self.high,
            &mut self.cab_hp,
            &mut self.cab_low,
            &mut self.cab_mid,
        ] {
            b.reset();
        }
        for b in self
            .up
            .iter_mut()
            .chain(self.down.iter_mut())
            .chain(self.cab_lp.iter_mut())
        {
            b.reset();
        }
    }

    /// Fixed filters; called from `prepare`.
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        let sr2 = sr * 2.0;
        let mut sec = [Coeffs::IDENTITY; 4];
        butterworth(false, 4, sr2, 0.17 * sr2, &mut sec);
        self.up[0].coeffs = sec[0];
        self.up[1].coeffs = sec[1];
        self.down[0].coeffs = sec[0];
        self.down[1].coeffs = sec[1];
        self.dc.coeffs = Coeffs::design(FilterKind::HighPass, sr, 22.0, 0.707, 0.0);
        self.cab_hp.coeffs = Coeffs::design(FilterKind::HighPass, sr, 78.0, 0.75, 0.0);
        self.cab_low.coeffs = Coeffs::design(FilterKind::Peak, sr, 105.0, 1.1, 4.5);
        self.cab_mid.coeffs = Coeffs::design(FilterKind::Peak, sr, 2_500.0, 0.9, 2.5);
        self.set_pickup(0);
        self.reset();
    }

    fn set_pickup(&mut self, kind: usize) {
        let (f, q, db, gain) = if kind == 0 {
            (4_200.0, 1.4, 5.0, 1.0)
        } else {
            (2_600.0, 1.2, 4.0, 1.5)
        };
        self.pickup.coeffs = Coeffs::design(FilterKind::Peak, self.sr, f, q, db);
        self.pickup_gain = gain;
    }

    fn set_cab(&mut self, hz: f32) {
        let mut sec = [Coeffs::IDENTITY; 4];
        butterworth(false, 4, self.sr, hz, &mut sec);
        self.cab_lp[0].coeffs = sec[0];
        self.cab_lp[1].coeffs = sec[1];
    }

    fn set_tone(&mut self, bass: f32, mid: f32, treble: f32) {
        self.low.coeffs = Coeffs::design(FilterKind::LowShelf, self.sr, 120.0, 0.7, bass);
        self.mid.coeffs = Coeffs::design(FilterKind::Peak, self.sr, 650.0, 0.7, mid);
        self.high.coeffs = Coeffs::design(FilterKind::HighShelf, self.sr, 3_200.0, 0.7, treble);
    }

    /// Gain-dependent staging for `gain` in 0..1.
    fn set_gain(&mut self, gain: f32) {
        let g = gain.clamp(0.0, 1.0);
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
    }

    #[inline]
    fn shape(&self, u: f32) -> f32 {
        let a = tanh_approx(u + self.bias) - self.bias_out;
        tanh_approx(a * self.k2)
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let mut s = self.pickup.process(x) * self.pickup_gain * self.pre;
        s = self.pre_hp.process(s);
        s = self.pre_mid.process(s);
        // 2x oversampled clipping: zero-stuff, filter, shape, filter, drop.
        let mut out = 0.0;
        for phase in 0..2 {
            let mut u = if phase == 0 { 2.0 * s } else { 0.0 };
            u = self.up[0].process(u);
            u = self.up[1].process(u);
            let y = self.shape(u);
            let z = self.down[0].process(y);
            let z = self.down[1].process(z);
            if phase == 1 {
                out = z;
            }
        }
        let mut s = self.dc.process(out);
        s = self.low.process(s);
        s = self.mid.process(s);
        s = self.high.process(s);
        s = tanh_approx(s * self.power) / self.power;
        s = self.cab_hp.process(s);
        s = self.cab_low.process(s);
        s = self.cab_mid.process(s);
        s = self.cab_lp[0].process(s);
        s = self.cab_lp[1].process(s);
        tanh_approx(s * self.makeup)
    }
}

pub(crate) struct Guitar {
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
    // Unsmoothed settings.
    pick_pos: f32,
    brightness: f32,
    vib_rate: f32,
    vel_sens: f32,
    pickup_type: usize,
    pickup_pos: f32,
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
    out_gain: f32,
}

fn max_delay(sr: f32) -> usize {
    (sr / MIN_HZ) as usize + 8
}

impl Guitar {
    pub(crate) fn new() -> Guitar {
        let md = max_delay(48_000.0);
        Guitar {
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
            pick_pos: 0.16,
            brightness: 0.55,
            vib_rate: 5.5,
            vel_sens: 0.7,
            pickup_type: 0,
            pickup_pos: 0.7,
            decay: Ramp::new(4.0f32.ln()),
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
            out_gain: 1.0,
        }
    }

    fn string_params(&self) -> StringParams {
        StringParams {
            decay: self.decay.get().exp(),
            damping: self.damping.get(),
            palm: self.palm.get(),
            vib_depth: self.vib_depth.get(),
            pickup: lerp(0.27, 0.08, self.pickup_pos),
        }
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

        self.lfo_phase += self.vib_rate * CTRL as f32 / self.sr;
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
        if now[1] != self.designed[1] || now[2] != self.designed[2] || now[3] != self.designed[3] {
            self.amp.set_tone(now[1], now[2], now[3]);
        }
        if now[4] != self.designed[4] {
            self.amp.set_cab(now[4].exp());
        }
        self.designed = now;

        let p = self.string_params();
        let sr = self.sr;
        let lfo = self.lfo;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.age += CTRL;
            v.tune(sr, &p, lfo);
        }
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
        self.amp.prepare(sample_rate);
        self.designed = [f32::NAN; 5];
        self.rel_k = (-11.512_925 / (RELEASE_S * sample_rate)).exp();
        self.tail = 0;
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.amp.reset();
        self.noise.reseed(0x6717);
        self.tail = 0;
        self.lfo_phase = 0.0;
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            DECAY => self.decay.set(v.ln(), imm),
            DAMPING => self.damping.set(v, imm),
            PICK_POS => self.pick_pos = v,
            BRIGHT => self.brightness = v,
            PALM => self.palm.set(v, imm),
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
            CAB_TONE => self.cab.set(v.ln(), imm),
            LEVEL => self.level.set(v, imm),
            VELOCITY => self.vel_sens = v,
            _ => {}
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.pool.begin(key, id);
        let p = self.string_params();
        let bright = (self.brightness + 0.6 * (velocity - 0.6) * self.vel_sens).clamp(0.0, 1.0);
        let sr = self.sr;
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.released = false;
        v.f0 = key_to_hz(key as f32).max(MIN_HZ);
        v.level = vel_gain(velocity, self.vel_sens);
        v.lev = 1.0;
        v.rel = 1.0;
        v.age = 0;
        v.fade.reset();
        v.pluck(sr, &p, self.pick_pos, bright, &mut self.noise);
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        self.pool.release_id(id, |v| v.released = true);
    }

    fn all_notes_off(&mut self) {
        self.pool.release_all(|v| v.released = true);
    }

    fn all_sound_off(&mut self) {
        self.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        let tail_len = (AMP_TAIL_S * self.sr) as usize;
        for n in 0..left.len().min(right.len()) {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let mut mix = 0.0;
            let mut any = false;
            let rel_k = self.rel_k;
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                any = true;
                let r = v.line.read_hermite(v.d);
                v.lp = (1.0 - v.c) * r + v.c * v.lp;
                let y = flush(v.lp * v.g);
                v.line.push(y);
                let pick = y - v.line.read_hermite(v.dpick + 1.0);
                v.lev = y.abs().max(v.lev * 0.9995);
                if v.released {
                    v.rel *= rel_k;
                }
                mix += pick * v.level * v.rel * v.fade.tick();
                if v.fade.is_done() || v.rel < 1.0e-5 || v.lev < DEAD {
                    v.active = false;
                }
            }
            if any {
                self.tail = tail_len;
            }
            let out = if self.tail > 0 {
                if !any {
                    self.tail -= 1;
                    if self.tail == 0 {
                        self.amp.reset();
                    }
                }
                self.amp.process(mix * 0.5) * self.out_gain
            } else {
                0.0
            };
            left[n] = out;
            right[n] = out;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent() && self.tail == 0
    }
}
