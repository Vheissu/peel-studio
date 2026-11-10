//! Two oscillators with cross modulation, sync and ring.
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

static PARAMS: [ParamDescriptor; 19] = [
    ParamDescriptor::choice(
        1,
        "osc1.wave",
        "Osc 1",
        "Oscillators",
        &["Sine", "Triangle", "Saw", "Square"],
        2,
    ),
    ParamDescriptor::choice(
        2,
        "osc2.wave",
        "Osc 2",
        "Oscillators",
        &["Sine", "Triangle", "Saw", "Square"],
        2,
    ),
    ParamDescriptor::linear(
        3,
        "osc2.semi",
        "Osc 2 Semi",
        "Oscillators",
        -24.0,
        24.0,
        0.0,
        Unit::Semitones,
    ),
    ParamDescriptor::linear(
        4,
        "osc2.detune",
        "Detune",
        "Oscillators",
        -50.0,
        50.0,
        7.0,
        Unit::Cents,
    ),
    ParamDescriptor::linear(
        5,
        "osc.mix",
        "Mix",
        "Oscillators",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::choice(
        6,
        "cross.mode",
        "Mode",
        "Cross",
        &["Free", "Sync", "Ring"],
        0,
    ),
    ParamDescriptor::power(
        7,
        "cross.fm12",
        "1 > 2",
        "Cross",
        0.0,
        1.0,
        0.15,
        Unit::Percent,
        2.0,
    ),
    ParamDescriptor::power(
        8,
        "cross.fm21",
        "2 > 1",
        "Cross",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
        2.0,
    ),
    ParamDescriptor::linear(
        9,
        "mod.amount",
        "FM Env",
        "Cross",
        -1.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        10,
        "mod.decay",
        "Env Decay",
        "Cross",
        0.01,
        5.0,
        0.4,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        11,
        "filter.cutoff",
        "Cutoff",
        "Filter",
        40.0,
        18000.0,
        2500.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        12,
        "filter.resonance",
        "Resonance",
        "Filter",
        0.0,
        1.0,
        0.2,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        13,
        "filter.env",
        "Filter Env",
        "Filter",
        -4.0,
        4.0,
        2.0,
        Unit::Octaves,
    ),
    ParamDescriptor::log(
        14,
        "amp.attack",
        "Attack",
        "Amp",
        0.001,
        5.0,
        0.005,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        15,
        "amp.decay",
        "Decay",
        "Amp",
        0.01,
        8.0,
        0.6,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        16,
        "amp.sustain",
        "Sustain",
        "Amp",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        17,
        "amp.release",
        "Release",
        "Amp",
        0.005,
        8.0,
        0.3,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        18,
        "out.velocity",
        "Velocity",
        "Output",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        19,
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
    type_id: "peel.synth.cross",
    name: "Cross",
    description: "Two oscillators with cross modulation, sync and ring",
    category: "Synth",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

use crate::analog::Down2;
use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, Vx, ramp_coef,
    vel_gain,
};
use peel_dsp::envelope::{Adsr, Decay};
use peel_dsp::filter::Ladder;
use peel_dsp::osc::{self, Waveform};
use peel_dsp::util::{db_to_gain, key_to_hz};
use peel_dsp::voice::StealPolicy;

pub static PRESETS: [Preset; 9] = [
    Preset {
        name: "Duo",
        params: &[],
    },
    Preset {
        name: "Sync Lead",
        params: &[
            ("cross.mode", 1.0),
            ("osc2.semi", 19.0),
            ("cross.fm12", 0.2),
            ("mod.amount", 0.5),
            ("mod.decay", 0.25),
            ("filter.cutoff", 6000.0),
            ("filter.resonance", 0.15),
            ("filter.env", 1.5),
            ("osc.mix", 0.85),
            ("amp.sustain", 0.8),
        ],
    },
    Preset {
        name: "FM Bell",
        params: &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 0.0),
            ("osc2.semi", 22.0),
            ("osc2.detune", -31.0),
            ("cross.fm12", 0.0),
            ("cross.fm21", 0.45),
            ("osc.mix", 0.0),
            ("mod.amount", 0.6),
            ("mod.decay", 1.2),
            ("filter.cutoff", 16000.0),
            ("filter.env", 0.0),
            ("amp.attack", 0.001),
            ("amp.decay", 3.0),
            ("amp.sustain", 0.0),
            ("amp.release", 2.0),
        ],
    },
    Preset {
        name: "Cross Bass",
        params: &[
            ("osc1.wave", 3.0),
            ("osc2.wave", 0.0),
            ("osc2.semi", -12.0),
            ("osc2.detune", 0.0),
            ("cross.fm12", 0.0),
            ("cross.fm21", 0.3),
            ("osc.mix", 0.3),
            ("mod.amount", 0.5),
            ("mod.decay", 0.15),
            ("filter.cutoff", 900.0),
            ("filter.resonance", 0.3),
            ("filter.env", 2.5),
            ("amp.attack", 0.002),
            ("amp.decay", 0.4),
            ("amp.sustain", 0.6),
            ("amp.release", 0.1),
        ],
    },
    Preset {
        name: "Metal Pluck",
        params: &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 3.0),
            ("osc2.semi", 13.0),
            ("osc2.detune", 0.0),
            ("cross.fm12", 0.5),
            ("cross.fm21", 0.2),
            ("mod.amount", 0.8),
            ("mod.decay", 0.12),
            ("filter.cutoff", 8000.0),
            ("filter.env", 1.0),
            ("amp.attack", 0.001),
            ("amp.decay", 0.5),
            ("amp.sustain", 0.0),
        ],
    },
    Preset {
        name: "Ring Pad",
        params: &[
            ("cross.mode", 2.0),
            ("osc1.wave", 1.0),
            ("osc2.wave", 0.0),
            ("osc2.semi", 7.0),
            ("osc2.detune", 3.0),
            ("osc.mix", 0.6),
            ("cross.fm12", 0.0),
            ("mod.amount", 0.0),
            ("filter.cutoff", 5000.0),
            ("filter.env", 0.0),
            ("amp.attack", 0.4),
            ("amp.decay", 2.0),
            ("amp.sustain", 0.6),
            ("amp.release", 1.5),
        ],
    },
    Preset {
        name: "Growl",
        params: &[
            ("osc2.semi", -12.0),
            ("osc2.detune", 12.0),
            ("cross.fm12", 0.4),
            ("cross.fm21", 0.4),
            ("filter.cutoff", 1500.0),
            ("filter.resonance", 0.45),
            ("mod.amount", 0.3),
            ("mod.decay", 0.5),
        ],
    },
    Preset {
        name: "FM Keys",
        params: &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 0.0),
            ("osc2.semi", 0.0),
            ("osc2.detune", 3.0),
            ("osc.mix", 0.3),
            ("cross.mode", 0.0),
            ("cross.fm12", 0.35),
            ("cross.fm21", 0.0),
            ("mod.amount", 0.6),
            ("mod.decay", 0.5),
            ("filter.cutoff", 8_000.0),
            ("filter.resonance", 0.05),
            ("filter.env", 0.5),
            ("amp.attack", 0.002),
            ("amp.decay", 1.8),
            ("amp.sustain", 0.2),
            ("amp.release", 0.35),
        ],
    },
    Preset {
        name: "Night Bell",
        params: &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 0.0),
            ("osc2.semi", 19.0),
            ("osc2.detune", 4.0),
            ("osc.mix", 0.4),
            ("cross.mode", 0.0),
            ("cross.fm12", 0.45),
            ("cross.fm21", 0.0),
            ("mod.amount", 0.5),
            ("mod.decay", 1.2),
            ("filter.cutoff", 5_000.0),
            ("filter.resonance", 0.1),
            ("filter.env", 1.0),
            ("amp.attack", 0.001),
            ("amp.decay", 2.2),
            ("amp.sustain", 0.0),
            ("amp.release", 1.6),
            ("out.level", -2.0),
        ],
    },
];

