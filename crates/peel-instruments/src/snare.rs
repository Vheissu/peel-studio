//! `peel.drum.snare`: three tuned (inharmonic) head modes with a small pitch
//! drop, a band-passed noise "snare wire" layer with a two-stage decay, a
//! short band-limited snap transient, an optional clap-style multi-strike
//! attack and a decorrelated stereo room tail.

use peel_dsp::envelope::Decay;
use peel_dsp::filter::{OnePole, Svf};
use peel_dsp::noise::Noise;
use peel_dsp::osc::fast_sin;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::db_to_gain;
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::acoustic::{Adaa, HitRng, Mode, Pulse, Room, analog_slot};
use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth,
    TIMBRE_OCTAVES, Vx, drive_curve, key_offset, lerp, ramp_coef, shape, st_ratio, vel_gain,
};

const PITCH: ParamId = 1;
const BODY_DECAY: ParamId = 2;
const OVERTONE: ParamId = 3;
const MIX: ParamId = 4;
const NOISE_DECAY: ParamId = 5;
const NOISE_TONE: ParamId = 6;
const SNAP: ParamId = 7;
const DRIVE: ParamId = 8;
const VELOCITY: ParamId = 9;
const LEVEL: ParamId = 10;
const BURST: ParamId = 11;
const ROOM: ParamId = 12;
const ACOUSTIC: ParamId = 13;
const CIRCUIT: ParamId = 14;

