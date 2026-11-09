//! `peel.drum.hat`: hats, shakers and cymbals.
//!
//! The core is the classic metallic bank: six band-limited square oscillators
//! at inharmonic ratios through band-pass and high-pass filters, mixed with
//! high-passed noise. Short decays give closed hats, long decays give open
//! hats. On top of that:
//!
//! * a three-band noise "wash" with a separate decay per band (the highs die
//!   first, as on a real cymbal), slow random amplitude shimmer, and a strike
//!   transient, for crashes, splashes and rides;
//! * a few inharmonic sine partials for the stick "ping" of a ride;
//! * slow random detune of the oscillator bank so long tails never sit on a
//!   static tone;
//! * decorrelated left/right noise for width, with the metal kept central.
//!
//! Retriggering chokes the previous hit; optionally a note-off does too.
//! Harder hits are brighter and have a stronger stick transient.

use peel_dsp::envelope::Decay;
use peel_dsp::filter::{OnePole, Svf};
use peel_dsp::noise::Noise;
use peel_dsp::osc::{Osc, Waveform, fast_sin};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::db_to_gain;
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::acoustic::{HitRng, Mode, Pulse, analog_slot};
use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, Synth, TIMBRE_OCTAVES, Vx,
    key_offset, lerp, ramp_coef, st_ratio, vel_gain,
};

const TUNE: ParamId = 1;
const METAL: ParamId = 2;
const COLOR: ParamId = 3;
const BRIGHT: ParamId = 4;
const DECAY: ParamId = 5;
const CHOKE: ParamId = 6;
const GATE: ParamId = 7;
const VELOCITY: ParamId = 8;
const LEVEL: ParamId = 9;
const WASH: ParamId = 10;
const SHIMMER: ParamId = 11;
const PING: ParamId = 12;
const BLOOM: ParamId = 13;
const WIDTH: ParamId = 14;
const ACOUSTIC: ParamId = 15;
const CIRCUIT: ParamId = 16;

/// Square-wave frequencies (Hz) of the metallic bank.
const BANK: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
/// Wash band centres relative to the colour control, and decay multipliers.
const WASH_CENTRE: [f32; 3] = [0.42, 0.78, 1.3];
const WASH_DECAY: [f32; 3] = [1.0, 0.72, 0.48];
const WASH_BLOOM: [f32; 3] = [1.0, 0.7, 0.4];
/// Ride "ping" partials (Hz at no transposition) and their levels.
const PING_HZ: [f32; 4] = [2_290.0, 3_425.0, 4_950.0, 6_210.0];
const PING_AMP: [f32; 4] = [1.0, 0.8, 0.55, 0.4];
/// Cut-offs (Hz) of the slow random modulators; the last one detunes the bank.
const MOD_HZ: [f32; 4] = [7.0, 11.0, 16.0, 5.0];

