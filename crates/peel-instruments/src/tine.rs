//! `peel.keys.tine`: an FM electric piano built from two operator pairs.
//!
//! The *tine* pair is a sine carrier phase-modulated at a ratio of 14, with a
//! fast-decaying index that produces the bell-like attack. The *bar* pair is
//! a slightly detuned carrier modulated at 1:1 whose index follows the note's
//! amplitude, giving the warm growl that fades as the note dies. Velocity
//! controls brightness, high notes get less modulation, and a tremolo or
//! auto-pan sweeps the stereo image.

use peel_dsp::envelope::{Adsr, Stage};
use peel_dsp::osc::{Lfo, LfoShape, fast_sin};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz, pan_gains};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, lerp, ramp_coef, vel_gain,
};

const TINE: ParamId = 1;
const BAR: ParamId = 2;
const BELL: ParamId = 3;
const GROWL: ParamId = 4;
const BELL_DECAY: ParamId = 5;
const VELOCITY: ParamId = 6;
const ATTACK: ParamId = 10;
const DECAY: ParamId = 11;
const RELEASE: ParamId = 12;
const TREM_RATE: ParamId = 20;
const TREM_DEPTH: ParamId = 21;
const TREM_MODE: ParamId = 22;
const WIDTH: ParamId = 30;
const LEVEL: ParamId = 31;