static PARAMS: [ParamDescriptor; 14] = [
    ParamDescriptor::log(
        PITCH,
        "body.pitch",
        "Pitch",
        "Body",
        120.0,
        400.0,
        190.0,
        Unit::Hertz,
    ),
    ParamDescriptor::log(
        BODY_DECAY,
        "body.decay",
        "Decay",
        "Body",
        0.04,
        0.6,
        0.16,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        OVERTONE,
        "body.overtone",
        "Overtone",
        "Body",
        1.3,
        2.2,
        1.68,
        Unit::Ratio,
    ),
    ParamDescriptor::linear(
        MIX,
        "body.mix",
        "Body Mix",
        "Body",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        NOISE_DECAY,
        "noise.decay",
        "Decay",
        "Noise",
        0.05,
        0.8,
        0.24,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        NOISE_TONE,
        "noise.tone",
        "Tone",
        "Noise",
        1_500.0,
        10_000.0,
        4_800.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        SNAP,
        "noise.snap",
        "Snap",
        "Noise",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BURST,
        "noise.burst",
        "Burst",
        "Noise",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ROOM,
        "out.room",
        "Room",
        "Output",
        0.0,
        1.0,
        0.15,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        DRIVE,
        "out.drive",
        "Drive",
        "Output",
        0.0,
        1.0,
        0.2,
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
    ParamDescriptor::toggle(ACOUSTIC, "body.acoustic", "Acoustic", "Body", false),
    ParamDescriptor::toggle(CIRCUIT, "body.circuit", "Circuit", "Body", false),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.drum.snare",
    name: "Snare",
    description: "Snare drum: electronic, drum-machine circuit or acoustic model",
    category: "Drums",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 19] = [
    Preset {
        name: "Crack",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 190.0),
            ("body.decay", 0.16),
            ("body.overtone", 1.68),
            ("body.mix", 0.5),
            ("noise.decay", 0.24),
            ("noise.tone", 4_800.0),
            ("noise.snap", 0.5),
            ("noise.burst", 0.0),
            ("out.drive", 0.2),
            ("out.room", 0.15),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Fat",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 150.0),
            ("body.decay", 0.28),
            ("body.mix", 0.65),
            ("noise.decay", 0.3),
            ("noise.tone", 3_200.0),
            ("noise.snap", 0.35),
            ("out.drive", 0.4),
            ("out.room", 0.2),
        ],
    },
    Preset {
        name: "Tight",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 230.0),
            ("body.decay", 0.09),
            ("body.mix", 0.4),
            ("noise.decay", 0.12),
            ("noise.tone", 6_000.0),
            ("noise.snap", 0.7),
            ("out.drive", 0.15),
            ("out.room", 0.05),
        ],
    },
    Preset {
        name: "Rimshot",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 300.0),
            ("body.decay", 0.07),
            ("body.overtone", 2.0),
            ("body.mix", 0.7),
            ("noise.decay", 0.08),
            ("noise.tone", 7_500.0),
            ("noise.snap", 1.0),
            ("out.drive", 0.3),
            ("out.room", 0.1),
        ],
    },
    Preset {
        name: "Lo-Fi",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 170.0),
            ("body.decay", 0.2),
            ("body.mix", 0.35),
            ("noise.decay", 0.35),
            ("noise.tone", 2_400.0),
            ("noise.snap", 0.25),
            ("out.drive", 0.65),
            ("out.room", 0.0),
            ("out.velocity", 0.5),
        ],
    },
    Preset {
        name: "Brush",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 180.0),
            ("body.decay", 0.1),
            ("body.mix", 0.2),
            ("noise.decay", 0.5),
            ("noise.tone", 2_800.0),
            ("noise.snap", 0.0),
            ("out.drive", 0.0),
            ("out.room", 0.0),
            ("out.velocity", 0.9),
        ],
    },
    Preset {
        name: "Trap",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 245.0),
            ("body.decay", 0.1),
            ("body.overtone", 1.74),
            ("body.mix", 0.35),
            ("noise.decay", 0.16),
            ("noise.tone", 7_000.0),
            ("noise.snap", 0.8),
            ("out.drive", 0.35),
            ("out.room", 0.08),
        ],
    },
    Preset {
        name: "Clap Snare",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 205.0),
            ("body.decay", 0.11),
            ("body.mix", 0.3),
            ("noise.decay", 0.28),
            ("noise.tone", 2_400.0),
            ("noise.snap", 0.45),
            ("noise.burst", 0.9),
            ("out.drive", 0.25),
            ("out.room", 0.3),
        ],
    },
    Preset {
        name: "Big Room",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 185.0),
            ("body.decay", 0.22),
            ("body.mix", 0.55),
            ("noise.decay", 0.4),
            ("noise.tone", 5_200.0),
            ("noise.snap", 0.65),
            ("out.drive", 0.4),
            ("out.room", 0.5),
        ],
    },
    Preset {
        name: "Pop",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 215.0),
            ("body.decay", 0.13),
            ("body.mix", 0.45),
            ("noise.decay", 0.2),
            ("noise.tone", 5_600.0),
            ("noise.snap", 0.55),
            ("out.drive", 0.25),
            ("out.room", 0.2),
        ],
    },
    Preset {
        name: "Acoustic Snare",
        params: &[
            ("body.acoustic", 1.0),
            ("body.circuit", 0.0),
            ("body.pitch", 205.0),
            ("body.decay", 0.22),
            ("body.overtone", 1.59),
            ("body.mix", 0.5),
            ("noise.decay", 0.26),
            ("noise.tone", 5_000.0),
            ("noise.snap", 0.5),
            ("noise.burst", 0.0),
            ("out.drive", 0.15),
            ("out.room", 0.2),
            ("out.velocity", 0.75),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Rock Snare",
        params: &[
            ("body.acoustic", 1.0),
            ("body.circuit", 0.0),
            ("body.pitch", 185.0),
            ("body.decay", 0.3),
            ("body.overtone", 1.62),
            ("body.mix", 0.6),
            ("noise.decay", 0.32),
            ("noise.tone", 4_200.0),
            ("noise.snap", 0.75),
            ("noise.burst", 0.0),
            ("out.drive", 0.18),
            ("out.room", 0.35),
            ("out.velocity", 0.65),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "909 Snare",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 1.0),
            ("body.pitch", 200.0),
            ("body.decay", 0.14),
            ("body.overtone", 1.62),
            ("body.mix", 0.45),
            ("noise.decay", 0.26),
            ("noise.tone", 5_500.0),
            ("noise.snap", 0.6),
            ("noise.burst", 0.0),
            ("out.drive", 0.3),
            ("out.room", 0.1),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "808 Snare",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 1.0),
            ("body.pitch", 238.0),
            ("body.decay", 0.12),
            ("body.overtone", 1.97),
            ("body.mix", 0.55),
            ("noise.decay", 0.17),
            ("noise.tone", 6_500.0),
            ("noise.snap", 0.35),
            ("noise.burst", 0.0),
            ("out.drive", 0.12),
            ("out.room", 0.0),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Rim Click",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 330.0),
            ("body.decay", 0.045),
            ("body.overtone", 2.1),
            ("body.mix", 0.85),
            ("noise.decay", 0.05),
            ("noise.tone", 6_500.0),
            ("noise.snap", 1.0),
            ("out.drive", 0.25),
            ("out.room", 0.0),
        ],
    },
    Preset {
        name: "Finger Snap",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 380.0),
            ("body.decay", 0.04),
            ("body.mix", 0.15),
            ("noise.decay", 0.09),
            ("noise.tone", 3_200.0),
            ("noise.snap", 0.9),
            ("noise.burst", 0.5),
            ("out.drive", 0.1),
            ("out.room", 0.2),
        ],
    },
    Preset {
        name: "Gated",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 175.0),
            ("body.decay", 0.22),
            ("body.mix", 0.55),
            ("noise.decay", 0.42),
            ("noise.tone", 4_200.0),
            ("noise.snap", 0.55),
            ("noise.burst", 0.2),
            ("out.drive", 0.3),
            ("out.room", 0.6),
        ],
    },
    Preset {
        name: "Dusty",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 165.0),
            ("body.decay", 0.18),
            ("body.mix", 0.5),
            ("noise.decay", 0.22),
            ("noise.tone", 3_000.0),
            ("noise.snap", 0.4),
            ("out.drive", 0.5),
            ("out.room", 0.1),
            ("out.velocity", 0.6),
        ],
    },
    Preset {
        name: "Piccolo",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
            ("body.pitch", 265.0),
            ("body.decay", 0.1),
            ("body.overtone", 1.8),
            ("body.mix", 0.45),
            ("noise.decay", 0.16),
            ("noise.tone", 7_000.0),
            ("noise.snap", 0.8),
            ("out.drive", 0.2),
            ("out.room", 0.1),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Snare::new()))
}