static PARAMS: [ParamDescriptor; 16] = [
    ParamDescriptor::linear(
        TUNE,
        "tone.tune",
        "Tune",
        "Tone",
        -12.0,
        12.0,
        0.0,
        Unit::Semitones,
    ),
    ParamDescriptor::linear(
        METAL,
        "tone.metal",
        "Metal",
        "Tone",
        0.0,
        1.0,
        0.65,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        COLOR,
        "tone.color",
        "Color",
        "Tone",
        3_000.0,
        15_000.0,
        9_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::log(
        BRIGHT,
        "tone.bright",
        "Brightness",
        "Tone",
        2_000.0,
        12_000.0,
        6_500.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        WASH,
        "tone.wash",
        "Wash",
        "Tone",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        SHIMMER,
        "tone.shimmer",
        "Shimmer",
        "Tone",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        PING,
        "tone.ping",
        "Ping",
        "Tone",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        DECAY,
        "amp.decay",
        "Decay",
        "Amp",
        0.015,
        4.0,
        0.08,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        BLOOM,
        "amp.bloom",
        "Bloom",
        "Amp",
        0.000_2,
        0.05,
        0.000_2,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        CHOKE,
        "amp.choke",
        "Choke",
        "Amp",
        0.001,
        0.03,
        0.006,
        Unit::Seconds,
    ),
    ParamDescriptor::toggle(GATE, "amp.gate", "Cut On Release", "Amp", false),
    ParamDescriptor::linear(
        WIDTH,
        "out.width",
        "Width",
        "Output",
        0.0,
        1.0,
        0.15,
        Unit::Percent,
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
    ParamDescriptor::toggle(ACOUSTIC, "tone.acoustic", "Acoustic", "Tone", false),
    ParamDescriptor::toggle(CIRCUIT, "tone.circuit", "Circuit", "Tone", false),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.drum.hat",
    name: "Hat",
    description: "Hi-hat, shaker and cymbal, closed to crash: electronic, circuit or acoustic",
    category: "Drums",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 18] = [
    Preset {
        name: "Closed",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("tone.tune", 0.0),
            ("tone.metal", 0.65),
            ("tone.color", 9_000.0),
            ("tone.bright", 6_500.0),
            ("tone.wash", 0.0),
            ("tone.shimmer", 0.0),
            ("tone.ping", 0.0),
            ("amp.decay", 0.08),
            ("amp.bloom", 0.000_2),
            ("amp.choke", 0.006),
            ("amp.gate", 0.0),
            ("out.width", 0.15),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Open",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("amp.decay", 0.55),
            ("tone.metal", 0.75),
            ("tone.bright", 6_000.0),
            ("tone.wash", 0.25),
            ("tone.shimmer", 0.25),
        ],
    },
    Preset {
        name: "Pedal",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("amp.decay", 0.04),
            ("tone.color", 7_500.0),
            ("tone.bright", 5_500.0),
            ("out.velocity", 0.5),
        ],
    },
    Preset {
        name: "Tight",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("amp.decay", 0.045),
            ("tone.metal", 0.5),
            ("tone.color", 11_000.0),
            ("tone.bright", 8_000.0),
        ],
    },
    Preset {
        name: "Sizzle",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("amp.decay", 0.28),
            ("tone.metal", 0.3),
            ("tone.color", 12_000.0),
            ("tone.bright", 7_000.0),
            ("tone.wash", 0.35),
            ("tone.shimmer", 0.4),
        ],
    },
    Preset {
        name: "Dark",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("amp.decay", 0.12),
            ("tone.tune", -4.0),
            ("tone.metal", 0.8),
            ("tone.color", 5_500.0),
            ("tone.bright", 3_500.0),
        ],
    },
    Preset {
        name: "Shaker",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("tone.metal", 0.0),
            ("tone.color", 7_500.0),
            ("tone.bright", 4_500.0),
            ("amp.decay", 0.09),
            ("amp.bloom", 0.006),
            ("out.width", 0.4),
            ("out.velocity", 0.6),
        ],
    },
    Preset {
        name: "Crash",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("tone.metal", 0.3),
            ("tone.color", 8_500.0),
            ("tone.bright", 4_200.0),
            ("tone.wash", 1.0),
            ("tone.shimmer", 0.7),
            ("amp.decay", 2.6),
            ("amp.bloom", 0.007),
            ("out.width", 0.45),
            ("out.velocity", 0.5),
        ],
    },
    Preset {
        name: "Ride",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("tone.metal", 0.2),
            ("tone.color", 7_000.0),
            ("tone.bright", 3_500.0),
            ("tone.wash", 0.5),
            ("tone.shimmer", 0.35),
            ("tone.ping", 0.8),
            ("amp.decay", 1.8),
            ("amp.bloom", 0.002),
            ("out.width", 0.4),
            ("out.velocity", 0.6),
        ],
    },
    Preset {
        name: "Splash",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("tone.metal", 0.35),
            ("tone.color", 11_000.0),
            ("tone.bright", 5_500.0),
            ("tone.wash", 0.9),
            ("tone.shimmer", 0.5),
            ("amp.decay", 0.7),
            ("amp.bloom", 0.003),
            ("out.width", 0.4),
        ],
    },
    Preset {
        name: "Trap Hat",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 0.0),
            ("tone.metal", 0.55),
            ("tone.color", 12_000.0),
            ("tone.bright", 8_500.0),
            ("amp.decay", 0.035),
            ("amp.choke", 0.003),
            ("out.width", 0.1),
        ],
    },
    Preset {
        name: "Acoustic Closed",
        params: &[
            ("tone.acoustic", 1.0),
            ("tone.circuit", 0.0),
            ("tone.tune", 0.0),
            ("tone.metal", 0.5),
            ("tone.color", 9_500.0),
            ("tone.bright", 7_000.0),
            ("tone.wash", 0.15),
            ("tone.shimmer", 0.35),
            ("tone.ping", 0.0),
            ("amp.decay", 0.1),
            ("amp.bloom", 0.000_2),
            ("amp.choke", 0.006),
            ("amp.gate", 0.0),
            ("out.width", 0.2),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Acoustic Open",
        params: &[
            ("tone.acoustic", 1.0),
            ("tone.circuit", 0.0),
            ("tone.tune", 0.0),
            ("tone.metal", 0.55),
            ("tone.color", 8_500.0),
            ("tone.bright", 6_500.0),
            ("tone.wash", 0.45),
            ("tone.shimmer", 0.6),
            ("tone.ping", 0.0),
            ("amp.decay", 0.9),
            ("amp.bloom", 0.001),
            ("amp.choke", 0.012),
            ("amp.gate", 0.0),
            ("out.width", 0.3),
            ("out.velocity", 0.65),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Acoustic Ride",
        params: &[
            ("tone.acoustic", 1.0),
            ("tone.circuit", 0.0),
            ("tone.tune", -2.0),
            ("tone.metal", 0.75),
            ("tone.color", 7_000.0),
            ("tone.bright", 5_000.0),
            ("tone.wash", 0.35),
            ("tone.shimmer", 0.25),
            ("tone.ping", 0.8),
            ("amp.decay", 2.2),
            ("amp.bloom", 0.000_5),
            ("amp.choke", 0.02),
            ("amp.gate", 0.0),
            ("out.width", 0.35),
            ("out.velocity", 0.6),
            ("out.level", -2.5),
        ],
    },
    Preset {
        name: "Acoustic Crash",
        params: &[
            ("tone.acoustic", 1.0),
            ("tone.circuit", 0.0),
            ("tone.tune", 0.0),
            ("tone.metal", 0.5),
            ("tone.color", 8_000.0),
            ("tone.bright", 4_500.0),
            ("tone.wash", 0.9),
            ("tone.shimmer", 0.5),
            ("tone.ping", 0.0),
            ("amp.decay", 2.8),
            ("amp.bloom", 0.012),
            ("amp.choke", 0.03),
            ("amp.gate", 0.0),
            ("out.width", 0.5),
            ("out.velocity", 0.55),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "808 Closed",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 1.0),
            ("tone.tune", 0.0),
            ("tone.metal", 0.85),
            ("tone.color", 9_000.0),
            ("tone.bright", 7_000.0),
            ("tone.wash", 0.0),
            ("tone.shimmer", 0.0),
            ("tone.ping", 0.0),
            ("amp.decay", 0.07),
            ("amp.bloom", 0.000_2),
            ("amp.choke", 0.005),
            ("amp.gate", 0.0),
            ("out.width", 0.1),
            ("out.velocity", 0.8),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "808 Open",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 1.0),
            ("tone.tune", 0.0),
            ("tone.metal", 0.85),
            ("tone.color", 9_000.0),
            ("tone.bright", 6_500.0),
            ("tone.wash", 0.0),
            ("tone.shimmer", 0.0),
            ("tone.ping", 0.0),
            ("amp.decay", 0.6),
            ("amp.bloom", 0.000_2),
            ("amp.choke", 0.006),
            ("amp.gate", 0.0),
            ("out.width", 0.15),
            ("out.velocity", 0.8),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "909 Closed",
        params: &[
            ("tone.acoustic", 0.0),
            ("tone.circuit", 1.0),
            ("tone.tune", 2.0),
            ("tone.metal", 0.45),
            ("tone.color", 11_000.0),
            ("tone.bright", 8_500.0),
            ("tone.wash", 0.0),
            ("tone.shimmer", 0.0),
            ("tone.ping", 0.0),
            ("amp.decay", 0.06),
            ("amp.bloom", 0.000_2),
            ("amp.choke", 0.004),
            ("amp.gate", 0.0),
            ("out.width", 0.2),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Hat::new()))
}