static PARAMS: [ParamDescriptor; 14] = [
    ParamDescriptor::linear(
        TINE,
        "tone.tine",
        "Tine",
        "Tone",
        0.0,
        1.0,
        0.6,
        Unit::Percent,
    ),
    ParamDescriptor::linear(BAR, "tone.bar", "Bar", "Tone", 0.0, 1.0, 0.6, Unit::Percent),
    ParamDescriptor::linear(
        BELL,
        "tone.bell",
        "Bell",
        "Tone",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        GROWL,
        "tone.growl",
        "Growl",
        "Tone",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        BELL_DECAY,
        "tone.bell_decay",
        "Bell Decay",
        "Tone",
        0.02,
        1.5,
        0.18,
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
        8.0,
        3.5,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        RELEASE,
        "amp.release",
        "Release",
        "Envelope",
        0.03,
        2.0,
        0.28,
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
        0.25,
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
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.keys.tine",
    name: "Tine",
    description: "FM electric piano with bell attack, tremolo and auto-pan",
    category: "Keys",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Classic",
        params: &[
            ("tone.tine", 0.6),
            ("tone.bar", 0.6),
            ("tone.bell", 0.35),
            ("tone.growl", 0.35),
            ("tone.bell_decay", 0.18),
            ("tone.velocity", 0.7),
            ("amp.attack", 0.002),
            ("amp.decay", 3.5),
            ("amp.release", 0.28),
            ("trem.rate", 5.2),
            ("trem.depth", 0.25),
            ("trem.mode", 1.0),
            ("out.width", 0.5),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Mellow",
        params: &[
            ("tone.tine", 0.4),
            ("tone.bar", 0.8),
            ("tone.bell", 0.2),
            ("tone.growl", 0.2),
            ("tone.bell_decay", 0.12),
            ("amp.decay", 4.5),
            ("amp.release", 0.35),
            ("trem.depth", 0.0),
        ],
    },
    Preset {
        name: "Bright Tine",
        params: &[
            ("tone.tine", 0.85),
            ("tone.bar", 0.4),
            ("tone.bell", 0.8),
            ("tone.growl", 0.3),
            ("tone.bell_decay", 0.3),
            ("tone.velocity", 0.9),
            ("amp.decay", 3.0),
            ("trem.depth", 0.1),
        ],
    },
    Preset {
        name: "Suitcase",
        params: &[
            ("tone.bell", 0.5),
            ("tone.growl", 0.5),
            ("trem.rate", 4.6),
            ("trem.depth", 0.65),
            ("trem.mode", 1.0),
            ("out.width", 0.8),
        ],
    },
    Preset {
        name: "Wurly Bark",
        params: &[
            ("tone.tine", 0.3),
            ("tone.bar", 0.9),
            ("tone.bell", 0.25),
            ("tone.growl", 0.85),
            ("tone.bell_decay", 0.08),
            ("tone.velocity", 0.85),
            ("amp.decay", 1.8),
            ("amp.release", 0.15),
            ("trem.rate", 5.8),
            ("trem.depth", 0.4),
            ("trem.mode", 0.0),
        ],
    },
    Preset {
        name: "Dream",
        params: &[
            ("tone.tine", 0.5),
            ("tone.bar", 0.7),
            ("tone.bell", 0.35),
            ("tone.growl", 0.25),
            ("tone.bell_decay", 0.5),
            ("amp.attack", 0.02),
            ("amp.decay", 7.0),
            ("amp.release", 1.1),
            ("trem.rate", 3.2),
            ("trem.depth", 0.35),
            ("out.width", 1.0),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Tine::new()))
}

/// Modulator-to-carrier ratio of the tine pair.
const TINE_RATIO: f32 = 14.0;

#[derive(Clone, Copy)]
struct TineVoice {
    active: bool,
    ph_c1: f32,
    ph_m1: f32,
    ph_c2: f32,
    ph_m2: f32,
    inc: f32,
    inc2: f32,
    amp: Adsr,
    bell_env: f32,
    bell_coef: f32,
    bell_index: f32,
    growl_index: f32,
    tine_l: f32,
    tine_r: f32,
    bar_l: f32,
    bar_r: f32,
    fade: Fade,
}

impl TineVoice {
    fn new(sr: f32) -> TineVoice {
        TineVoice {
            active: false,
            ph_c1: 0.0,
            ph_m1: 0.0,
            ph_c2: 0.0,
            ph_m2: 0.0,
            inc: 0.0,
            inc2: 0.0,
            amp: Adsr::new(sr),
            bell_env: 0.0,
            bell_coef: 0.0,
            bell_index: 0.0,
            growl_index: 0.0,
            tine_l: 0.0,
            tine_r: 0.0,
            bar_l: 0.0,
            bar_r: 0.0,
            fade: Fade::new(),
        }
    }

    #[inline]
    fn tick(&mut self) -> (f32, f32) {
        let a = self.amp.tick();
        // Tine pair: bell index decays quickly.
        let bell_i = self.bell_index * self.bell_env;
        self.bell_env *= self.bell_coef;
        let m1 = fast_sin(self.ph_m1);
        let c1 = fast_sin(self.ph_c1 + bell_i * m1);
        // Bar pair: growl follows the amplitude.
        let growl_i = self.growl_index * (0.25 + 0.75 * a);
        let m2 = fast_sin(self.ph_m2);
        let c2 = fast_sin(self.ph_c2 + growl_i * m2);

        let x = self.ph_c1 + self.inc;
        self.ph_c1 = x - x.floor();
        let x = self.ph_m1 + self.inc * TINE_RATIO;
        self.ph_m1 = x - x.floor();
        let x = self.ph_c2 + self.inc2;
        self.ph_c2 = x - x.floor();
        let x = self.ph_m2 + self.inc2;
        self.ph_m2 = x - x.floor();

        let g = a * self.fade.tick();
        // A note whose envelope has decayed to nothing is finished even if
        // the key is still down.
        let dead = self.amp.stage() == Stage::Sustain && self.amp.level() < 1.0e-4;
        if self.amp.is_idle() || self.fade.is_done() || dead {
            self.active = false;
        }
        (
            (c1 * self.tine_l + c2 * self.bar_l) * g,
            (c1 * self.tine_r + c2 * self.bar_r) * g,
        )
    }
}

impl PoolVoice for TineVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.amp.level()
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
        self.amp.reset();
    }
}

pub(crate) struct Tine {
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    pool: Pool<TineVoice>,
    tine: f32,
    bar: f32,
    bell: f32,
    growl: f32,
    bell_decay: f32,
    vel_sens: f32,
    attack: f32,
    decay: f32,
    release: f32,
    mode: usize,
    width: f32,
    trem_rate: Ramp,
    trem_depth: Ramp,
    level: Ramp,
    lfo: Lfo,
    lfo_dt: f32,
    gain_l: f32,
    gain_r: f32,
    out_gain: f32,
}

impl Tine {
    pub(crate) fn new() -> Tine {
        Tine {
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            pool: Pool::new(20, 16, StealPolicy::Quietest, 48_000.0, || {
                TineVoice::new(48_000.0)
            }),
            tine: 0.6,
            bar: 0.6,
            bell: 0.45,
            growl: 0.35,
            bell_decay: 0.18,
            vel_sens: 0.7,
            attack: 0.002,
            decay: 3.5,
            release: 0.28,
            mode: 1,
            width: 0.5,
            trem_rate: Ramp::new(5.2f32.ln()),
            trem_depth: Ramp::new(0.25),
            level: Ramp::new(0.0),
            lfo: Lfo::new(0x7117),
            lfo_dt: 0.0,
            gain_l: 1.0,
            gain_r: 1.0,
            out_gain: 1.0,
        }
    }