/// Ideal membrane mode ratios of the batter head, (0,1) first, with each
/// mode's number of nodal diameters (0 for the axisymmetric modes).
const AS_RATIO: [f32; 10] = [
    1.0, 1.593, 2.136, 2.296, 2.653, 2.918, 3.156, 3.501, 3.600, 4.06,
];
const AS_DIAM: [u8; 10] = [0, 1, 2, 0, 3, 1, 4, 2, 0, 3];
const AS_GAIN: [f32; 10] = [1.0, 0.8, 0.6, 0.45, 0.42, 0.36, 0.3, 0.26, 0.2, 0.2];
/// Decay of each batter mode as a multiple of the body Decay.
const AS_DECAY: [f32; 10] = [1.0, 0.75, 0.58, 0.6, 0.46, 0.44, 0.38, 0.34, 0.36, 0.3];
/// The snare-side head, tuned above the batter: ratio, gain, decay.
const AS_RESO: [(f32, f32, f32); 2] = [(1.48, 0.32, 0.8), (2.36, 0.18, 0.5)];
/// Batter modes plus the two snare-side modes.
const AS_MODES: usize = 12;

/// The acoustic snare: a stick pulse driving batter and snare-side head
/// modes; wires that rattle as long as the heads move, gated by the
/// fundamental's own motion; a separate stick crack; and early reflections
/// from a small room.
#[derive(Clone, Copy)]
struct AcSnareVoice {
    active: bool,
    modes: [Mode; AS_MODES],
    base: [f32; AS_MODES],
    weight: [f32; AS_MODES],
    pulse: Pulse,
    drop: f32,
    pitch_env: f32,
    pitch_coef: f32,
    noise: Noise,
    band: Svf,
    band_lo: Svf,
    top1: OnePole,
    top2: OnePole,
    drive_lp: OnePole,
    wires: Decay,
    wire_gain: f32,
    crack_noise: Noise,
    crack_band: Svf,
    crack_hp: OnePole,
    crack: Decay,
    crack_gain: f32,
    body_gain: f32,
    gain: f32,
    push: f32,
    fade: Fade,
    xgain: f32,
    bend: f32,
    tone: f32,
    band_hz: f32,
}

impl AcSnareVoice {
    fn new() -> AcSnareVoice {
        AcSnareVoice {
            active: false,
            modes: [Mode::new(); AS_MODES],
            base: [200.0; AS_MODES],
            weight: [0.0; AS_MODES],
            pulse: Pulse::new(),
            drop: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            noise: Noise::new(21),
            band: Svf::new(),
            band_lo: Svf::new(),
            top1: OnePole::new(),
            top2: OnePole::new(),
            drive_lp: OnePole::new(),
            wires: Decay::new(),
            wire_gain: 0.0,
            crack_noise: Noise::new(22),
            crack_band: Svf::new(),
            crack_hp: OnePole::new(),
            crack: Decay::new(),
            crack_gain: 0.0,
            body_gain: 1.0,
            gain: 1.0,
            push: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            bend: 1.0,
            tone: 0.0,
            band_hz: 5_000.0,
        }
    }

    fn set_tone(&mut self, x: &Vx, sr: f32) {
        if x.tone != self.tone {
            self.tone = x.tone;
            self.band.set_q(
                (self.band_hz * x.tone_ratio(TIMBRE_OCTAVES)).min(0.4 * sr),
                0.75,
                sr,
            );
        }
    }

    /// Head tension falls back as the stroke's energy spreads; control rate.
    fn retune(&mut self, sr_inv: f32) {
        let m = (1.0 + self.drop * self.pitch_env) * self.bend;
        for (md, &f) in self.modes.iter_mut().zip(self.base.iter()) {
            md.tune(f * m, sr_inv);
            md.flush();
        }
    }

    #[inline]
    fn tick(&mut self, drive_g: f32, drive_norm: f32, drive_mix: f32) -> f32 {
        self.pitch_env *= self.pitch_coef;
        let x = self.pulse.tick();
        let mut body = 0.0;
        for (m, &w) in self.modes.iter_mut().zip(self.weight.iter()) {
            body += m.tick(x * w);
        }
        // Wires: they buzz while the heads move (the square root makes soft
        // strokes relatively wirier, as on a real snare) and slap against
        // the snare-side head once per cycle of its motion.
        let motion = self.modes[0].amp() + 0.6 * self.modes[1].amp() + 0.8 * self.modes[10].amp();
        let slap = 0.65 + 0.7 * self.modes[10].unit().max(0.0);
        let n = self.noise.white();
        let wire_src = self.band.tick(n).band_unity + 0.3 * self.band_lo.tick(n).band_unity;
        let wires = wire_src * motion.sqrt() * slap * self.wires.tick() * self.wire_gain;
        // Stick crack: a short bright burst at the moment of contact.
        let c = self.crack_band.tick(self.crack_noise.white()).band_unity;
        let crack = self.crack_hp.high(c) * self.crack.tick() * self.crack_gain;
        let top = self.top2.low(self.top1.low(wires + crack));
        let mix = body * self.body_gain * self.push + top;
        let y = lerp(mix, shape(mix, drive_g, drive_norm), drive_mix);
        let y = self.drive_lp.low(y);
        let out = y * self.gain * self.fade.tick() * self.xgain;
        if self.fade.is_done()
            || (!self.pulse.busy() && motion < 2.0e-4 && self.crack.is_below(1.0e-4))
        {
            self.active = false;
        }
        out
    }
}