struct Voice {
    active: bool,
    key: u8,
    /// Runtime voice ID, for per-note expression.
    id: u32,
    velocity: f32,
    phase: [f32; 2],
    last2: f32,
    sync_jump: f32,
    amp: Adsr,
    mod_env: Decay,
    filter: Ladder,
    down: Down2,
    fade: Fade,
    dt: [f32; 2],
    gain: f32,
}
impl Voice {
    fn new() -> Self {
        Self {
            active: false,
            key: 60,
            id: 0,
            velocity: 0.0,
            phase: [0.0; 2],
            last2: 0.0,
            sync_jump: 0.0,
            amp: Adsr::new(48000.0),
            mod_env: Decay::new(),
            filter: Ladder::new(),
            down: Down2::new(),
            fade: Fade::new(),
            dt: [0.0; 2],
            gain: 0.0,
        }
    }
    fn derive(&mut self, p: &[f32; 19], sr: f32, x: Vx) {
        let f = key_to_hz(self.key as f32) * x.ratio;
        self.dt = [
            (f / (2.0 * sr)).min(0.45),
            (f * (p[2] / 12.0 + p[3] / 1200.0).exp2() / (2.0 * sr)).min(0.45),
        ];
        let cutoff = p[10]
            * (0.5 * (self.key as f32 - 60.0) / 12.0
                + p[12] * self.mod_env.level()
                + x.cutoff_oct())
            .exp2();
        self.filter.set(cutoff.min(0.45 * sr), p[11], 1.0, sr);
        self.amp.set(p[13], p[14], p[15], p[16], false);
        self.mod_env.set_t60(p[9], sr);
        self.gain = 0.3 * db_to_gain(p[18]) * vel_gain(self.velocity, p[17]) * x.gain();
    }
    fn pair(&mut self, p: &[f32; 19], env: f32) -> f32 {
        let mod_scale =
            (1.0 + p[8] * env * (0.5 + 0.5 * self.velocity)).max(0.0) * 8.0 / std::f32::consts::TAU;
        let phase1 = (self.phase[0] + p[7] * mod_scale * self.last2).rem_euclid(1.0);
        let a = osc::eval(
            Waveform::from_index(p[0].round() as usize),
            phase1,
            self.dt[0],
            0.5,
        );
        let phase2 = (self.phase[1] + p[6] * mod_scale * a).rem_euclid(1.0);
        let mut b = osc::eval(
            Waveform::from_index(p[1].round() as usize),
            phase2,
            self.dt[1],
            0.5,
        );
        let mode = p[5].round() as usize;
        if mode == 1 {
            // PolyBLEP at the master's wrap compensates the slave's reset step.
            b += 0.5 * self.sync_jump * osc::poly_blep(self.phase[0], self.dt[0]);
        }
        self.last2 = b;
        self.phase[0] += self.dt[0];
        self.phase[1] = (self.phase[1] + self.dt[1]).fract();
        if self.phase[0] >= 1.0 {
            self.phase[0] -= 1.0;
            if mode == 1 {
                let before = osc::eval(
                    Waveform::from_index(p[1].round() as usize),
                    self.phase[1],
                    self.dt[1],
                    0.5,
                );
                let after = osc::eval(
                    Waveform::from_index(p[1].round() as usize),
                    0.0,
                    self.dt[1],
                    0.5,
                );
                self.sync_jump = after - before;
                self.phase[1] = self.phase[0] / self.dt[0].max(1e-8) * self.dt[1];
            }
        }
        if mode == 2 {
            b *= a;
        }
        a + (b - a) * p[4]
    }
}
impl PoolVoice for Voice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.amp.level() * self.gain
    }
    fn fade_out(&mut self, sr: f32) {
        self.fade.start(sr, STEAL_FADE_MS)
    }
    fn kill(&mut self) {
        self.active = false;
        self.amp.reset()
    }
}
struct Cross {
    sr: f32,
    p: [Ramp; 19],
    values: [f32; 19],
    pool: Pool<Voice>,
    expr: Expression,
    k: f32,
    ctrl: usize,
}
impl Cross {
    fn new() -> Self {
        Self {
            sr: 48000.0,
            p: std::array::from_fn(|i| Ramp::new(PARAMS[i].default)),
            values: std::array::from_fn(|i| PARAMS[i].default),
            pool: Pool::new(18, 8, StealPolicy::Oldest, 48000.0, Voice::new),
            expr: Expression::new(),
            k: 0.1,
            ctrl: 0,
        }
    }
    fn control(&mut self) {
        for (r, v) in self.p.iter_mut().zip(&mut self.values) {
            *v = r.tick(self.k)
        }
        self.expr.tick();
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.derive(&self.values, self.sr, self.expr.voice(v.id, v.key))
        }
    }
}
impl Synth for Cross {
    fn prepare(&mut self, sr: f32, _: usize) {
        self.expr.reset();
        self.sr = sr;
        self.k = ramp_coef(sr);
        self.ctrl = 0;
        self.pool = Pool::new(18, 8, StealPolicy::Oldest, sr, Voice::new);
        for v in &mut self.pool.voices {
            v.amp.set_sample_rate(sr);
        }
    }
    fn reset(&mut self) {
        self.expr.reset();
        self.pool.kill_all();
        self.ctrl = 0;
    }
    fn param_changed(&mut self, id: ParamId, v: f32, immediate: bool) {
        if let Some(i) = (id as usize).checked_sub(1).filter(|i| *i < 19) {
            let snap = immediate || PARAMS[i].is_choice();
            self.p[i].set(v, snap);
            if snap {
                self.values[i] = v;
            }
        }
    }
    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.pool.begin(key, id);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.key = key;
        v.id = id;
        v.velocity = velocity;
        v.phase = [0.0, 0.17];
        v.last2 = 0.0;
        v.sync_jump = 0.0;
        v.fade.reset();
        v.filter.reset();
        v.down.reset();
        v.amp.reset();
        v.mod_env.set_t60(self.values[9], self.sr);
        v.mod_env.trigger(1.0, 0.001, self.sr);
        v.derive(&self.values, self.sr, self.expr.voice(id, key));
        v.amp.note_on();
    }
    fn note_off(&mut self, _: u8, _: f32, id: u32) {
        self.pool.release_id(id, |v| v.amp.note_off())
    }
    fn all_notes_off(&mut self) {
        self.pool.release_all(|v| v.amp.note_off())
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
            let mut out = 0.0;
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let env = v.mod_env.tick();
                let a = v.pair(&self.values, env);
                let b = v.pair(&self.values, env);
                let x = v.down.tick(a, b);
                out += v.filter.tick(x) * v.amp.tick() * v.fade.tick() * v.gain;
                if v.amp.is_idle() || v.fade.is_done() {
                    v.active = false;
                }
            }
            *l = out;
            *r = out;
        }
    }
}
pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Cross::new()))
}
