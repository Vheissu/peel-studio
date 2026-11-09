//! Pitched modal percussion with mallet, membrane, plate and bell models.
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

static PARAMS: [ParamDescriptor; 17] = [
    ParamDescriptor::linear(
        1,
        "mallet.hardness",
        "Hardness",
        "Mallet",
        0.0,
        1.0,
        0.45,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        2,
        "mallet.vel_tone",
        "Vel Tone",
        "Mallet",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        3,
        "mallet.noise",
        "Noise",
        "Mallet",
        0.0,
        1.0,
        0.1,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        4,
        "mallet.noise_decay",
        "Noise Decay",
        "Mallet",
        0.002,
        0.2,
        0.015,
        Unit::Seconds,
    ),
    ParamDescriptor::choice(
        5,
        "body.model",
        "Model",
        "Body",
        &["Bar", "Marimba", "Membrane", "Plate", "Bell"],
        1,
    ),
    ParamDescriptor::linear(
        6,
        "body.material",
        "Material",
        "Body",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        7,
        "body.decay",
        "Decay",
        "Body",
        0.05,
        8.0,
        1.2,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        8,
        "body.position",
        "Position",
        "Body",
        0.0,
        1.0,
        0.25,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        9,
        "body.stretch",
        "Stretch",
        "Body",
        -1.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        10,
        "body.tension",
        "Tension",
        "Body",
        0.0,
        12.0,
        0.0,
        Unit::Semitones,
    ),
    ParamDescriptor::toggle(11, "damp.enable", "Damper", "Damper", false),
    ParamDescriptor::log(
        12,
        "damp.release",
        "Release",
        "Damper",
        0.02,
        2.0,
        0.25,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        13,
        "pitch.tune",
        "Tune",
        "Pitch",
        -24.0,
        24.0,
        0.0,
        Unit::Semitones,
    ),
    ParamDescriptor::linear(
        14,
        "pitch.keytrack",
        "Key Track",
        "Pitch",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        15,
        "out.width",
        "Width",
        "Output",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        16,
        "out.velocity",
        "Velocity",
        "Output",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        17,
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
    type_id: "peel.perc.mallet",
    name: "Mallet",
    description: "Pitched modal percussion with mallet, membrane, plate and bell models",
    category: "Percussion",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, ramp_coef,
    vel_gain,
};
use peel_dsp::noise::Noise;
use peel_dsp::osc::fast_sin;
use peel_dsp::util::{db_to_gain, key_to_hz};
use peel_dsp::voice::StealPolicy;

pub static PRESETS: [Preset; 10] = [
    Preset {
        name: "Marimba",
        params: &[],
    },
    Preset {
        name: "Vibes",
        params: &[
            ("body.material", 0.9),
            ("body.decay", 4.0),
            ("mallet.hardness", 0.35),
            ("mallet.noise", 0.03),
        ],
    },
    Preset {
        name: "Glockenspiel",
        params: &[
            ("body.model", 0.0),
            ("body.material", 0.95),
            ("body.decay", 2.5),
            ("mallet.hardness", 0.85),
            ("mallet.noise", 0.05),
            ("body.position", 0.15),
        ],
    },
    Preset {
        name: "Kalimba",
        params: &[
            ("body.model", 0.0),
            ("body.material", 0.6),
            ("body.decay", 0.9),
            ("mallet.hardness", 0.7),
            ("mallet.noise", 0.2),
            ("mallet.noise_decay", 0.008),
            ("body.stretch", -0.2),
            ("body.position", 0.4),
        ],
    },
    Preset {
        name: "Wood Block",
        params: &[
            ("body.model", 0.0),
            ("body.material", 0.0),
            ("body.decay", 0.15),
            ("mallet.hardness", 0.9),
            ("mallet.noise", 0.25),
            ("pitch.keytrack", 0.5),
        ],
    },
    Preset {
        name: "Conga",
        params: &[
            ("body.model", 2.0),
            ("body.material", 0.2),
            ("body.decay", 0.5),
            ("body.tension", 2.0),
            ("pitch.keytrack", 0.25),
            ("mallet.hardness", 0.4),
            ("mallet.noise", 0.35),
            ("mallet.noise_decay", 0.02),
            ("body.position", 0.3),
        ],
    },
    Preset {
        name: "Tabla",
        params: &[
            ("body.model", 2.0),
            ("body.material", 0.5),
            ("body.decay", 0.7),
            ("body.tension", 5.0),
            ("mallet.hardness", 0.6),
            ("body.position", 0.15),
        ],
    },
    Preset {
        name: "Tuned Tom",
        params: &[
            ("body.model", 2.0),
            ("body.material", 0.3),
            ("body.decay", 0.6),
            ("body.tension", 8.0),
            ("pitch.keytrack", 0.5),
            ("mallet.hardness", 0.5),
            ("mallet.noise", 0.25),
        ],
    },
    Preset {
        name: "Metal Plate",
        params: &[
            ("body.model", 3.0),
            ("body.material", 0.85),
            ("body.decay", 1.6),
            ("mallet.hardness", 0.4),
            ("body.position", 0.2),
            ("out.width", 0.5),
        ],
    },
    Preset {
        name: "Temple Bell",
        params: &[
            ("body.model", 4.0),
            ("body.material", 1.0),
            ("body.decay", 7.0),
            ("mallet.hardness", 0.3),
            ("body.position", 0.3),
            ("out.width", 0.6),
        ],
    },
];

const RATIOS: [[f32; 8]; 5] = [
    [1.0, 2.756, 5.404, 8.933, 13.344, 18.638, 24.814, 31.871],
    [1.0, 3.99, 10.0, 17.6, 27.0, 38.1, 51.0, 65.6],
    [1.0, 1.594, 2.136, 2.296, 2.653, 2.918, 3.156, 3.501],
    [1.0, 2.5, 4.0, 5.0, 6.5, 8.5, 9.0, 10.0],
    [0.5, 1.0, 1.183, 1.506, 2.0, 2.514, 2.662, 3.011],
];

#[derive(Clone, Copy, Default)]
struct Mode {
    y1: f32,
    y2: f32,
    a: f32,
    b: f32,
    input: f32,
    weight: f32,
}
impl Mode {
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.a * self.y1 - self.b * self.y2 + self.input * x;
        self.y2 = self.y1;
        self.y1 = if y.abs() < 1e-25 { 0.0 } else { y };
        y * self.weight
    }
}
struct Voice {
    active: bool,
    key: u8,
    velocity: f32,
    modes: [Mode; 8],
    fade: Fade,
    age: usize,
    pulse_len: usize,
    noise: Noise,
    noise_env: f32,
    noise_coef: f32,
    released: bool,
    release_at: usize,
    gain: f32,
    max_age: usize,
}
impl Voice {
    fn new() -> Self {
        Self {
            active: false,
            key: 60,
            velocity: 0.0,
            modes: [Mode::default(); 8],
            fade: Fade::new(),
            age: 0,
            pulse_len: 1,
            noise: Noise::new(1),
            noise_env: 0.0,
            noise_coef: 0.0,
            released: false,
            release_at: 0,
            gain: 0.0,
            max_age: 0,
        }
    }
    fn derive(&mut self, p: &[f32; 17], sr: f32, expr: &Expression) {
        let f = key_to_hz(60.0 + p[12] + p[13] * (self.key as f32 - 60.0)) * expr.bend_ratio();
        let tension =
            (p[9] * self.velocity * (-(self.age as f32) / (0.04 * sr)).exp() / 12.0).exp2();
        let ratios = RATIOS[p[4].round() as usize];
        let mut longest = 0.0f32;
        for (i, m) in self.modes.iter_mut().enumerate() {
            let ratio = 1.0 + (ratios[i] - 1.0) * (1.0 + 0.5 * p[8]);
            let hz = f * ratio * tension;
            let decay = p[6] / (1.0 + (ratio - 1.0).max(0.0) * (1.0 - p[5]) * 2.5);
            let decay = if self.released && p[10] >= 0.5 {
                decay.min(p[11])
            } else {
                decay
            };
            longest = longest.max(decay);
            let r = (-6.907755 / (decay * sr)).exp();
            let w = std::f32::consts::TAU * hz.min(sr * 0.45) / sr;
            m.a = 2.0 * r * w.cos();
            m.b = r * r;
            m.input = w.sin();
            m.weight = if hz < sr * 0.45 {
                (0.15
                    + 0.85
                        * (std::f32::consts::PI * (i + 1) as f32 * (0.05 + 0.9 * p[7]))
                            .sin()
                            .abs())
                    / ((i + 1) as f32).sqrt()
            } else {
                0.0
            };
        }
        self.max_age = (longest * 1.7 * sr) as usize
            + self.pulse_len
            + if self.released && p[10] >= 0.5 {
                self.release_at
            } else {
                0
            };
        self.gain = 0.55 * vel_gain(self.velocity, p[15]) * db_to_gain(p[16]) * expr.gain(self.key);
    }
}
impl PoolVoice for Voice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.modes.iter().map(|m| m.y1.abs()).sum::<f32>() * self.gain
    }
    fn fade_out(&mut self, sr: f32) {
        self.fade.start(sr, STEAL_FADE_MS)
    }
    fn kill(&mut self) {
        self.active = false
    }
}
struct Mallet {
    sr: f32,
    p: [Ramp; 17],
    values: [f32; 17],
    pool: Pool<Voice>,
    expr: Expression,
    ctrl: usize,
    k: f32,
    hits: u32,
}
impl Mallet {
    fn new() -> Self {
        Self {
            sr: 48000.0,
            p: std::array::from_fn(|i| Ramp::new(PARAMS[i].default)),
            values: std::array::from_fn(|i| PARAMS[i].default),
            pool: Pool::new(18, 8, StealPolicy::Oldest, 48000.0, Voice::new),
            expr: Expression::new(),
            ctrl: 0,
            k: 0.1,
            hits: 0,
        }
    }
    fn control(&mut self) {
        for (r, v) in self.p.iter_mut().zip(&mut self.values) {
            *v = r.tick(self.k)
        }
        self.expr.tick();
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.derive(&self.values, self.sr, &self.expr)
        }
    }
}
impl Synth for Mallet {
    fn prepare(&mut self, sr: f32, _: usize) {
        self.expr.reset();
        self.sr = sr;
        self.k = ramp_coef(sr);
        self.ctrl = 0;
        self.hits = 0;
        self.pool = Pool::new(18, 8, StealPolicy::Oldest, sr, Voice::new)
    }
    fn reset(&mut self) {
        self.expr.reset();
        self.pool.kill_all();
        self.ctrl = 0;
        self.hits = 0
    }
    fn param_changed(&mut self, id: ParamId, v: f32, immediate: bool) {
        if let Some(i) = (id as usize).checked_sub(1).filter(|i| *i < 17) {
            let snap = immediate || PARAMS[i].is_choice();
            self.p[i].set(v, snap);
            if snap {
                self.values[i] = v;
            }
        }
    }
    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.pool.begin(key, id);
        let p = &self.values;
        self.hits = self.hits.wrapping_add(1);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.key = key;
        v.velocity = velocity;
        v.modes = [Mode::default(); 8];
        v.fade.reset();
        v.age = 0;
        v.released = false;
        v.release_at = 0;
        let hard = (p[0] + p[1] * (velocity - 0.7)).clamp(0.0, 1.0);
        v.pulse_len = ((0.006 + (0.00025 - 0.006) * hard) * self.sr).max(2.0) as usize;
        v.noise.reseed(self.hits);
        v.noise_env = p[2] * 0.035;
        v.noise_coef = (-6.907755 / (p[3] * self.sr)).exp();
        v.derive(p, self.sr, &self.expr);
    }
    fn note_off(&mut self, _: u8, _: f32, id: u32) {
        self.pool.release_id(id, |v| {
            v.released = true;
            v.release_at = v.age;
        })
    }
    fn all_notes_off(&mut self) {
        self.pool.release_all(|v| {
            v.released = true;
            v.release_at = v.age;
        })
    }
    fn all_sound_off(&mut self) {
        self.pool.kill_all()
    }
    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }
    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
    fn render(&mut self, _: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right) {
            if self.ctrl == 0 {
                self.control()
            }
            self.ctrl = (self.ctrl + 1) % CTRL;
            let (mut a, mut b) = (0.0, 0.0);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let pulse = if v.age < v.pulse_len {
                    (1.0 - fast_sin(v.age as f32 / v.pulse_len as f32 + 0.25)) / v.pulse_len as f32
                } else {
                    0.0
                };
                let x = pulse + v.noise.white() * v.noise_env;
                v.noise_env *= v.noise_coef;
                let (mut vl, mut vr) = (0.0, 0.0);
                for (i, m) in v.modes.iter_mut().enumerate() {
                    let y = m.tick(x);
                    let pan = if i % 2 == 0 { -0.6 } else { 0.6 } * self.values[14];
                    vl += y * (1.0 - pan) * 0.5;
                    vr += y * (1.0 + pan) * 0.5;
                }
                let gain = v.gain * v.fade.tick();
                a += vl * gain;
                b += vr * gain;
                v.age += 1;
                if v.age > v.max_age || v.fade.is_done() {
                    v.active = false;
                }
            }
            *l = a;
            *r = b;
        }
    }
}
pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Mallet::new()))
}