/// The circuit snare: two bridged-T style resonators pinged by a trigger
/// pulse (with a small accent-dependent pitch drop), a "snappy" noise
/// layer through a high-pass and band-pass with a two-stage decay, a pulse
/// tick, and anti-aliased drive. A retrigger on the same key re-pings the
/// ringing resonators and restarts the envelopes from where they are, as
/// the circuits do, so rolls stay continuous.
#[derive(Clone, Copy)]
struct CkSnareVoice {
    active: bool,
    modes: [Mode; 2],
    base: [f32; 2],
    weight: [f32; 2],
    pulse: Pulse,
    drop: f32,
    pitch_env: f32,
    pitch_coef: f32,
    noise: Noise,
    hp: Svf,
    band: Svf,
    fast: Decay,
    slow: Decay,
    nz_gain: f32,
    tick_pulse: Pulse,
    tick_hp: OnePole,
    tick_gain: f32,
    drive: Adaa,
    top: OnePole,
    body_gain: f32,
    push: f32,
    gain: f32,
    fade: Fade,
    xgain: f32,
    bend: f32,
    tone: f32,
    band_hz: f32,
}

impl CkSnareVoice {
    fn new() -> CkSnareVoice {
        CkSnareVoice {
            active: false,
            modes: [Mode::new(); 2],
            base: [200.0; 2],
            weight: [0.0; 2],
            pulse: Pulse::new(),
            drop: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            noise: Noise::new(61),
            hp: Svf::new(),
            band: Svf::new(),
            fast: Decay::new(),
            slow: Decay::new(),
            nz_gain: 0.0,
            tick_pulse: Pulse::new(),
            tick_hp: OnePole::new(),
            tick_gain: 0.0,
            drive: Adaa::new(),
            top: OnePole::new(),
            body_gain: 1.0,
            push: 1.0,
            gain: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            bend: 1.0,
            tone: 0.0,
            band_hz: 5_000.0,
        }
    }

    fn set_tone(&mut self, x: &Vx, sr: f32) {
        if x.tone != self.tone {
            self.tone = x.tone;
            self.band.set_q(
                (self.band_hz * x.tone_ratio(TIMBRE_OCTAVES)).min(0.4 * sr),
                0.6,
                sr,
            );
        }
    }

    fn retune(&mut self, sr_inv: f32) {
        let m = (1.0 + self.drop * self.pitch_env) * self.bend;
        for (md, &f) in self.modes.iter_mut().zip(self.base.iter()) {
            md.tune(f * m, sr_inv);
            md.flush();
        }
    }

    #[inline]
    fn tick(&mut self, drive_g: f32, drive_norm: f32, drive_mix: f32) -> f32 {
        self.pitch_env *= self.pitch_coef;
        let x = self.pulse.tick();
        let body = self.modes[0].tick(x * self.weight[0]) + self.modes[1].tick(x * self.weight[1]);
        let n = self.noise.white();
        let hp = self.hp.tick(n).high;
        let nz = (0.45 * hp + self.band.tick(n).band_unity)
            * (0.6 * self.fast.tick() + 0.4 * self.slow.tick())
            * self.nz_gain;
        let tick = self.tick_hp.high(self.tick_pulse.tick()) * self.tick_gain;
        let mix = body * self.body_gain * self.push + self.top.low(nz + tick);
        let y = lerp(mix, self.drive.tick(mix * drive_g) * drive_norm, drive_mix);
        let out = y * self.gain * self.fade.tick() * self.xgain;
        if self.fade.is_done()
            || (!self.pulse.busy()
                && self.modes[0].amp() < 1.0e-4
                && self.modes[1].amp() < 1.0e-4
                && self.slow.is_below(1.0e-4))
        {
            self.active = false;
        }
        out
    }
}

impl PoolVoice for CkSnareVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.modes[0].amp().max(self.slow.level())
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

impl PoolVoice for AcSnareVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.modes[0].amp()
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

/// Burst spacing of the clap-style attack, in seconds.
const BURST_GAP: f32 = 0.009;
/// Number of re-attacks (including the first) in the clap-style attack.
const BURSTS: u32 = 3;

#[derive(Clone, Copy)]
struct SnareVoice {
    active: bool,
    ph1: f32,
    ph2: f32,
    ph3: f32,
    f1: f32,
    f2: f32,
    f3: f32,
    drop: f32,
    pitch_env: f32,
    pitch_coef: f32,
    amp1: Decay,
    amp2: Decay,
    amp3: Decay,
    wires: Decay,
    wires_fast: Decay,
    noise: Noise,
    band: Svf,
    band_lo: Svf,
    top1: OnePole,
    top2: OnePole,
    snap_hp: OnePole,
    snap_env: f32,
    snap_coef: f32,
    snap_gain: f32,
    body_gain: f32,
    noise_gain: f32,
    gain: f32,
    push: f32,
    t: u32,
    gap: u32,
    burst: f32,
    burst_k: f32,
    room_env: Decay,
    room_gain: f32,
    room_l: Noise,
    room_r: Noise,
    room_bp_l: Svf,
    room_bp_r: Svf,
    fade: Fade,
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
    /// Pitch bend ratio, 1.0 when centred.
    bend: f32,
    /// Timbre the wire band is tuned for (bipolar, 0 neutral) and its base.
    tone: f32,
    band_hz: f32,
}