/// Plate modes in the acoustic model.
const AH_MODES: usize = 32;

/// Plate mode frequencies (Hz at no transposition) and their stereo
/// placement: an irregular, roughly geometric spread from 300 Hz to 15 kHz,
/// fixed so a cymbal keeps its identity from hit to hit.
fn plate_table() -> ([f32; AH_MODES], [f32; AH_MODES]) {
    let mut hz = [0.0; AH_MODES];
    let mut pan = [0.0; AH_MODES];
    let mut rng = HitRng::new(0x51a7, 0x0c1b);
    for k in 0..AH_MODES {
        hz[k] = 300.0 * 1.135f32.powi(k as i32) * (1.0 + 0.045 * rng.next());
        pan[k] = rng.next();
    }
    (hz, pan)
}

/// The acoustic hat and cymbal: a stick pulse exciting inharmonic plate
/// modes whose decay shortens with frequency, a dense shaped-noise layer
/// for the part of the spectrum too crowded to model mode by mode, and a
/// sizzle that follows the plates' own motion (two hats or rivets touching).
#[derive(Clone, Copy)]
struct AcHatVoice {
    active: bool,
    modes: [Mode; AH_MODES],
    weight: [f32; AH_MODES],
    side: [f32; AH_MODES],
    pulse: Pulse,
    modal_amp: f32,
    common: Noise,
    noise_l: Noise,
    noise_r: Noise,
    hiss_l: Svf,
    hiss_r: Svf,
    wash_l: [Svf; 3],
    wash_r: [Svf; 3],
    fast: Decay,
    slow: Decay,
    strike: Decay,
    band_env: [Decay; 3],
    sizzle: f32,
    tick_hp: OnePole,
    tick: Decay,
    tick_gain: f32,
    air: [OnePole; 4],
    noise_gain: f32,
    wash_gain: f32,
    mix_common: f32,
    mix_side: f32,
    gain: f32,
    fade: Fade,
    xgain: f32,
    tone: f32,
    bright_hz: f32,
    choke_ms: f32,
}

impl AcHatVoice {
    fn new() -> AcHatVoice {
        AcHatVoice {
            active: false,
            modes: [Mode::new(); AH_MODES],
            weight: [0.0; AH_MODES],
            side: [0.0; AH_MODES],
            pulse: Pulse::new(),
            modal_amp: 0.0,
            common: Noise::new(41),
            noise_l: Noise::new(42),
            noise_r: Noise::new(43),
            hiss_l: Svf::new(),
            hiss_r: Svf::new(),
            wash_l: [Svf::new(); 3],
            wash_r: [Svf::new(); 3],
            fast: Decay::new(),
            slow: Decay::new(),
            strike: Decay::new(),
            band_env: [Decay::new(); 3],
            sizzle: 0.0,
            tick_hp: OnePole::new(),
            tick: Decay::new(),
            tick_gain: 0.0,
            air: [OnePole::new(); 4],
            noise_gain: 1.0,
            wash_gain: 0.0,
            mix_common: 0.84,
            mix_side: 0.55,
            gain: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            tone: 0.0,
            bright_hz: 6_500.0,
            choke_ms: 6.0,
        }
    }

    fn set_tone(&mut self, x: &Vx, sr: f32) {
        if x.tone != self.tone {
            self.tone = x.tone;
            let hz = (self.bright_hz * x.tone_ratio(TIMBRE_OCTAVES)).min(0.42 * sr);
            self.hiss_l.set_q(hz, 0.7, sr);
            self.hiss_r.set_q(hz, 0.7, sr);
        }
    }

    /// Control-rate upkeep: flushes tiny states and tracks the modal level.
    fn upkeep(&mut self) {
        let mut a = 0.0f32;
        for m in &mut self.modes {
            m.flush();
            a = a.max(m.amp());
        }
        self.modal_amp = a;
    }

    #[inline]
    #[allow(clippy::needless_range_loop)]
    fn tick(&mut self) -> (f32, f32) {
        let x = self.pulse.tick();
        let (mut ml, mut mr) = (0.0, 0.0);
        for k in 0..AH_MODES {
            let y = self.modes[k].tick(x * self.weight[k]);
            ml += y * (1.0 + self.side[k]);
            mr += y * (1.0 - self.side[k]);
        }
        // Sizzle: the noise is chopped by the motion of two low plate modes,
        // like hats or rivets chattering against the cymbal.
        let chatter = self.modes[4].unit() * self.modes[9].unit();
        let am = 1.0 + self.sizzle * chatter;

        let c = self.common.white() * self.mix_common;
        let nl = c + self.noise_l.white() * self.mix_side;
        let nr = c + self.noise_r.white() * self.mix_side;
        let env = (0.6 * self.fast.tick() + 0.4 * self.slow.tick()) * am * self.noise_gain;
        let mut l = ml + self.hiss_l.tick(nl).high * env;
        let mut r = mr + self.hiss_r.tick(nr).high * env;
        let strike = 1.0 + 1.2 * self.strike.tick();
        if self.wash_gain > 0.0 {
            for k in 0..3 {
                let a = self.band_env[k].tick() * strike * am * self.wash_gain;
                l += self.wash_l[k].tick(nl).band_unity * a;
                r += self.wash_r[k].tick(nr).band_unity * a;
            }
        }
        let t = self.tick_hp.high(c) * self.tick.tick() * self.tick_gain;
        l += t;
        r += t;
        let l = {
            let t = self.air[0].low(l);
            self.air[1].low(t)
        };
        let r = {
            let t = self.air[2].low(r);
            self.air[3].low(t)
        };
        let g = self.gain * self.fade.tick() * self.xgain;
        let done = !self.pulse.busy()
            && self.modal_amp < 1.0e-4
            && self.slow.is_below(1.0e-4)
            && (self.wash_gain == 0.0 || self.band_env[0].is_below(1.0e-4));
        if self.fade.is_done() || done {
            self.active = false;
        }
        (l * g, r * g)
    }
}

