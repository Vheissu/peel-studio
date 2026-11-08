//! `peel.drum.hat`: a bank of six band-limited square oscillators at
//! inharmonic ratios through band-pass and high-pass filters, mixed with
//! filtered noise. Short decays give closed hats, long decays give open hats.
//! Retriggering chokes the previous hit; optionally a note-off does too.

use peel_dsp::envelope::Decay;
use peel_dsp::filter::Svf;
use peel_dsp::noise::Noise;
use peel_dsp::osc::{Osc, Waveform};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::db_to_gain;
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Fade, Instrument, Pool, PoolVoice, Ramp, Synth, key_offset, lerp, ramp_coef, st_ratio,
    vel_gain,
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

/// Square-wave frequencies (Hz) of the metallic bank.
const BANK: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

static PARAMS: [ParamDescriptor; 9] = [
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
    ParamDescriptor::log(
        DECAY,
        "amp.decay",
        "Decay",
        "Amp",
        0.015,
        1.5,
        0.08,
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
    type_id: "peel.drum.hat",
    name: "Hat",
    description: "Metallic hi-hat, closed to open, with choke on retrigger",
    category: "Drums",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Closed",
        params: &[
            ("tone.tune", 0.0),
            ("tone.metal", 0.65),
            ("tone.color", 9_000.0),
            ("tone.bright", 6_500.0),
            ("amp.decay", 0.08),
            ("amp.choke", 0.006),
            ("amp.gate", 0.0),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Open",
        params: &[
            ("amp.decay", 0.55),
            ("tone.metal", 0.75),
            ("tone.bright", 6_000.0),
        ],
    },
    Preset {
        name: "Pedal",
        params: &[
            ("amp.decay", 0.04),
            ("tone.color", 7_500.0),
            ("tone.bright", 5_500.0),
            ("out.velocity", 0.5),
        ],
    },
    Preset {
        name: "Tight",
        params: &[
            ("amp.decay", 0.045),
            ("tone.metal", 0.5),
            ("tone.color", 11_000.0),
            ("tone.bright", 8_000.0),
        ],
    },
    Preset {
        name: "Sizzle",
        params: &[
            ("amp.decay", 0.28),
            ("tone.metal", 0.3),
            ("tone.color", 12_000.0),
            ("tone.bright", 7_000.0),
        ],
    },
    Preset {
        name: "Dark",
        params: &[
            ("amp.decay", 0.12),
            ("tone.tune", -4.0),
            ("tone.metal", 0.8),
            ("tone.color", 5_500.0),
            ("tone.bright", 3_500.0),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Hat::new()))
}

#[derive(Clone, Copy)]
struct HatVoice {
    active: bool,
    osc: [Osc; 6],
    dt: [f32; 6],
    noise: Noise,
    band: Svf,
    high: Svf,
    noise_high: Svf,
    fast: Decay,
    slow: Decay,
    metal_gain: f32,
    noise_gain: f32,
    gain: f32,
    fade: Fade,
    choke_ms: f32,
}

impl HatVoice {
    fn new() -> HatVoice {
        HatVoice {
            active: false,
            osc: [Osc::new(); 6],
            dt: [0.0; 6],
            noise: Noise::new(1),
            band: Svf::new(),
            high: Svf::new(),
            noise_high: Svf::new(),
            fast: Decay::new(),
            slow: Decay::new(),
            metal_gain: 1.0,
            noise_gain: 1.0,
            gain: 1.0,
            fade: Fade::new(),
            choke_ms: 6.0,
        }
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        let mut bank = 0.0;
        for (o, &dt) in self.osc.iter_mut().zip(self.dt.iter()) {
            bank += o.tick(Waveform::Square, dt, 0.5);
        }
        let metal = self.high.tick(self.band.tick(bank).band_unity).high;
        let hiss = self.noise_high.tick(self.noise.white()).high;
        let env = 0.55 * self.fast.tick() + 0.45 * self.slow.tick();
        let out = (metal * self.metal_gain + hiss * self.noise_gain) * env;
        let out = out * self.gain * self.fade.tick();
        if self.fade.is_done() || self.slow.is_below(1.0e-4) {
            self.active = false;
        }
        out
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
    sr: f32,
    pool: Pool<HatVoice>,
    ramp_k: f32,
    ctrl: usize,
    hits: u32,
    tune: f32,
    metal: f32,
    color: f32,
    bright: f32,
    decay: f32,
    choke_ms: f32,
    gate: bool,
    vel_sens: f32,
    level: Ramp,
    level_gain: f32,
}

impl Hat {
    pub(crate) fn new() -> Hat {
        Hat {
            sr: 48_000.0,
            pool: Pool::new(4, 1, StealPolicy::Oldest, 48_000.0, HatVoice::new),
            ramp_k: 0.1,
            ctrl: 0,
            hits: 0,
            tune: 0.0,
            metal: 0.65,
            color: 9_000.0,
            bright: 6_500.0,
            decay: 0.08,
            choke_ms: 6.0,
            gate: false,
            vel_sens: 0.7,
            level: Ramp::new(0.0),
            level_gain: 1.0,
        }
    }

    fn derive(&mut self) {
        self.level_gain = db_to_gain(self.level.get()) * 0.75;
    }

    fn control(&mut self) {
        self.level.tick(self.ramp_k);
        self.derive();
    }
}

impl Synth for Hat {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(4, 1, StealPolicy::Oldest, sample_rate, HatVoice::new);
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, immediate: bool) {
        match id {
            TUNE => self.tune = v,
            METAL => self.metal = v,
            COLOR => self.color = v,
            BRIGHT => self.bright = v,
            DECAY => self.decay = v,
            CHOKE => {
                self.choke_ms = v * 1000.0;
                for voice in &mut self.pool.voices {
                    voice.choke_ms = self.choke_ms;
                }
            }
            GATE => self.gate = v >= 0.5,
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
        let ratio = st_ratio(self.tune + key_offset(key));
        let bright = (self.bright * ratio.sqrt() * lerp(0.75, 1.0, velocity)).min(0.42 * sr);
        let color = (self.color * ratio.sqrt()).min(0.42 * sr);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        for (i, &hz) in BANK.iter().enumerate() {
            v.dt[i] = (hz * ratio / sr).min(0.45);
            // Fixed, spread phases so every hit has the same metallic attack.
            v.osc[i] = Osc::with_phase((i as f32 * 0.37 + 0.11).fract());
        }
        v.noise.reseed(self.hits);
        v.band.set_q(color, 1.4, sr);
        v.band.reset();
        v.high.set_q(bright, 0.8, sr);
        v.high.reset();
        v.noise_high.set_q(bright * 0.8, 0.7, sr);
        v.noise_high.reset();
        v.fast.set_t60(self.decay * 0.3, sr);
        v.fast.trigger(1.0, 0.000_2, sr);
        v.slow.set_t60(self.decay, sr);
        v.slow.trigger(1.0, 0.000_2, sr);
        v.metal_gain = self.metal * 1.6;
        v.noise_gain = lerp(0.9, 0.3, self.metal);
        v.gain = vel_gain(velocity, self.vel_sens);
        v.choke_ms = self.choke_ms;
        v.fade.reset();
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        if self.gate {
            let sr = self.sr;
            self.pool.release_id(id, |v| v.fade_out(sr));
        }
    }

    fn all_notes_off(&mut self) {}

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        for n in 0..left.len().min(right.len()) {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let mut out = 0.0;
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                out += v.tick();
            }
            left[n] = out * self.level_gain;
            right[n] = out * self.level_gain;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