impl SnareVoice {
    /// Timbre moves the wire band up or down by up to two octaves.
    fn set_tone(&mut self, x: &Vx, sr: f32) {
        if x.tone != self.tone {
            self.tone = x.tone;
            self.band.set_q(
                (self.band_hz * x.tone_ratio(TIMBRE_OCTAVES)).min(0.4 * sr),
                0.8,
                sr,
            );
        }
    }

    fn new() -> SnareVoice {
        let mut snap_hp = OnePole::new();
        snap_hp.set_cutoff(2_500.0, 48_000.0);
        SnareVoice {
            active: false,
            ph1: 0.0,
            ph2: 0.0,
            ph3: 0.0,
            f1: 190.0,
            f2: 320.0,
            f3: 400.0,
            drop: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            amp1: Decay::new(),
            amp2: Decay::new(),
            amp3: Decay::new(),
            wires: Decay::new(),
            wires_fast: Decay::new(),
            noise: Noise::new(1),
            band: Svf::new(),
            band_lo: Svf::new(),
            top1: OnePole::new(),
            top2: OnePole::new(),
            snap_hp,
            snap_env: 0.0,
            snap_coef: 0.0,
            snap_gain: 0.0,
            body_gain: 1.0,
            noise_gain: 1.0,
            gain: 1.0,
            push: 1.0,
            t: 0,
            gap: 400,
            burst: 0.0,
            burst_k: 0.0,
            room_env: Decay::new(),
            room_gain: 0.0,
            room_l: Noise::new(3),
            room_r: Noise::new(4),
            room_bp_l: Svf::new(),
            room_bp_r: Svf::new(),
            fade: Fade::new(),
            xgain: 1.0,
            bend: 1.0,
            tone: 0.0,
            band_hz: 4_800.0,
        }
    }

    /// Returns `(mono, room_left, room_right)`.
    #[inline]
    fn tick(
        &mut self,
        sr_inv: f32,
        drive_g: f32,
        drive_norm: f32,
        drive_mix: f32,
    ) -> (f32, f32, f32) {
        let m = 1.0 + self.drop * self.pitch_env;
        self.pitch_env *= self.pitch_coef;
        for (ph, f) in [
            (&mut self.ph1, self.f1),
            (&mut self.ph2, self.f2),
            (&mut self.ph3, self.f3),
        ] {
            *ph += (f * m * self.bend * sr_inv).min(0.45);
            if *ph >= 1.0 {
                *ph -= 1.0;
            }
        }
        let a1 = self.amp1.tick();
        let a2 = self.amp2.tick();
        let a3 = self.amp3.tick();
        let body = fast_sin(self.ph1) * a1
            + 0.5 * fast_sin(self.ph2) * a2
            + 0.22 * fast_sin(self.ph3) * a3;

        // Clap-style re-attacks: the noise is re-struck a few times at the
        // start. `burst` is the depth of the gaps between strikes.
        let strike = if self.burst > 0.0 && self.t < self.gap * BURSTS {
            let u = (self.t % self.gap) as f32;
            1.0 - self.burst * (1.0 - (-u * self.burst_k).exp())
        } else {
            1.0
        };
        self.t = self.t.saturating_add(1);

        let n = self.noise.white();
        let w = 0.5 * self.wires_fast.tick() + 0.5 * self.wires.tick();
        // Wires: a broad band around the tone control plus a lower band for
        // body in the mids, rolled off above ~12 kHz like real wires and mics.
        let wires = (self.band.tick(n).band_unity + 0.3 * self.band_lo.tick(n).band_unity) * w;
        let snap = self.snap_hp.high(n) * self.snap_env * self.snap_gain;
        self.snap_env *= self.snap_coef;
        let top = self.top2.low(self.top1.low(wires * self.noise_gain + snap));

        let mix = body * self.body_gain * self.push + top * strike;
        let y = lerp(mix, shape(mix, drive_g, drive_norm), drive_mix);
        let fade = self.gain * self.fade.tick() * self.xgain;

        let r = self.room_env.tick() * self.room_gain;
        let rl = self.room_bp_l.tick(self.room_l.white()).band_unity * r * fade;
        let rr = self.room_bp_r.tick(self.room_r.white()).band_unity * r * fade;

        if self.fade.is_done()
            || (self.amp1.is_below(1.0e-4)
                && self.wires.is_below(1.0e-4)
                && self.room_env.is_below(1.0e-4))
        {
            self.active = false;
        }
        (y * fade, rl, rr)
    }
}

impl PoolVoice for SnareVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.amp1.level().max(self.wires.level())
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

pub(crate) struct Snare {
    expr: Expression,
    sr: f32,
    pool: Pool<SnareVoice>,
    ac: Pool<AcSnareVoice>,
    ck: Pool<CkSnareVoice>,
    acoustic: bool,
    circuit: bool,
    room_er: Room,
    ramp_k: f32,
    ctrl: usize,
    hits: u32,
    pitch: f32,
    body_decay: f32,
    overtone: f32,
    mix: f32,
    noise_decay: f32,
    noise_tone: f32,
    snap: f32,
    burst: f32,
    room: f32,
    vel_sens: f32,
    drive: Ramp,
    level: Ramp,
    drive_g: f32,
    drive_norm: f32,
    drive_mix: f32,
    level_gain: f32,
}