/// The circuit hat: the six-square metallic bank of the classic analogue
/// machines, with oscillators that run freely between hits (each hit
/// catches them at a different phase, as on the hardware), two band-passes
/// and a high-pass, noise mixed by Metal, and a VCA envelope that restarts
/// from its current level when retriggered instead of cutting to silence.
#[derive(Clone, Copy)]
struct CkHatVoice {
    active: bool,
    osc: [Osc; 6],
    dt: [f32; 6],
    bp_lo: Svf,
    bp_hi: Svf,
    hp: Svf,
    common: Noise,
    noise_l: Noise,
    noise_r: Noise,
    hiss_l: Svf,
    hiss_r: Svf,
    fast: Decay,
    slow: Decay,
    tick_pulse: Pulse,
    tick_hp: OnePole,
    tick_gain: f32,
    air: [OnePole; 4],
    metal_gain: f32,
    noise_gain: f32,
    mix_common: f32,
    mix_side: f32,
    gain: f32,
    fade: Fade,
    xgain: f32,
    tone: f32,
    bright_hz: f32,
    choke_ms: f32,
}

impl CkHatVoice {
    fn new() -> CkHatVoice {
        CkHatVoice {
            active: false,
            osc: [Osc::new(); 6],
            dt: [0.0; 6],
            bp_lo: Svf::new(),
            bp_hi: Svf::new(),
            hp: Svf::new(),
            common: Noise::new(81),
            noise_l: Noise::new(82),
            noise_r: Noise::new(83),
            hiss_l: Svf::new(),
            hiss_r: Svf::new(),
            fast: Decay::new(),
            slow: Decay::new(),
            tick_pulse: Pulse::new(),
            tick_hp: OnePole::new(),
            tick_gain: 0.0,
            air: [OnePole::new(); 4],
            metal_gain: 1.0,
            noise_gain: 0.0,
            mix_common: 0.9,
            mix_side: 0.3,
            gain: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            tone: 0.0,
            bright_hz: 7_000.0,
            choke_ms: 6.0,
        }
    }

    fn set_tone(&mut self, x: &Vx, sr: f32) {
        if x.tone != self.tone {
            self.tone = x.tone;
            let hz = (self.bright_hz * x.tone_ratio(TIMBRE_OCTAVES)).min(0.42 * sr);
            self.hp.set_q(hz, 0.75, sr);
            self.hiss_l.set_q(hz * 0.8, 0.7, sr);
            self.hiss_r.set_q(hz * 0.8, 0.7, sr);
        }
    }

    #[inline]
    fn tick(&mut self) -> (f32, f32) {
        let mut bank = 0.0;
        for (o, &dt) in self.osc.iter_mut().zip(self.dt.iter()) {
            bank += o.tick(Waveform::Square, dt, 0.5);
        }
        let metal = 0.35 * self.bp_lo.tick(bank).band_unity + self.bp_hi.tick(bank).band_unity;
        let metal = self.hp.tick(metal).high * self.metal_gain;
        let env = 0.6 * self.fast.tick() + 0.4 * self.slow.tick();
        let c = self.common.white() * self.mix_common;
        let nl = c + self.noise_l.white() * self.mix_side;
        let nr = c + self.noise_r.white() * self.mix_side;
        let t = self.tick_hp.high(self.tick_pulse.tick()) * self.tick_gain;
        let mono = metal * env + t;
        let l = mono + self.hiss_l.tick(nl).high * self.noise_gain * env;
        let r = mono + self.hiss_r.tick(nr).high * self.noise_gain * env;
        let l = {
            let t = self.air[0].low(l);
            self.air[1].low(t)
        };
        let r = {
            let t = self.air[2].low(r);
            self.air[3].low(t)
        };
        let g = self.gain * self.fade.tick() * self.xgain;
        if self.fade.is_done() || (self.slow.is_below(1.0e-4) && !self.tick_pulse.busy()) {
            self.active = false;
        }
        (l * g, r * g)
    }
}

impl PoolVoice for CkHatVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.slow.level()
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, self.choke_ms);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

impl PoolVoice for AcHatVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.slow.level().max(self.modal_amp)
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, self.choke_ms);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

#[derive(Clone, Copy)]
struct HatVoice {
    active: bool,
    osc: [Osc; 6],
    dt: [f32; 6],
    band: Svf,
    high: Svf,
    // Noise: one common source and one per channel, mixed by the width.
    common: Noise,
    noise_l: Noise,
    noise_r: Noise,
    hiss_l: Svf,
    hiss_r: Svf,
    wash_l: [Svf; 3],
    wash_r: [Svf; 3],
    fast: Decay,
    slow: Decay,
    strike: Decay,
    band_env: [Decay; 3],
    // Slow random modulation.
    mod_noise: Noise,
    mod_lp: [OnePole; 4],
    mod_gain: [f32; 4],
    shimmer: f32,
    // Ride ping.
    ping_phase: [f32; 4],
    ping_dt: [f32; 4],
    ping_env: Decay,
    ping_gain: f32,
    // Stick transient.
    air: [OnePole; 4],
    tick_hp: OnePole,
    tick_env: f32,
    tick_coef: f32,
    tick_gain: f32,
    metal_gain: f32,
    noise_gain: f32,
    wash_gain: f32,
    mix_common: f32,
    mix_side: f32,
    gain: f32,
    fade: Fade,
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
    /// Timbre the top filters are set for (bipolar, 0 neutral) and their base.
    tone: f32,
    bright_hz: f32,
    choke_ms: f32,
}

