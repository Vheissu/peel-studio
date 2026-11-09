//! Linked stereo attack and sustain shaping from envelope differences.
use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{Smoothed, db_to_gain};
use peel_dsp::{DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

pub const ID_ATTACK: ParamId = 1;
pub const ID_SUSTAIN: ParamId = 2;
pub const ID_ATTACK_TIME: ParamId = 3;
pub const ID_SUSTAIN_TIME: ParamId = 4;
pub const ID_CLIP: ParamId = 5;
pub const ID_OUTPUT: ParamId = 6;
pub const ID_MIX: ParamId = 7;
static PARAMS: [ParamDescriptor; 7] = [
    ParamDescriptor::linear(
        1,
        "attack",
        "Attack",
        "Shape",
        -1.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        2,
        "sustain",
        "Sustain",
        "Shape",
        -1.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        3,
        "attack_time",
        "Attack Time",
        "Timing",
        1.0,
        50.0,
        12.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::log(
        4,
        "sustain_time",
        "Sustain Time",
        "Timing",
        20.0,
        500.0,
        150.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::toggle(5, "clip", "Clip", "Output", false),
    ParamDescriptor::linear(
        6,
        "output",
        "Output",
        "Output",
        -12.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(7, "mix", "Mix", "Output", 0.0, 1.0, 1.0, Unit::Percent),
];
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.transient",
    name: "Transient",
    description: "Shape attack and sustain without a threshold",
    category: "Dynamics",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};
pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Punch",
        params: &[],
    },
    Preset {
        name: "Snap",
        params: &[("attack", 0.7), ("attack_time", 5.0)],
    },
    Preset {
        name: "Tighten",
        params: &[("attack", 0.0), ("sustain", -0.6), ("sustain_time", 120.0)],
    },
    Preset {
        name: "Room Up",
        params: &[("attack", -0.1), ("sustain", 0.6), ("sustain_time", 250.0)],
    },
    Preset {
        name: "Soften",
        params: &[("attack", -0.6), ("attack_time", 20.0)],
    },
    Preset {
        name: "Smack",
        params: &[
            ("attack", 0.9),
            ("sustain", -0.3),
            ("clip", 1.0),
            ("output", -2.0),
        ],
    },
];

pub struct Transient {
    store: ParamStore,
    sr: f32,
    p: [Smoothed; 7],
    fast: f32,
    slow: f32,
    floor: f32,
    gain_db: f32,
    fast_attack: f32,
    fast_release: f32,
    slow_attack: f32,
    slow_release: f32,
    gain_smooth: f32,
    control: u8,
    silence: SilenceTracker,
}
impl Default for Transient {
    fn default() -> Self {
        Self::new()
    }
}
impl Transient {
    pub fn new() -> Self {
        let mut s = Self {
            store: ParamStore::new(&DESCRIPTOR),
            sr: 48_000.0,
            p: std::array::from_fn(|i| Smoothed::new(PARAMS[i].default)),
            fast: 0.0,
            slow: 0.0,
            floor: 0.0,
            gain_db: 0.0,
            fast_attack: 0.0,
            fast_release: 0.0,
            slow_attack: 0.0,
            slow_release: 0.0,
            gain_smooth: 0.0,
            control: 0,
            silence: SilenceTracker::default(),
        };
        s.setup(48_000.0, 512);
        s
    }
    /// Signed envelope gain, excluding output trim and the parallel mix.
    pub fn gain_change_db(&self) -> f32 {
        self.gain_db
    }
    fn coefficients(&mut self, attack_ms: f32, sustain_ms: f32) {
        self.fast_attack = (-1.0 / (0.0001 * self.sr)).exp();
        self.fast_release = (-1.0 / (attack_ms * 0.001 * self.sr)).exp();
        self.slow_attack = self.fast_release;
        self.slow_release = (-1.0 / (sustain_ms * 0.001 * self.sr)).exp();
    }
}
impl Fx for Transient {
    fn store(&self) -> &ParamStore {
        &self.store
    }
    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }
    fn setup(&mut self, sr: f32, _max: usize) {
        self.sr = sr.max(8_000.0);
        for p in &mut self.p {
            p.set_ramp(self.sr, RAMP_SECONDS);
        }
        refresh_all(self);
        for p in &mut self.p {
            p.set_immediate(p.target());
        }
        self.gain_smooth = 1.0 - (-1.0 / (0.001 * self.sr)).exp();
        self.coefficients(self.p[2].value(), self.p[3].value());
        self.clear();
    }
    fn param_changed(&mut self, id: ParamId, v: f32) {
        if let Some(p) = self.p.get_mut(id.wrapping_sub(1) as usize) {
            p.set_target(v);
        }
    }
    fn clear(&mut self) {
        self.fast = 0.0;
        self.slow = 0.0;
        self.floor = 0.0;
        self.gain_db = 0.0;
        self.control = 0;
        self.silence.reset();
    }
    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let p = self.p.each_mut().map(Smoothed::next);
            if self.control == 0 {
                self.coefficients(p[2], p[3]);
            }
            self.control = (self.control + 1) % 8;
            let peak = l.abs().max(r.abs());
            let a = if peak > self.fast {
                self.fast_attack
            } else {
                self.fast_release
            };
            self.fast = peak + (self.fast - peak) * a;
            let a = if peak > self.slow {
                self.slow_attack
            } else {
                self.slow_release
            };
            self.slow = peak + (self.slow - peak) * a;
            self.floor = self.slow + (self.floor - self.slow) * self.slow_release;
            let attack = if self.fast > 1e-12 {
                (20.0 * (self.fast / self.slow.max(1e-12)).log10() / 24.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let sustain = if self.slow > 1e-12 {
                // The floor identifies the early body. After it catches up,
                // the slow/fast gap keeps shaping the late decaying room tail.
                let body = (20.0 * (self.slow / self.floor.max(1e-12)).log10() / 24.0)
                    .clamp(0.0, 1.0)
                    * (1.0 - attack);
                let tail =
                    (20.0 * (self.slow / self.fast.max(1e-12)).log10() / 24.0).clamp(0.0, 1.0);
                body.max(tail)
            } else {
                0.0
            };
            let target = (12.0 * (p[0] * attack + p[1] * sustain)).clamp(-12.0, 12.0);
            self.gain_db += (target - self.gain_db) * self.gain_smooth;
            let gain = db_to_gain(self.gain_db);
            let out = db_to_gain(p[5]);
            let dry = [*l, *r];
            let wet = dry.map(|x| {
                let y = x * gain;
                y + (y.tanh() - y) * p[4]
            });
            *l = (dry[0] + (wet[0] - dry[0]) * p[6]) * out;
            *r = (dry[1] + (wet[1] - dry[1]) * p[6]) * out;
        }
    }
    fn silent(&self) -> bool {
        self.silence.quiet_for((0.1 * self.sr) as u32)
    }
}
impl_device!(Transient, DESCRIPTOR);