impl Snare {
    pub(crate) fn new() -> Snare {
        Snare {
            expr: Expression::new(),
            sr: 48_000.0,
            pool: Pool::new(5, 2, StealPolicy::Oldest, 48_000.0, SnareVoice::new),
            ac: Pool::new(5, 2, StealPolicy::Oldest, 48_000.0, AcSnareVoice::new),
            ck: Pool::new(5, 2, StealPolicy::Oldest, 48_000.0, CkSnareVoice::new),
            acoustic: false,
            circuit: false,
            room_er: Room::new(),
            ramp_k: 0.1,
            ctrl: 0,
            hits: 0,
            pitch: 190.0,
            body_decay: 0.16,
            overtone: 1.68,
            mix: 0.5,
            noise_decay: 0.24,
            noise_tone: 4_800.0,
            snap: 0.5,
            burst: 0.0,
            room: 0.15,
            vel_sens: 0.7,
            drive: Ramp::new(0.2),
            level: Ramp::new(0.0),
            drive_g: 1.0,
            drive_norm: 1.0,
            drive_mix: 0.0,
            level_gain: 1.0,
        }
    }

    fn derive(&mut self) {
        let drive = self.drive.get();
        (self.drive_g, self.drive_norm) = drive_curve(drive);
        self.drive_mix = (drive * 5.0).min(1.0);
        self.level_gain = db_to_gain(self.level.get()) * 0.7;
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.drive.tick(k);
        self.level.tick(k);
        self.derive();
        self.expr.tick();
        let ex = &self.expr;
        let sr = self.sr;
        self.pool.for_active_id(|v, key, id| {
            let x = ex.voice(id, key);
            v.xgain = x.gain();
            v.bend = x.ratio;
            v.set_tone(&x, sr);
        });
        let sr_inv = 1.0 / sr;
        self.ac.for_active_id(|v, key, id| {
            let x = ex.voice(id, key);
            v.xgain = x.gain();
            v.bend = x.ratio;
            v.set_tone(&x, sr);
            v.retune(sr_inv);
        });
        self.ck.for_active_id(|v, key, id| {
            let x = ex.voice(id, key);
            v.xgain = x.gain();
            v.bend = x.ratio;
            v.set_tone(&x, sr);
            v.retune(sr_inv);
        });
    }