    fn derive(&mut self) {
        self.lfo_dt = self.trem_rate.get().exp() / self.sr;
        self.out_gain = db_to_gain(self.level.get()) * 0.85;
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.trem_rate.tick(k);
        self.trem_depth.tick(k);
        self.level.tick(k);
        self.derive();
        self.pool.sync();
    }
}

impl Synth for Tine {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(20, 16, StealPolicy::Quietest, sample_rate, || {
            TineVoice::new(sample_rate)
        });
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
        self.lfo.reset(0.0);
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            TINE => self.tine = v,
            BAR => self.bar = v,
            BELL => self.bell = v,
            GROWL => self.growl = v,
            BELL_DECAY => self.bell_decay = v,
            VELOCITY => self.vel_sens = v,
            ATTACK => self.attack = v,
            DECAY => self.decay = v,
            RELEASE => {
                self.release = v;
                // Release applies to notes already sounding.
                for voice in &mut self.pool.voices {
                    voice.amp.set_release(v);
                }
            }
            TREM_RATE => self.trem_rate.set(v.ln(), imm),
            TREM_DEPTH => self.trem_depth.set(v, imm),
            TREM_MODE => self.mode = v as usize,
            WIDTH => self.width = v,
            LEVEL => self.level.set(v, imm),
            _ => {}
        }
        if imm {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.pool.begin(key, id);
        let sr = self.sr;
        let hz = key_to_hz(key as f32);
        let inc = hz / sr;
        // Brightness: velocity opens the modulation, high notes close it.
        let bright = lerp(1.0, velocity.powf(1.3), self.vel_sens);
        let kscale = (1.25 - (key as f32 - 36.0) / 72.0).clamp(0.15, 1.3);
        // The 14x modulator would alias on very high notes; fade it out.
        let mod_hz = hz * TINE_RATIO;
        let guard = ((0.3 * sr - mod_hz) / (0.15 * sr)).clamp(0.0, 1.0);
        let key_pan = ((key as f32 - 60.0) / 48.0).clamp(-1.0, 1.0) * 0.3;
        let (tl, tr) = pan_gains(((-0.25 + key_pan) * self.width).clamp(-1.0, 1.0));
        let (bl, br) = pan_gains(((0.25 + key_pan) * self.width).clamp(-1.0, 1.0));
        let level = vel_gain(velocity, 0.3 + 0.5 * self.vel_sens);
        let (a, d, r) = (
            self.attack,
            self.decay * (1.0 - (key as f32 - 60.0) / 150.0).clamp(0.3, 1.6),
            self.release,
        );
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.ph_c1 = 0.0;
        v.ph_m1 = 0.0;
        v.ph_c2 = 0.0;
        v.ph_m2 = 0.0;
        v.inc = inc;
        v.inc2 = inc * 1.0016;
        v.amp.reset();
        v.amp.set(a, d, 0.0, r, true);
        v.amp.note_on();
        v.bell_env = 1.0;
        v.bell_coef = (-1.0 / (self.bell_decay * kscale.max(0.5) * sr)).exp();
        v.bell_index = self.bell * 0.9 * bright * kscale * guard;
        v.growl_index = self.growl * 0.45 * bright * kscale;
        v.tine_l = self.tine * 0.5 * tl * level;
        v.tine_r = self.tine * 0.5 * tr * level;
        v.bar_l = self.bar * 0.5 * bl * level;
        v.bar_r = self.bar * 0.5 * br * level;
        v.fade.reset();
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        self.pool.release_id(id, |v| v.amp.note_off());
    }

    fn all_notes_off(&mut self) {
        self.pool.release_all(|v| v.amp.note_off());
    }

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        for n in 0..left.len().min(right.len()) {
            let lfo = self.lfo.tick(LfoShape::Sine, self.lfo_dt);
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let depth = self.trem_depth.get();
            if self.mode == 0 {
                let g = 1.0 - depth * (0.5 - 0.5 * lfo);
                self.gain_l = g;
                self.gain_r = g;
            } else {
                self.gain_l = 1.0 - depth * (0.5 - 0.5 * lfo);
                self.gain_r = 1.0 - depth * (0.5 + 0.5 * lfo);
            }
            let (mut l, mut r) = (0.0, 0.0);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick();
                l += a;
                r += b;
            }
            left[n] = l * self.gain_l * self.out_gain;
            right[n] = r * self.gain_r * self.out_gain;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