impl HatVoice {
    /// Timbre moves the bright band and hiss by up to two octaves.
    fn set_tone(&mut self, x: &Vx, sr: f32) {
        if x.tone != self.tone {
            self.tone = x.tone;
            let hz = (self.bright_hz * x.tone_ratio(TIMBRE_OCTAVES)).min(0.42 * sr);
            self.high.set_q(hz, 0.8, sr);
            self.hiss_l.set_q(hz * 0.8, 0.7, sr);
            self.hiss_r.set_q(hz * 0.8, 0.7, sr);
        }
    }

    fn new() -> HatVoice {
        HatVoice {
            active: false,
            osc: [Osc::new(); 6],
            dt: [0.0; 6],
            band: Svf::new(),
            high: Svf::new(),
            common: Noise::new(1),
            noise_l: Noise::new(2),
            noise_r: Noise::new(3),
            hiss_l: Svf::new(),
            hiss_r: Svf::new(),
            wash_l: [Svf::new(); 3],
            wash_r: [Svf::new(); 3],
            fast: Decay::new(),
            slow: Decay::new(),
            strike: Decay::new(),
            band_env: [Decay::new(); 3],
            mod_noise: Noise::new(4),
            mod_lp: [OnePole::new(); 4],
            mod_gain: [1.0; 4],
            shimmer: 0.0,
            ping_phase: [0.0; 4],
            ping_dt: [0.0; 4],
            ping_env: Decay::new(),
            ping_gain: 0.0,
            air: [OnePole::new(); 4],
            tick_hp: OnePole::new(),
            tick_env: 0.0,
            tick_coef: 0.0,
            tick_gain: 0.0,
            metal_gain: 1.0,
            noise_gain: 1.0,
            wash_gain: 0.0,
            mix_common: 0.84,
            mix_side: 0.55,
            gain: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            tone: 0.0,
            bright_hz: 6_500.0,
            choke_ms: 6.0,
        }
    }

    #[inline]
    #[allow(clippy::needless_range_loop)]
    fn tick(&mut self) -> (f32, f32) {
        // Slow unit-RMS random modulators.
        let mut m = [0.0f32; 4];
        for (k, slot) in m.iter_mut().enumerate() {
            *slot = self.mod_lp[k].low(self.mod_noise.white()) * self.mod_gain[k];
        }

        // Metal: the bank drifts a few cents so a long tail is never a
        // static comb of tones.
        let depth = 0.003 + 0.004 * self.shimmer;
        let mut bank = 0.0;
        for (i, (o, &dt)) in self.osc.iter_mut().zip(self.dt.iter()).enumerate() {
            // Each oscillator drifts on its own modulator, so the comb of
            // partials slowly shifts instead of sitting still.
            let det = 1.0 + depth * m[(i + 1) % 4];
            bank += o.tick(Waveform::Square, dt * det, 0.5);
        }
        let metal = self.high.tick(self.band.tick(bank).band_unity).high;
        let env = 0.55 * self.fast.tick() + 0.45 * self.slow.tick();

        // Noise: common part plus decorrelated sides.
        let c = self.common.white() * self.mix_common;
        let nl = c + self.noise_l.white() * self.mix_side;
        let nr = c + self.noise_r.white() * self.mix_side;
        let hiss_l = self.hiss_l.tick(nl).high;
        let hiss_r = self.hiss_r.tick(nr).high;

        let mono = metal * self.metal_gain * env;
        let mut l = mono + hiss_l * self.noise_gain * env;
        let mut r = mono + hiss_r * self.noise_gain * env;

        // Wash: per-band decays and shimmer.
        let strike = 1.0 + 1.2 * self.strike.tick();
        if self.wash_gain > 0.0 {
            for k in 0..3 {
                let e = self.band_env[k].tick() * strike;
                let depth = (1.0 + 0.6 * self.shimmer * m[k]).clamp(0.15, 2.5);
                let a = e * depth * self.wash_gain;
                l += self.wash_l[k].tick(nl).band_unity * a;
                r += self.wash_r[k].tick(nr).band_unity * a;
            }
        }

        // Ping: inharmonic sine partials.
        if self.ping_gain > 0.0 {
            let pe = self.ping_env.tick() * self.ping_gain;
            let mut p = 0.0;
            for k in 0..4 {
                self.ping_phase[k] += self.ping_dt[k];
                if self.ping_phase[k] >= 1.0 {
                    self.ping_phase[k] -= 1.0;
                }
                p += fast_sin(self.ping_phase[k]) * PING_AMP[k];
            }
            l += p * pe;
            r += p * pe;
        }

        // Stick transient.
        if self.tick_env > 1.0e-3 {
            let t = self.tick_hp.high(c) * self.tick_env * self.tick_gain;
            self.tick_env *= self.tick_coef;
            l += t;
            r += t;
        }

        // Gentle roll-off above ~15 kHz (two poles per channel), as on a
        // recorded cymbal, so nothing reaches the Nyquist region.
        let l = {
            let t = self.air[0].low(l);
            self.air[1].low(t)
        };
        let r = {
            let t = self.air[2].low(r);
            self.air[3].low(t)
        };
        let g = self.gain * self.fade.tick() * self.xgain;
        let done = self.slow.is_below(1.0e-4)
            && (self.wash_gain == 0.0 || self.band_env[0].is_below(1.0e-4))
            && (self.ping_gain == 0.0 || self.ping_env.is_below(1.0e-4));
        if self.fade.is_done() || done {
            self.active = false;
        }
        (l * g, r * g)
    }
}

impl PoolVoice for HatVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.slow.level()
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, self.choke_ms);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