    /// Starts a circuit hit, re-pinging a voice already ringing on the key.
    fn ck_note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let sr = self.sr;
        let (slot, warm) = analog_slot(&mut self.ck, key, id, false, |v| v.fade.is_fading());
        self.hits = self.hits.wrapping_add(1);
        let ratio = st_ratio(key_offset(key));
        let f1 = (self.pitch * ratio).clamp(30.0, 0.1 * sr);
        let center = (self.noise_tone * ratio.sqrt() * lerp(0.8, 1.05, velocity)).min(0.4 * sr);
        let hits = self.hits;
        let x = self.expr.voice(id, key);
        let v = &mut self.ck.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        v.bend = x.ratio;
        v.base = [f1, (f1 * self.overtone).min(0.2 * sr)];
        v.weight = [0.9, 0.5];
        let decays = [self.body_decay, self.body_decay * 0.6];
        for ((m, &f), &d) in v.modes.iter_mut().zip(v.base.iter()).zip(decays.iter()) {
            if warm {
                m.retarget(d, sr);
            } else {
                m.start(f, d, sr);
            }
        }
        v.drop = (0.03 + 0.09 * self.snap) * velocity;
        v.pitch_env = 1.0;
        v.pitch_coef = (-1.0 / (0.01 * sr)).exp();
        v.retune(1.0 / sr);
        v.pulse.start(0.000_25, sr);
        v.noise.reseed(hits.wrapping_mul(5).wrapping_add(900));
        v.band_hz = center;
        v.tone = 0.0;
        v.band.set_q(center, 0.6, sr);
        v.set_tone(&x, sr);
        v.hp.set_q((center * 0.45).max(1_200.0).min(0.4 * sr), 0.7, sr);
        v.top.set_cutoff(14_000.0, sr);
        if !warm {
            v.band.reset();
            v.hp.reset();
            v.top.reset();
            v.drive.reset();
            v.fast.reset();
            v.slow.reset();
        }
        // Accent: louder, longer snappy noise.
        let nd = self.noise_decay * lerp(0.75, 1.1, velocity);
        v.fast.set_t60(nd * 0.25, sr);
        v.fast.trigger(1.0, 0.000_3, sr);
        v.slow.set_t60(nd, sr);
        v.slow.trigger(1.0, 0.000_5, sr);
        v.nz_gain = lerp(1.5, 0.45, self.mix) * velocity.powf(0.3);
        v.tick_pulse.start(0.000_1, sr);
        v.tick_hp.set_cutoff(2_000.0, sr);
        if !warm {
            v.tick_hp.reset();
        }
        v.tick_gain = self.snap * 0.8 * velocity;
        v.body_gain = lerp(0.3, 1.1, self.mix);
        v.push = lerp(0.75, 1.1, velocity);
        v.gain = vel_gain(velocity, self.vel_sens);
        v.fade.reset();
    }

    /// Starts an acoustic stroke.
    fn ac_note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.ac.begin(key, id);
        self.hits = self.hits.wrapping_add(1);
        let sr = self.sr;
        let mut rng = HitRng::new(self.hits, 0x5a4e);
        let ratio = st_ratio(key_offset(key));
        let f1 = (self.pitch * ratio).clamp(30.0, 0.1 * sr);
        // Overtone sets the second mode's ratio; the rest spread with it.
        let spread = (self.overtone - 1.0) / 0.593;
        // Strike radius as a fraction of the head: a third of the way out on
        // average, and a different spot on every stroke.
        let radius = 0.33 + 0.12 * rng.next();
        let contact = lerp(0.000_8, 0.000_28, velocity) * (1.0 + 0.1 * rng.next());
        let force = velocity.max(0.1);
        let center = (self.noise_tone * ratio.sqrt() * lerp(0.75, 1.0, velocity)).min(0.4 * sr);
        let hits = self.hits;
        let x = self.expr.voice(id, key);
        let v = &mut self.ac.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        v.bend = x.ratio;
        for k in 0..10 {
            v.base[k] = f1 * (1.0 + (AS_RATIO[k] - 1.0) * spread);
            let pos = if AS_DIAM[k] == 0 {
                1.2 - 0.9 * radius
            } else {
                (0.3 + 2.1 * radius).min(1.6)
            };
            v.weight[k] = 0.65 * AS_GAIN[k] * pos * (1.0 + 0.08 * rng.next());
            v.modes[k].start(v.base[k], (self.body_decay * AS_DECAY[k]).max(0.01), sr);
        }
        for (k, &(r, g, d)) in AS_RESO.iter().enumerate() {
            let i = 10 + k;
            v.base[i] = f1 * r;
            v.weight[i] = 0.65 * g * (1.0 + 0.1 * rng.next());
            v.modes[i].start(v.base[i], (self.body_decay * d).max(0.01), sr);
        }
        // Tension modulation: a hard stroke starts a little sharp.
        v.drop = (0.015 + 0.04 * self.snap) * velocity.powf(1.5);
        v.pitch_env = 1.0;
        v.pitch_coef = (-1.0 / (0.012 * sr)).exp();
        v.retune(1.0 / sr);
        v.pulse.start(contact, sr);
        v.noise.reseed(hits.wrapping_mul(3).wrapping_add(17));
        v.band_hz = center;
        v.tone = 0.0;
        v.band.set_q(center, 0.75, sr);
        v.set_tone(&x, sr);
        v.band.reset();
        v.band_lo.set_q(1_600.0, 0.7, sr);
        v.band_lo.reset();
        for f in [&mut v.top1, &mut v.top2] {
            f.set_cutoff(12_500.0, sr);
            f.reset();
        }
        v.drive_lp.set_cutoff(16_000.0, sr);
        v.drive_lp.reset();
        v.wires
            .set_t60(self.noise_decay * lerp(0.75, 1.15, velocity), sr);
        v.wires.trigger(1.0, 0.000_6, sr);
        // Softer strokes leave the wires relatively louder than the head.
        v.wire_gain = lerp(1.6, 0.5, self.mix) * 0.9 / force.sqrt() * (1.0 + 0.06 * rng.next());
        v.crack_noise.reseed(hits.wrapping_mul(3).wrapping_add(18));
        v.crack_band
            .set_q((3_400.0 * (1.0 + 0.12 * rng.next())).min(0.4 * sr), 0.9, sr);
        v.crack_band.reset();
        v.crack_hp.set_cutoff(1_200.0, sr);
        v.crack_hp.reset();
        v.crack.set_t60(lerp(0.01, 0.016, self.snap), sr);
        v.crack.trigger(1.0, 0.000_15, sr);
        v.crack_gain = self.snap * 0.9 * velocity.powf(1.3) * (1.0 + 0.15 * rng.next());
        v.body_gain = lerp(0.25, 1.0, self.mix);
        v.push = lerp(0.8, 1.0, velocity);
        v.gain = vel_gain(velocity, self.vel_sens) * (1.0 + 0.03 * rng.next());
        v.fade.reset();
    }
}

