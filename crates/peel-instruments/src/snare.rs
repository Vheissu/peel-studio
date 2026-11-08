//! `peel.drum.snare`: two tuned head modes with a small pitch drop, a band-
//! passed noise "snare wire" layer with its own decay, and a short snap
//! transient.

use peel_dsp::envelope::Decay;
use peel_dsp::filter::{OnePole, Svf};
use peel_dsp::noise::Noise;
use peel_dsp::osc::fast_sin;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::db_to_gain;
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, drive_curve, key_offset,
    lerp, ramp_coef, shape, st_ratio, vel_gain,
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

static PARAMS: [ParamDescriptor; 10] = [
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
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.drum.snare",
    name: "Snare",
    description: "Synthesized snare with tuned head modes, noise wires and snap",
    category: "Drums",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Crack",
        params: &[
            ("body.pitch", 190.0),
            ("body.decay", 0.16),
            ("body.overtone", 1.68),
            ("body.mix", 0.5),
            ("noise.decay", 0.24),
            ("noise.tone", 4_800.0),
            ("noise.snap", 0.5),
            ("out.drive", 0.2),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Fat",
        params: &[
            ("body.pitch", 150.0),
            ("body.decay", 0.28),
            ("body.mix", 0.65),
            ("noise.decay", 0.3),
            ("noise.tone", 3_200.0),
            ("noise.snap", 0.35),
            ("out.drive", 0.4),
        ],
    },
    Preset {
        name: "Tight",
        params: &[
            ("body.pitch", 230.0),
            ("body.decay", 0.09),
            ("body.mix", 0.4),
            ("noise.decay", 0.12),
            ("noise.tone", 6_000.0),
            ("noise.snap", 0.7),
            ("out.drive", 0.15),
        ],
    },
    Preset {
        name: "Rimshot",
        params: &[
            ("body.pitch", 300.0),
            ("body.decay", 0.07),
            ("body.overtone", 2.0),
            ("body.mix", 0.7),
            ("noise.decay", 0.08),
            ("noise.tone", 7_500.0),
            ("noise.snap", 1.0),
            ("out.drive", 0.3),
        ],
    },
    Preset {
        name: "Lo-Fi",
        params: &[
            ("body.pitch", 170.0),
            ("body.decay", 0.2),
            ("body.mix", 0.35),
            ("noise.decay", 0.35),
            ("noise.tone", 2_400.0),
            ("noise.snap", 0.25),
            ("out.drive", 0.65),
            ("out.velocity", 0.5),
        ],
    },
    Preset {
        name: "Brush",
        params: &[
            ("body.pitch", 180.0),
            ("body.decay", 0.1),
            ("body.mix", 0.2),
            ("noise.decay", 0.5),
            ("noise.tone", 2_800.0),
            ("noise.snap", 0.0),
            ("out.drive", 0.0),
            ("out.velocity", 0.9),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Snare::new()))
}

#[derive(Clone, Copy)]
struct SnareVoice {
    active: bool,
    ph1: f32,
    ph2: f32,
    f1: f32,
    f2: f32,
    drop: f32,
    pitch_env: f32,
    pitch_coef: f32,
    amp1: Decay,
    amp2: Decay,
    wires: Decay,
    noise: Noise,
    band: Svf,
    snap_hp: OnePole,
    snap_env: f32,
    snap_coef: f32,
    snap_gain: f32,
    body_gain: f32,
    noise_gain: f32,
    gain: f32,
    fade: Fade,
}

impl SnareVoice {
    fn new() -> SnareVoice {
        let mut snap_hp = OnePole::new();
        snap_hp.set_cutoff(2_500.0, 48_000.0);
        SnareVoice {
            active: false,
            ph1: 0.0,
            ph2: 0.0,
            f1: 190.0,
            f2: 320.0,
            drop: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            amp1: Decay::new(),
            amp2: Decay::new(),
            wires: Decay::new(),
            noise: Noise::new(1),
            band: Svf::new(),
            snap_hp,
            snap_env: 0.0,
            snap_coef: 0.0,
            snap_gain: 0.0,
            body_gain: 1.0,
            noise_gain: 1.0,
            gain: 1.0,
            fade: Fade::new(),
        }
    }