pub(crate) struct Hat {
    expr: Expression,
    sr: f32,
    pool: Pool<HatVoice>,
    ac: Pool<AcHatVoice>,
    ck: Pool<CkHatVoice>,
    acoustic: bool,
    circuit: bool,
    /// Samples rendered since `prepare`: the free-running circuit
    /// oscillators take their phase from it.
    clock: u64,
    plate_hz: [f32; AH_MODES],
    plate_pan: [f32; AH_MODES],
    ramp_k: f32,
    ctrl: usize,
    hits: u32,
    tune: f32,
    metal: f32,
    color: f32,
    bright: f32,
    wash: f32,
    shimmer: f32,
    ping: f32,
    decay: f32,
    bloom: f32,
    width: f32,
    choke_ms: f32,
    gate: bool,
    vel_sens: f32,
    level: Ramp,
    level_gain: f32,
}

impl Hat {
    pub(crate) fn new() -> Hat {
        Hat {
            expr: Expression::new(),
            sr: 48_000.0,
            pool: Pool::new(4, 1, StealPolicy::Oldest, 48_000.0, HatVoice::new),
            ac: Pool::new(4, 1, StealPolicy::Oldest, 48_000.0, AcHatVoice::new),
            ck: Pool::new(4, 1, StealPolicy::Oldest, 48_000.0, CkHatVoice::new),
            acoustic: false,
            circuit: false,
            clock: 0,
            plate_hz: plate_table().0,
            plate_pan: plate_table().1,
            ramp_k: 0.1,
            ctrl: 0,
            hits: 0,
            tune: 0.0,
            metal: 0.65,
            color: 9_000.0,
            bright: 6_500.0,
            wash: 0.0,
            shimmer: 0.0,
            ping: 0.0,
            decay: 0.08,
            bloom: 0.000_2,
            width: 0.15,
            choke_ms: 6.0,
            gate: false,
            vel_sens: 0.7,
            level: Ramp::new(0.0),
            level_gain: 1.0,
        }
    }

    fn derive(&mut self) {
        self.level_gain = db_to_gain(self.level.get()) * 0.68;
    }

    fn control(&mut self) {
        self.level.tick(self.ramp_k);
        self.derive();
        self.expr.tick();
        let ex = &self.expr;
        let sr = self.sr;
        self.pool.for_active_id(|v, key, id| {
            let x = ex.voice(id, key);
            v.xgain = x.gain();
            v.set_tone(&x, sr);
        });
        self.ac.for_active_id(|v, key, id| {
            let x = ex.voice(id, key);
            v.xgain = x.gain();
            v.set_tone(&x, sr);
            v.upkeep();
        });
        self.ck.for_active_id(|v, key, id| {
            let x = ex.voice(id, key);
            v.xgain = x.gain();
            v.set_tone(&x, sr);
        });
    }