impl Synth for Snare {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(5, 2, StealPolicy::Oldest, sample_rate, SnareVoice::new);
        self.ac = Pool::new(5, 2, StealPolicy::Oldest, sample_rate, AcSnareVoice::new);
        self.ck = Pool::new(5, 2, StealPolicy::Oldest, sample_rate, CkSnareVoice::new);
        self.room_er.prepare(sample_rate);
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ac.kill_all();
        self.ck.kill_all();
        self.room_er.clear();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, immediate: bool) {
        match id {
            PITCH => self.pitch = v,
            BODY_DECAY => self.body_decay = v,
            OVERTONE => self.overtone = v,
            MIX => self.mix = v,
            NOISE_DECAY => self.noise_decay = v,
            NOISE_TONE => self.noise_tone = v,
            SNAP => self.snap = v,
            BURST => self.burst = v,
            ROOM => self.room = v,
            DRIVE => self.drive.set(v, immediate),
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

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        if self.acoustic {
            self.ac_note_on(key, velocity, id);
            return;
        }
        if self.circuit {
            self.ck_note_on(key, velocity, id);
            return;
        }
        let slot = self.pool.begin(key, id);
        self.hits = self.hits.wrapping_add(1);
        let sr = self.sr;
        let ratio = st_ratio(key_offset(key));
        let f1 = (self.pitch * ratio).clamp(30.0, 0.3 * sr);
        let f2 = (f1 * self.overtone).min(0.4 * sr);
        // Third membrane mode, about 1.27 times the second (2.14 : 1.59).
        let f3 = (f2 * 1.27).min(0.4 * sr);
        let center = (self.noise_tone * ratio.sqrt() * lerp(0.7, 1.0, velocity)).min(0.4 * sr);
        let hits = self.hits;
        let x = self.expr.voice(id, key);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        v.bend = x.ratio;
        v.ph1 = 0.0;
        v.ph2 = 0.0;
        v.ph3 = 0.0;
        v.f1 = f1;
        v.f2 = f2;
        v.f3 = f3;
        v.drop = 0.12 + 0.5 * self.snap * lerp(0.5, 1.0, velocity);
        v.pitch_env = 1.0;
        v.pitch_coef = (-1.0 / (0.012 * sr)).exp();
        v.amp1.set_t60(self.body_decay, sr);
        v.amp1.trigger(1.0, 0.000_3, sr);
        v.amp2.set_t60(self.body_decay * 0.6, sr);
        v.amp2.trigger(1.0, 0.000_3, sr);
        v.amp3.set_t60(self.body_decay * 0.35, sr);
        v.amp3.trigger(1.0, 0.000_3, sr);
        v.wires.set_t60(self.noise_decay, sr);
        v.wires.trigger(1.0, 0.000_5, sr);
        v.wires_fast.set_t60(self.noise_decay * 0.22, sr);
        v.wires_fast.trigger(1.0, 0.000_5, sr);
        v.noise.reseed(hits);
        v.band_hz = center;
        v.tone = 0.0;
        v.band.set_q(center, 0.8, sr);
        v.set_tone(&x, sr);
        v.band.reset();
        v.snap_hp.set_cutoff(2_500.0, sr);
        v.snap_hp.reset();
        v.band_lo.set_q(1_400.0, 0.7, sr);
        v.band_lo.reset();
        v.top1.set_cutoff(12_500.0, sr);
        v.top1.reset();
        v.top2.set_cutoff(12_500.0, sr);
        v.top2.reset();
        v.snap_env = 1.0;
        v.snap_coef = (-1.0 / (0.004 * sr)).exp();
        v.snap_gain = self.snap * 1.1 * lerp(0.4, 1.0, velocity);
        v.body_gain = lerp(0.2, 1.0, self.mix);
        v.noise_gain = lerp(2.0, 0.45, self.mix) * lerp(0.6, 1.0, velocity);
        v.gain = vel_gain(velocity, self.vel_sens);
        // Harder hits push the shaper and the body harder.
        v.push = lerp(0.8, 1.0, velocity);
        v.t = 0;
        v.gap = (BURST_GAP * sr) as u32;
        v.burst = self.burst;
        v.burst_k = 4.0 / v.gap as f32;
        v.room_env.set_t60(0.18 + 0.5 * self.noise_decay, sr);
        v.room_env.trigger(1.0, 0.002, sr);
        v.room_gain = self.room * 0.28 * lerp(0.5, 1.0, velocity);
        v.room_l.reseed(hits.wrapping_mul(2).wrapping_add(1_000));
        v.room_r.reseed(hits.wrapping_mul(2).wrapping_add(1_001));
        v.room_bp_l.set_q(2_200.0, 0.45, sr);
        v.room_bp_l.reset();
        v.room_bp_r.set_q(2_200.0, 0.45, sr);
        v.room_bp_r.reset();
        v.fade.reset();
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, _id: u32) {}

    fn all_notes_off(&mut self) {}

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
        self.ac.kill_all();
        self.ck.kill_all();
        self.room_er.clear();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        let sr_inv = 1.0 / self.sr;
        for n in 0..left.len().min(right.len()) {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let (mut out, mut rl, mut rr) = (0.0, 0.0, 0.0);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (m, l, r) = v.tick(sr_inv, self.drive_g, self.drive_norm, self.drive_mix);
                out += m;
                rl += l;
                rr += r;
            }
            // The acoustic model's room is fed from its dry hit; nothing here
            // runs for the electronic voices.
            if !self.ac.is_silent() || !self.ck.is_silent() || self.room_er.ringing() {
                let mut dry = 0.0;
                for v in self.ac.voices.iter_mut().filter(|v| v.active) {
                    dry += v.tick(self.drive_g, self.drive_norm, self.drive_mix);
                }
                for v in self.ck.voices.iter_mut().filter(|v| v.active) {
                    dry += v.tick(self.drive_g, self.drive_norm, self.drive_mix);
                }
                let (el, er) = self.room_er.tick(dry * self.room * 1.4);
                out += dry;
                rl += el;
                rr += er;
            }
            left[n] = (out + rl) * self.level_gain;
            right[n] = (out + rr) * self.level_gain;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
            && self.ac.is_silent()
            && self.ck.is_silent()
            && !self.room_er.ringing()
    }
}