    #[inline]
    fn tick(&mut self, sr_inv: f32, drive_g: f32, drive_norm: f32) -> f32 {
        let m = 1.0 + self.drop * self.pitch_env;
        self.pitch_env *= self.pitch_coef;
        self.ph1 += (self.f1 * m * sr_inv).min(0.45);
        self.ph2 += (self.f2 * m * sr_inv).min(0.45);
        if self.ph1 >= 1.0 {
            self.ph1 -= 1.0;
        }
        if self.ph2 >= 1.0 {
            self.ph2 -= 1.0;
        }
        let a1 = self.amp1.tick();
        let a2 = self.amp2.tick();
        let body = fast_sin(self.ph1) * a1 + 0.5 * fast_sin(self.ph2) * a2;

        let n = self.noise.white();
        let w = self.wires.tick();
        let wires = self.band.tick(n).band_unity * w;
        let snap = self.snap_hp.high(n) * self.snap_env * self.snap_gain;
        self.snap_env *= self.snap_coef;

        let mix = body * self.body_gain + wires * self.noise_gain + snap;
        let y = shape(mix, drive_g, drive_norm);
        let out = y * self.gain * self.fade.tick();
        if self.fade.is_done() || (self.amp1.is_below(1.0e-4) && self.wires.is_below(1.0e-4)) {
            self.active = false;
        }
        out
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
    sr: f32,
    pool: Pool<SnareVoice>,
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
    vel_sens: f32,
    drive: Ramp,
    level: Ramp,
    drive_g: f32,
    drive_norm: f32,
    level_gain: f32,
}

impl Snare {
    pub(crate) fn new() -> Snare {
        Snare {
            sr: 48_000.0,
            pool: Pool::new(5, 2, StealPolicy::Oldest, 48_000.0, SnareVoice::new),
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
            vel_sens: 0.7,
            drive: Ramp::new(0.2),
            level: Ramp::new(0.0),
            drive_g: 1.0,
            drive_norm: 1.0,
            level_gain: 1.0,
        }
    }

    fn derive(&mut self) {
        (self.drive_g, self.drive_norm) = drive_curve(self.drive.get());
        self.level_gain = db_to_gain(self.level.get()) * 0.8;
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.drive.tick(k);
        self.level.tick(k);
        self.derive();
    }
}

impl Synth for Snare {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(5, 2, StealPolicy::Oldest, sample_rate, SnareVoice::new);
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
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
            DRIVE => self.drive.set(v, immediate),
            VELOCITY => self.vel_sens = v,
            LEVEL => self.level.set(v, immediate),
            _ => {}
        }
        if immediate {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.pool.begin(key, id);
        self.hits = self.hits.wrapping_add(1);
        let sr = self.sr;
        let ratio = st_ratio(key_offset(key));
        let f1 = (self.pitch * ratio).clamp(30.0, 0.3 * sr);
        let f2 = (f1 * self.overtone).min(0.4 * sr);
        let center = (self.noise_tone * ratio.sqrt() * lerp(0.7, 1.0, velocity)).min(0.4 * sr);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.ph1 = 0.0;
        v.ph2 = 0.0;
        v.f1 = f1;
        v.f2 = f2;
        v.drop = 0.12 + 0.5 * self.snap * lerp(0.5, 1.0, velocity);
        v.pitch_env = 1.0;
        v.pitch_coef = (-1.0 / (0.012 * sr)).exp();
        v.amp1.set_t60(self.body_decay, sr);
        v.amp1.trigger(1.0, 0.000_3, sr);
        v.amp2.set_t60(self.body_decay * 0.6, sr);
        v.amp2.trigger(1.0, 0.000_3, sr);
        v.wires.set_t60(self.noise_decay, sr);
        v.wires.trigger(1.0, 0.000_5, sr);
        v.noise.reseed(self.hits);
        v.band.set_q(center, 0.8, sr);
        v.band.reset();
        v.snap_hp.set_cutoff(2_500.0, sr);
        v.snap_hp.reset();
        v.snap_env = 1.0;
        v.snap_coef = (-1.0 / (0.004 * sr)).exp();
        v.snap_gain = self.snap * 1.1 * lerp(0.4, 1.0, velocity);
        v.body_gain = lerp(0.2, 1.0, self.mix);
        v.noise_gain = lerp(1.6, 0.35, self.mix) * lerp(0.6, 1.0, velocity);
        v.gain = vel_gain(velocity, self.vel_sens) * self.level_gain;
        v.fade.reset();
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, _id: u32) {}

    fn all_notes_off(&mut self) {}

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        let sr_inv = 1.0 / self.sr;
        for n in 0..left.len().min(right.len()) {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let mut out = 0.0;
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                out += v.tick(sr_inv, self.drive_g, self.drive_norm);
            }
            left[n] = out;
            right[n] = out;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