    /// Starts a circuit hit. A hit still sounding carries on with its
    /// oscillators and filters; only the envelope is retriggered.
    fn ck_note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let sr = self.sr;
        self.pool.release_all(|v| v.fade_out(sr));
        self.ac.release_all(|v| v.fade_out(sr));
        let (slot, warm) = analog_slot(&mut self.ck, key, id, true, |v| v.fade.is_fading());
        self.hits = self.hits.wrapping_add(1);
        let hits = self.hits;
        let ratio = st_ratio(self.tune + key_offset(key));
        // Accent opens the filters a little as well as raising the level.
        let vb = lerp(0.75, 1.1, velocity);
        let bright = (self.bright * ratio.sqrt() * vb).min(0.42 * sr);
        let color = self.color * ratio.sqrt() * vb.sqrt();
        let clock = self.clock;
        let x = self.expr.voice(id, key);
        let v = &mut self.ck.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        for (i, &hz) in BANK.iter().enumerate() {
            v.dt[i] = (hz * ratio / sr).min(0.45);
            if !warm {
                let cycles = clock as f64 * v.dt[i] as f64;
                v.osc[i] = Osc::with_phase((cycles - cycles.floor()) as f32);
            }
        }
        v.bp_lo.set_q((color * 0.38).min(0.42 * sr), 1.6, sr);
        v.bp_hi.set_q((color * 0.79).min(0.42 * sr), 1.6, sr);
        v.bright_hz = bright;
        v.tone = f32::NAN;
        v.set_tone(&x, sr);
        if !warm {
            v.bp_lo.reset();
            v.bp_hi.reset();
            v.hp.reset();
            v.hiss_l.reset();
            v.hiss_r.reset();
            v.fast.reset();
            v.slow.reset();
            v.tick_hp.reset();
            for a in &mut v.air {
                a.reset();
            }
        }
        v.common.reseed(hits.wrapping_add(8_000));
        v.noise_l.reseed(hits.wrapping_mul(2).wrapping_add(8_100));
        v.noise_r.reseed(hits.wrapping_mul(2).wrapping_add(8_101));
        v.fast.set_t60(self.decay * 0.3, sr);
        v.fast.trigger(1.0, self.bloom.min(0.004), sr);
        v.slow.set_t60(self.decay, sr);
        v.slow.trigger(1.0, self.bloom, sr);
        v.tick_pulse.start(0.000_06, sr);
        v.tick_hp.set_cutoff(5_000.0, sr);
        v.tick_gain = 0.6 * velocity * velocity;
        for a in &mut v.air {
            a.set_cutoff(15_000.0, sr);
        }
        v.metal_gain = self.metal * 1.7;
        v.noise_gain = (1.0 - self.metal) * 1.2;
        v.mix_common = (1.0 - self.width).sqrt();
        v.mix_side = self.width.sqrt();
        v.gain = vel_gain(velocity, self.vel_sens);
        v.choke_ms = self.choke_ms;
        v.fade.reset();
    }

    /// Starts an acoustic stroke.
    #[allow(clippy::needless_range_loop)]
    fn ac_note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let sr = self.sr;
        // One cymbal: a new stroke chokes whatever the other models ring.
        self.pool.release_all(|v| v.fade_out(sr));
        self.ck.release_all(|v| v.fade_out(sr));
        let slot = self.ac.begin(key, id);
        self.hits = self.hits.wrapping_add(1);
        let hits = self.hits;
        let mut rng = HitRng::new(hits, 0x4a75);
        let ratio = st_ratio(self.tune + key_offset(key));
        let vb = lerp(0.65, 1.1, velocity);
        let bright = (self.bright * ratio.sqrt() * vb).min(0.42 * sr);
        // Where the plate modes are strongest: Color sets the centre.
        let centre = self.color * 0.6 * ratio.sqrt();
        let contact = lerp(0.000_22, 0.000_07, velocity) * (1.0 + 0.12 * rng.next());
        let modal = self.metal * 0.55;
        let x = self.expr.voice(id, key);
        let v = &mut self.ac.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        let mut sum = 0.0;
        for k in 0..AH_MODES {
            let f = (self.plate_hz[k] * ratio * (1.0 + 0.003 * rng.next())).min(0.45 * sr);
            let oct = (f / centre).log2() / 1.4;
            let ping_oct = (f / (3_600.0 * ratio)).log2() / 0.6;
            let ping = self.ping * (-0.5 * ping_oct * ping_oct).exp();
            // Stick position changes how strongly each mode is struck, and a
            // harder stroke tilts the energy towards the upper modes.
            let w = (0.03 + (-0.5 * oct * oct).exp())
                * (f / 2_500.0).powf(0.6 * (velocity - 0.6))
                * (1.0 + 2.5 * ping)
                * (1.0 + 0.45 * rng.next()).max(0.1);
            v.weight[k] = w;
            sum += w * w;
            // Higher modes lose energy faster; ping modes ring longer.
            let t60 = self.decay * (1_500.0 / f).powf(0.3).clamp(0.3, 1.8) * (1.0 + 1.2 * ping);
            v.modes[k].start(f, t60.max(0.008), sr);
            v.side[k] = self.width * 0.6 * self.plate_pan[k];
        }
        let norm = modal / sum.sqrt().max(1.0e-6);
        for w in &mut v.weight {
            *w *= norm;
        }
        v.pulse.start(contact, sr);
        v.modal_amp = 1.0;
        v.common.reseed(hits.wrapping_add(6_000));
        v.noise_l.reseed(hits.wrapping_mul(2).wrapping_add(7_000));
        v.noise_r.reseed(hits.wrapping_mul(2).wrapping_add(7_001));
        for f in [&mut v.hiss_l, &mut v.hiss_r] {
            f.set_q(bright, 0.7, sr);
            f.reset();
        }
        v.bright_hz = bright;
        v.tone = 0.0;
        v.set_tone(&x, sr);
        let atk = self.bloom;
        v.fast.set_t60(self.decay * 0.25, sr);
        v.fast.trigger(1.0, atk.min(0.004), sr);
        v.slow.set_t60(self.decay, sr);
        v.slow.trigger(1.0, atk, sr);
        v.strike.set_t60((self.decay * 0.1).clamp(0.02, 0.2), sr);
        v.strike.trigger(1.0, 0.000_2, sr);
        for k in 0..3 {
            let hz = (self.color * WASH_CENTRE[k] * ratio.sqrt() * vb.sqrt()).min(0.42 * sr);
            for f in [&mut v.wash_l[k], &mut v.wash_r[k]] {
                f.set_q(hz, 0.9, sr);
                f.reset();
            }
            v.band_env[k].set_t60((self.decay * WASH_DECAY[k]).max(0.02), sr);
            v.band_env[k].trigger(1.0, atk * WASH_BLOOM[k], sr);
        }
        v.wash_gain = self.wash * 0.15 * lerp(0.8, 1.0, velocity);
        v.sizzle = self.shimmer * 0.8;
        // The bright hiss grows with the stroke faster than the wash does.
        v.noise_gain =
            lerp(1.0, 0.45, self.metal) * (1.0 - 0.6 * self.wash) * lerp(0.5, 1.1, velocity);
        v.tick_hp.set_cutoff(lerp(4_000.0, 8_000.0, velocity), sr);
        v.tick_hp.reset();
        v.tick.set_t60(0.006, sr);
        v.tick.trigger(1.0, 0.000_1, sr);
        v.tick_gain = 0.5 * velocity * velocity * (1.0 + 0.2 * rng.next());
        for a in &mut v.air {
            a.set_cutoff(15_000.0, sr);
            a.reset();
        }
        v.mix_common = (1.0 - self.width).sqrt();
        v.mix_side = self.width.sqrt();
        v.gain = vel_gain(velocity, self.vel_sens) * (1.0 + 0.03 * rng.next());
        v.choke_ms = self.choke_ms;
        v.fade.reset();
    }
}

