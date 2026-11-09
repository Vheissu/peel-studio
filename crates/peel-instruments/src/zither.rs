//! Paired plucked and struck strings with coupling, buzz and body.
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

static PARAMS: [ParamDescriptor; 18] = [
    ParamDescriptor::choice(
        1,
        "exciter.type",
        "Exciter",
        "Exciter",
        &["Pluck", "Strike", "Brush"],
        0,
    ),
    ParamDescriptor::log(
        2,
        "exciter.tone",
        "Tone",
        "Exciter",
        300.0,
        16000.0,
        5000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        3,
        "exciter.position",
        "Position",
        "Exciter",
        0.02,
        0.5,
        0.15,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        4,
        "exciter.bow",
        "Bow",
        "Exciter",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        5,
        "string.decay",
        "Decay",
        "Strings",
        0.2,
        20.0,
        4.0,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        6,
        "string.damping",
        "Damping",
        "Strings",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::choice(
        7,
        "string.ratio",
        "String 2",
        "Strings",
        &[
            "Unison",
            "Octave",
            "Fifth",
            "Fourth",
            "Octave + Fifth",
            "Two Octaves",
            "Major Third",
            "Minor Third",
        ],
        0,
    ),
    ParamDescriptor::linear(
        8,
        "string.detune",
        "Detune",
        "Strings",
        0.0,
        50.0,
        3.0,
        Unit::Cents,
    ),
    ParamDescriptor::linear(
        9,
        "string.mix",
        "S2 Level",
        "Strings",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        10,
        "string.coupling",
        "Coupling",
        "Strings",
        0.0,
        1.0,
        0.15,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        11,
        "string.buzz",
        "Buzz",
        "Strings",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::choice(
        12,
        "body.type",
        "Body",
        "Body",
        &["None", "Box", "Harp", "Gourd"],
        2,
    ),
    ParamDescriptor::linear(
        13,
        "body.amount",
        "Amount",
        "Body",
        0.0,
        1.0,
        0.45,
        Unit::Percent,
    ),
    ParamDescriptor::toggle(14, "damp.enable", "Damper", "Damper", false),
    ParamDescriptor::log(
        15,
        "damp.release",
        "Release",
        "Damper",
        0.02,
        2.0,
        0.3,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        16,
        "out.width",
        "Width",
        "Output",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        17,
        "out.velocity",
        "Velocity",
        "Output",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        18,
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
    type_id: "peel.strings.zither",
    name: "Zither",
    description: "Paired plucked and struck strings with coupling, buzz and body",
    category: "Strings",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, Vx, ramp_coef,
    vel_gain,
};
use peel_dsp::delay::{AllpassTap, DelayLine};
use peel_dsp::filter::{Svf, tanh_approx};
use peel_dsp::noise::Noise;
use peel_dsp::util::{db_to_gain, key_to_hz};
use peel_dsp::voice::StealPolicy;

pub static PRESETS: [Preset; 8] = [
    Preset {
        name: "Harp",
        params: &[],
    },
    Preset {
        name: "Koto",
        params: &[
            ("exciter.tone", 7000.0),
            ("exciter.position", 0.08),
            ("string.decay", 2.5),
            ("string.damping", 0.25),
            ("string.detune", 0.0),
            ("string.mix", 0.0),
            ("body.type", 1.0),
            ("body.amount", 0.6),
            ("string.buzz", 0.05),
        ],
    },
    Preset {
        name: "Dulcimer",
        params: &[
            ("exciter.type", 1.0),
            ("exciter.tone", 9000.0),
            ("exciter.position", 0.12),
            ("string.decay", 3.5),
            ("string.damping", 0.2),
            ("string.detune", 6.0),
            ("string.mix", 0.5),
            ("string.coupling", 0.3),
            ("body.type", 1.0),
            ("body.amount", 0.5),
            ("out.width", 0.7),
        ],
    },
    Preset {
        name: "Twelve String",
        params: &[
            ("string.ratio", 1.0),
            ("string.detune", 5.0),
            ("string.mix", 0.45),
            ("string.damping", 0.3),
            ("string.decay", 3.0),
            ("body.type", 1.0),
            ("body.amount", 0.4),
        ],
    },
    Preset {
        name: "Jawari Drone",
        params: &[
            ("string.ratio", 2.0),
            ("string.buzz", 0.6),
            ("string.decay", 8.0),
            ("string.damping", 0.15),
            ("exciter.bow", 0.25),
            ("body.type", 3.0),
            ("body.amount", 0.6),
            ("string.coupling", 0.35),
            ("out.width", 0.7),
        ],
    },
    Preset {
        name: "Music Box",
        params: &[
            ("exciter.type", 1.0),
            ("exciter.tone", 12000.0),
            ("exciter.position", 0.3),
            ("string.decay", 1.2),
            ("string.damping", 0.5),
            ("string.ratio", 5.0),
            ("string.mix", 0.3),
            ("body.type", 0.0),
        ],
    },
    Preset {
        name: "Bowed Glass",
        params: &[
            ("exciter.type", 2.0),
            ("exciter.bow", 0.7),
            ("exciter.tone", 3000.0),
            ("string.decay", 10.0),
            ("string.damping", 0.6),
            ("string.ratio", 4.0),
            ("string.mix", 0.35),
            ("string.coupling", 0.4),
            ("body.type", 0.0),
            ("out.width", 0.8),
        ],
    },
    Preset {
        name: "Muted Pluck",
        params: &[
            ("damp.enable", 1.0),
            ("damp.release", 0.08),
            ("string.decay", 0.6),
            ("string.damping", 0.7),
            ("body.type", 1.0),
            ("body.amount", 0.6),
        ],
    },
];
const RATIOS: [f32; 8] = [1.0, 2.0, 1.5, 4.0 / 3.0, 3.0, 4.0, 1.25, 1.2];
const BODIES: [[f32; 3]; 4] = [
    [110.0, 240.0, 1100.0],
    [110.0, 240.0, 1100.0],
    [180.0, 420.0, 2200.0],
    [140.0, 600.0, 1800.0],
];
struct StringLoop {
    line: DelayLine,
    tap: AllpassTap,
    exc: Vec<f32>,
    exc_len: usize,
    exc_pos: usize,
    delay: f32,
    coef: f32,
    feedback: f32,
    z: f32,
    dc_in: f32,
    dc_out: f32,
    dc_pole: f32,
    audible: bool,
}
impl StringLoop {
    fn new(sr: f32) -> Self {
        let n = (sr / 8.0).ceil() as usize + 8;
        Self {
            line: DelayLine::new(n),
            tap: AllpassTap::new(),
            exc: vec![0.0; n],
            exc_len: 0,
            exc_pos: 0,
            delay: 100.0,
            coef: 0.5,
            feedback: 0.9,
            z: 0.0,
            dc_in: 0.0,
            dc_out: 0.0,
            dc_pole: (-std::f32::consts::TAU * 5.0 / sr).exp(),
            audible: true,
        }
    }
    fn clear(&mut self) {
        self.line.clear();
        self.tap.reset();
        self.exc_len = 0;
        self.exc_pos = 0;
        self.z = 0.0;
        self.dc_in = 0.0;
        self.dc_out = 0.0;
    }
    fn tune(&mut self, freq: f32, decay: f32, damping: f32, sr: f32) {
        // Omit strings beyond the usable waveguide range instead of
        // sounding them at an unrelated clamped pitch.
        self.audible = freq >= 8.0 && freq < sr * 0.4;
        let f = freq.clamp(8.0, sr * 0.4);
        let w = std::f64::consts::TAU * f64::from(f) / f64::from(sr);
        let cutoff = (f * (4.0 + 60.0 * (1.0 - damping).powi(2))).min(sr * 0.4);
        self.coef = 1.0 - (-std::f32::consts::TAU * cutoff / sr).exp();
        let pole = f64::from(1.0 - self.coef);
        let phase_delay = (pole * w.sin()).atan2(1.0 - pole * w.cos()) / w;
        // The loop's DC blocker leads phase; compensate it as well as
        // the damping filter's lag, at the actual sample rate.
        let dc_pole = f64::from(self.dc_pole);
        let dc_lead =
            w.sin().atan2(1.0 - w.cos()) - (dc_pole * w.sin()).atan2(1.0 - dc_pole * w.cos());
        self.delay = (sr / f - phase_delay as f32 + (dc_lead / w) as f32)
            .clamp(2.0, (self.line.max_delay() - 2) as f32);
        self.feedback = (-6.907755 / (f * decay)).exp().min(0.99999);
    }
    fn excite(&mut self, kind: usize, tone: f32, position: f32, sr: f32, noise: &mut Noise) {
        let period = self.delay.round() as usize;
        let n = if kind == 2 {
            period.max((0.025 * sr) as usize)
        } else {
            period
        };
        self.exc_len = n.min(self.exc.len());
        self.exc_pos = 0;
        let c = 1.0 - (-std::f32::consts::TAU * tone.min(sr * 0.4) / sr).exp();
        let mut z = 0.0;
        let pulse = (sr / (tone * 0.15)).clamp(2.0, 0.006 * sr) as usize;
        for i in 0..self.exc_len {
            let x = if kind == 1 {
                if i < pulse {
                    0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / pulse as f32).cos()
                } else {
                    0.0
                }
            } else {
                noise.white()
            };
            z += c * (x - z);
            self.exc[i] = z;
        }
        {
            // Circular pick-position comb; cycle traversal preserves source samples.
            let offset = (position * period as f32).round() as usize;
            let n = self.exc_len;
            let d = offset.max(1).min(n - 1);
            let gcd = |mut a: usize, mut b: usize| {
                while b != 0 {
                    let r = a % b;
                    a = b;
                    b = r;
                }
                a
            };
            for start in 0..gcd(n, d) {
                let mut i = start;
                let first = self.exc[start];
                let mut previous = first;
                loop {
                    let next = (i + d) % n;
                    if next == start {
                        self.exc[next] = first - previous;
                        break;
                    }
                    let current = self.exc[next];
                    self.exc[next] = current - previous;
                    previous = current;
                    i = next;
                }
            }
        }
        let mean = self.exc[..self.exc_len].iter().sum::<f32>() / self.exc_len as f32;
        for (i, x) in self.exc[..self.exc_len].iter_mut().enumerate() {
            let attack = (i as f32 / (0.0005 * sr)).min(1.0);
            let end = if kind == 2 {
                1.0 - i as f32 / self.exc_len as f32
            } else {
                1.0
            };
            *x = (*x - mean) * 0.65 * attack * end;
        }
    }
    fn read(&mut self) -> f32 {
        let x = self.tap.read(&self.line, self.delay);
        self.z += self.coef * (x - self.z);
        if self.audible { self.z } else { 0.0 }
    }
    fn push(&mut self, x: f32, bow: f32, buzz: f32) {
        let excitation = if self.exc_pos < self.exc_len {
            let v = self.exc[self.exc_pos];
            self.exc_pos += 1;
            v
        } else {
            0.0
        };
        let v = x * self.feedback + excitation + bow;
        let v = v - buzz * 0.7 * v.max(0.0).powi(2) / (1.0 + v.abs());
        // Remove the bridge's DC asymmetry from the loop itself.
        let dc = v - self.dc_in + self.dc_pole * self.dc_out;
        self.dc_in = v;
        self.dc_out = dc;
        self.line.push(tanh_approx(dc));
    }
}
struct Voice {
    active: bool,
    key: u8,
    /// Runtime voice ID, for per-note expression.
    id: u32,
    velocity: f32,
    held: bool,
    strings: [StringLoop; 2],
    body: [Svf; 3],
    noise: Noise,
    noise_z: f32,
    fade: Fade,
    life: f32,
    life_coef: f32,
    gain: f32,
    bow_level: f32,
}
impl Voice {
    fn new(sr: f32) -> Self {
        Self {
            active: false,
            key: 60,
            id: 0,
            velocity: 0.0,
            held: false,
            strings: std::array::from_fn(|_| StringLoop::new(sr)),
            body: [Svf::new(); 3],
            noise: Noise::new(1),
            noise_z: 0.0,
            fade: Fade::new(),
            life: 0.0,
            life_coef: 0.0,
            gain: 0.0,
            bow_level: 0.0,
        }
    }
    fn derive(&mut self, p: &[f32; 18], sr: f32, x: Vx) {
        let f = key_to_hz(self.key as f32) * x.ratio;
        let decay = if !self.held && p[13] >= 0.5 {
            p[4].min(p[14])
        } else {
            p[4]
        };
        // Timbre opens or closes the string damping: brighter above 0.5.
        let damping = (p[5] - 0.4 * x.tone).clamp(0.0, 1.0);
        self.strings[0].tune(f, decay, damping, sr);
        self.strings[1].tune(
            f * RATIOS[p[6].round() as usize] * (p[7] / 1200.0).exp2(),
            decay,
            damping,
            sr,
        );
        self.life_coef = (-6.907755 / (decay * sr)).exp();
        self.bow_level = if self.held {
            p[3] * (1.0 + 0.5 * x.pressure) * self.velocity * 0.025
        } else {
            0.0
        };
        self.gain = 0.85 * vel_gain(self.velocity, p[16]) * db_to_gain(p[17]) * x.gain();
        for (f, hz) in self.body.iter_mut().zip(BODIES[p[11].round() as usize]) {
            f.set_q(hz, 2.5, sr);
        }
    }
}
impl PoolVoice for Voice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.life * self.gain
    }
    fn fade_out(&mut self, sr: f32) {
        self.fade.start(sr, STEAL_FADE_MS)
    }
    fn kill(&mut self) {
        self.active = false;
    }
}
struct Zither {
    sr: f32,
    p: [Ramp; 18],
    values: [f32; 18],
    pool: Pool<Voice>,
    expr: Expression,
    k: f32,
    ctrl: usize,
    hits: u32,
}
impl Zither {
    fn new() -> Self {
        Self {
            sr: 48000.0,
            p: std::array::from_fn(|i| Ramp::new(PARAMS[i].default)),
            values: std::array::from_fn(|i| PARAMS[i].default),
            pool: Pool::new(26, 12, StealPolicy::Oldest, 48000.0, || Voice::new(48000.0)),
            expr: Expression::new(),
            k: 0.1,
            ctrl: 0,
            hits: 0,
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
impl Synth for Zither {
    fn prepare(&mut self, sr: f32, _: usize) {
        self.expr.reset();
        self.sr = sr;
        self.k = ramp_coef(sr);
        self.ctrl = 0;
        self.hits = 0;
        self.pool = Pool::new(26, 12, StealPolicy::Oldest, sr, || Voice::new(sr));
    }
    fn reset(&mut self) {
        self.expr.reset();
        self.pool.kill_all();
        self.ctrl = 0;
        self.hits = 0;
    }
    fn param_changed(&mut self, id: ParamId, v: f32, immediate: bool) {
        if let Some(i) = (id as usize).checked_sub(1).filter(|i| *i < 18) {
            let snap = immediate || PARAMS[i].is_choice();
            self.p[i].set(v, snap);
            if snap {
                self.values[i] = v;
            }
        }
    }
    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let previous = self
            .pool
            .voices
            .iter()
            .position(|v| v.active && v.key == key && !v.fade.is_fading());
        let slot = self.pool.begin(key, id);
        let reuse = previous.filter(|old| *old != slot);
        if let Some(old) = reuse {
            self.pool.voices.swap(old, slot);
            self.pool.voices[old].kill();
            self.pool.alloc.free(old);
        }
        self.hits = self.hits.wrapping_add(1);
        let v = &mut self.pool.voices[slot];
        if reuse.is_none() {
            for string in &mut v.strings {
                string.clear();
            }
            for f in &mut v.body {
                f.reset();
            }
            v.noise_z = 0.0;
        }
        v.active = true;
        v.key = key;
        v.id = id;
        v.velocity = velocity;
        v.held = true;
        v.life = 1.0;
        v.fade.reset();
        v.noise.reseed(self.hits);
        v.derive(&self.values, self.sr, self.expr.voice(id, key));
        let tone = self.values[1] * (0.5 + velocity).min(2.0);
        for string in &mut v.strings {
            string.excite(
                self.values[0].round() as usize,
                tone,
                self.values[2],
                self.sr,
                &mut v.noise,
            );
        }
    }
    fn note_off(&mut self, _: u8, _: f32, id: u32) {
        self.pool.release_id(id, |v| v.held = false)
    }
    fn all_notes_off(&mut self) {
        self.pool.release_all(|v| v.held = false)
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
                let x = [v.strings[0].read(), v.strings[1].read()];
                let c = 0.5 * self.values[9];
                v.noise_z += 0.15 * (v.noise.white() - v.noise_z);
                let bow = v.noise_z * v.bow_level;
                // A convex energy exchange is passive even at full coupling.
                v.strings[0].push((1.0 - c) * x[0] + c * x[1], bow, self.values[10]);
                v.strings[1].push((1.0 - c) * x[1] + c * x[0], bow, self.values[10]);
                let s2 = self.values[8];
                let norm = 1.0 / (1.0 + s2);
                let dry = (x[0] + s2 * x[1]) * norm;
                let mut body = 0.0;
                for f in &mut v.body {
                    body += f.tick(dry).band_unity / 3.0;
                }
                let amount = if self.values[11].round() as usize == 0 {
                    0.0
                } else {
                    self.values[12]
                };
                let width = self.values[15];
                let mid = dry * (1.0 - amount) + body * amount;
                let side = (x[0] - s2 * x[1]) * norm * width * (1.0 - amount);
                let gain = v.gain * v.fade.tick();
                a += (mid + side) * gain;
                b += (mid - side) * gain;
                if v.bow_level == 0.0 {
                    v.life *= v.life_coef;
                }
                if v.life < 1e-5 || v.fade.is_done() {
                    v.active = false;
                }
            }
            *l = a;
            *r = b;
        }
    }
}
pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Zither::new()))
}