impl Synth for Hat {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(4, 1, StealPolicy::Oldest, sample_rate, HatVoice::new);
        self.ac = Pool::new(4, 1, StealPolicy::Oldest, sample_rate, AcHatVoice::new);
        self.ck = Pool::new(4, 1, StealPolicy::Oldest, sample_rate, CkHatVoice::new);
        self.clock = 0;
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ac.kill_all();
        self.ck.kill_all();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, immediate: bool) {
        match id {
            TUNE => self.tune = v,
            METAL => self.metal = v,
            COLOR => self.color = v,
            BRIGHT => self.bright = v,
            WASH => self.wash = v,
            SHIMMER => self.shimmer = v,
            PING => self.ping = v,
            DECAY => self.decay = v,
            BLOOM => self.bloom = v,
            WIDTH => self.width = v,
            CHOKE => {
                self.choke_ms = v * 1000.0;
                for voice in &mut self.pool.voices {
                    voice.choke_ms = self.choke_ms;
                }
                for voice in &mut self.ac.voices {
                    voice.choke_ms = self.choke_ms;
                }
                for voice in &mut self.ck.voices {
                    voice.choke_ms = self.choke_ms;
                }
            }
            GATE => self.gate = v >= 0.5,
            VELOCITY => self.vel_sens = v,
            LEVEL => self.level.set(v, immediate),
            ACOUSTIC => self.acoustic = v >= 0.5,
            CIRCUIT => self.circuit = v >= 0.5,
            _ => {}
        }
        if immediate {
            self.derive();
        }
    }

    #[allow(clippy::needless_range_loop)]
    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        if self.acoustic {
            self.ac_note_on(key, velocity, id);
            return;
        }
        if self.circuit {
            self.ck_note_on(key, velocity, id);
            return;
        }
        let fade_sr = self.sr;
        self.ac.release_all(|v| v.fade_out(fade_sr));
        self.ck.release_all(|v| v.fade_out(fade_sr));
        let slot = self.pool.begin(key, id);
        self.hits = self.hits.wrapping_add(1);
        let sr = self.sr;
        let ratio = st_ratio(self.tune + key_offset(key));
        // Harder hits are brighter.
        let vb = lerp(0.65, 1.1, velocity);
        let bright = (self.bright * ratio.sqrt() * vb).min(0.42 * sr);
        let color = (self.color * ratio.sqrt()).min(0.42 * sr);
        let hits = self.hits;
        // Inharmonic metal has no pitch to bend; pressure and timbre apply.
        let x = self.expr.voice(id, key);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        for (i, &hz) in BANK.iter().enumerate() {
            v.dt[i] = (hz * ratio / sr).min(0.45);
            // Fixed, spread phases so every hit has the same metallic attack.
            v.osc[i] = Osc::with_phase((i as f32 * 0.37 + 0.11).fract());
        }
        v.common.reseed(hits);
        v.noise_l.reseed(hits.wrapping_mul(2).wrapping_add(5_000));
        v.noise_r.reseed(hits.wrapping_mul(2).wrapping_add(5_001));
        v.mod_noise.reseed(hits.wrapping_add(9_000));
        v.band.set_q(color, 1.4, sr);
        v.band.reset();
        v.high.set_q(bright, 0.8, sr);
        v.high.reset();
        for f in [&mut v.hiss_l, &mut v.hiss_r] {
            f.set_q(bright * 0.8, 0.7, sr);
            f.reset();
        }
        v.bright_hz = bright;
        v.tone = 0.0;
        v.set_tone(&x, sr);
        let atk = self.bloom;
        v.fast.set_t60(self.decay * 0.3, sr);
        v.fast.trigger(1.0, atk.min(0.004), sr);
        v.slow.set_t60(self.decay, sr);
        v.slow.trigger(1.0, atk, sr);

        // Wash bands.
        v.strike.set_t60((self.decay * 0.1).clamp(0.02, 0.2), sr);
        v.strike.trigger(1.0, 0.000_2, sr);
        for k in 0..3 {
            let hz = (self.color * WASH_CENTRE[k] * ratio.sqrt() * vb.sqrt()).min(0.42 * sr);
            for f in [&mut v.wash_l[k], &mut v.wash_r[k]] {
                f.set_q(hz, 0.9, sr);
                f.reset();
            }
            v.band_env[k].set_t60((self.decay * WASH_DECAY[k]).max(0.02), sr);
            v.band_env[k].trigger(1.0, atk * WASH_BLOOM[k], sr);
        }
        v.wash_gain = self.wash * 0.15 * lerp(0.7, 1.0, velocity);

        // Slow random modulation, scaled to unit RMS.
        for k in 0..4 {
            let a = 1.0 - (-std::f32::consts::TAU * MOD_HZ[k] / sr).exp();
            v.mod_lp[k].set_cutoff(MOD_HZ[k], sr);
            v.mod_lp[k].reset();
            v.mod_gain[k] = 1.0 / (0.577 * (a / (2.0 - a)).sqrt());
        }
        v.shimmer = self.shimmer;

        // Ping.
        for k in 0..4 {
            let f = (PING_HZ[k] * ratio).min(0.42 * sr);
            v.ping_dt[k] = f / sr;
            v.ping_phase[k] = 0.0;
        }
        v.ping_env.set_t60((self.decay * 0.55).max(0.05), sr);
        v.ping_env.trigger(1.0, 0.000_3, sr);
        v.ping_gain = self.ping * 0.22 * lerp(0.6, 1.0, velocity);

        // Stick transient: stronger and brighter with velocity.
        for a in &mut v.air {
            a.set_cutoff(15_000.0, sr);
            a.reset();
        }
        v.tick_hp.set_cutoff(lerp(5_000.0, 8_500.0, velocity), sr);
        v.tick_hp.reset();
        v.tick_env = 1.0;
        v.tick_coef = (-1.0 / (0.001_2 * sr)).exp();
        v.tick_gain = 0.45 * velocity * velocity;

        v.metal_gain = self.metal * 1.6;
        v.noise_gain = lerp(0.9, 0.3, self.metal) * (1.0 - 0.7 * self.wash);
        // Power-preserving blend between a common and decorrelated noise.
        v.mix_common = (1.0 - self.width).sqrt();
        v.mix_side = self.width.sqrt();
        v.gain = vel_gain(velocity, self.vel_sens);
        v.choke_ms = self.choke_ms;
        v.fade.reset();
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        if self.gate {
            let sr = self.sr;
            self.pool.release_id(id, |v| v.fade_out(sr));
            self.ac.release_id(id, |v| v.fade_out(sr));
            self.ck.release_id(id, |v| v.fade_out(sr));
        }
    }

    fn all_notes_off(&mut self) {}

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
        self.ac.kill_all();
        self.ck.kill_all();
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
            for v in self.ac.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick();
                l += a;
                r += b;
            }
            for v in self.ck.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick();
                l += a;
                r += b;
            }
            left[n] = l * self.level_gain;
            right[n] = r * self.level_gain;
        }
        self.clock = self.clock.wrapping_add(left.len().min(right.len()) as u64);
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent() && self.ac.is_silent() && self.ck.is_silent()
    }
}
