//! `peel.drum.clap`: a cluster of short band-passed noise bursts followed by
//! a diffuse noise tail. Left and right use independent noise so the room
//! tail is naturally wide.

use peel_dsp::envelope::Decay;
use peel_dsp::filter::Svf;
use peel_dsp::noise::Noise;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::db_to_gain;
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, drive_curve, key_offset,
    lerp, ramp_coef, shape, st_ratio, vel_gain,
};

const TONE: ParamId = 1;
const FOCUS: ParamId = 2;
const BURSTS: ParamId = 3;
const SPREAD: ParamId = 4;
const SNAP: ParamId = 5;
const DECAY: ParamId = 6;
const ROOM: ParamId = 7;
const DRIVE: ParamId = 8;
const VELOCITY: ParamId = 9;
const LEVEL: ParamId = 10;

static PARAMS: [ParamDescriptor; 10] = [
    ParamDescriptor::log(
        TONE,
        "tone.freq",
        "Tone",
        "Tone",
        600.0,
        3_500.0,
        1_350.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        FOCUS,
        "tone.focus",
        "Focus",
        "Tone",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::choice(
        BURSTS,
        "burst.count",
        "Bursts",
        "Bursts",
        &["2", "3", "4", "5"],
        2,
    ),
    ParamDescriptor::log(
        SPREAD,
        "burst.spread",
        "Spread",
        "Bursts",
        0.005,
        0.025,
        0.0105,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        SNAP,
        "burst.snap",
        "Snap",
        "Bursts",
        0.0,
        1.0,
        0.8,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        DECAY,
        "tail.decay",
        "Decay",
        "Tail",
        0.08,
        1.0,
        0.3,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        ROOM,
        "tail.room",
        "Room",
        "Tail",
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
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.drum.clap",
    name: "Clap",
    description: "Multi-burst clap with a diffuse stereo room tail",
    category: "Drums",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Classic",
        params: &[
            ("tone.freq", 1_350.0),
            ("tone.focus", 0.35),
            ("burst.count", 2.0),
            ("burst.spread", 0.0105),
            ("burst.snap", 0.8),
            ("tail.decay", 0.3),
            ("tail.room", 0.5),
            ("out.drive", 0.15),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Tight",
        params: &[
            ("tone.freq", 1_700.0),
            ("burst.count", 1.0),
            ("burst.spread", 0.007),
            ("tail.decay", 0.14),
            ("tail.room", 0.3),
        ],
    },
    Preset {
        name: "Big Room",
        params: &[
            ("tone.freq", 1_100.0),
            ("burst.count", 3.0),
            ("burst.spread", 0.013),
            ("tail.decay", 0.75),
            ("tail.room", 0.85),
            ("burst.snap", 0.6),
        ],
    },
    Preset {
        name: "Snappy",
        params: &[
            ("tone.freq", 2_000.0),
            ("tone.focus", 0.55),
            ("burst.count", 2.0),
            ("burst.spread", 0.009),
            ("burst.snap", 1.0),
            ("tail.decay", 0.2),
            ("tail.room", 0.25),
            ("out.drive", 0.3),
        ],
    },
    Preset {
        name: "Crowd",
        params: &[
            ("tone.freq", 900.0),
            ("tone.focus", 0.15),
            ("burst.count", 3.0),
            ("burst.spread", 0.02),
            ("tail.decay", 0.55),
            ("tail.room", 0.9),
            ("burst.snap", 0.5),
        ],
    },
    Preset {
        name: "Dry",
        params: &[
            ("tone.freq", 1_500.0),
            ("burst.count", 2.0),
            ("tail.decay", 0.1),
            ("tail.room", 0.1),
            ("out.drive", 0.1),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Clap::new()))
}

#[derive(Clone, Copy)]
struct ClapVoice {
    active: bool,
    noise_l: Noise,
    noise_r: Noise,
    burst_l: Svf,
    burst_r: Svf,
    tail_l: Svf,
    tail_r: Svf,
    /// Samples since the hit.
    t: u32,
    spacing: u32,
    bursts: u32,
    next_burst: u32,
    burst_env: f32,
    burst_coef: f32,
    burst_level: f32,
    tail: Decay,
    snap_gain: f32,
    room_gain: f32,
    gain: f32,
    fade: Fade,
    jitter: Noise,
    sr: f32,
}

impl ClapVoice {
    fn new() -> ClapVoice {
        ClapVoice {
            active: false,
            noise_l: Noise::new(1),
            noise_r: Noise::new(2),
            burst_l: Svf::new(),
            burst_r: Svf::new(),
            tail_l: Svf::new(),
            tail_r: Svf::new(),
            t: 0,
            spacing: 500,
            bursts: 3,
            next_burst: 0,
            burst_env: 0.0,
            burst_coef: 0.0,
            burst_level: 1.0,
            tail: Decay::new(),
            snap_gain: 1.0,
            room_gain: 1.0,
            gain: 1.0,
            fade: Fade::new(),
            jitter: Noise::new(3),
            sr: 48_000.0,
        }
    }

    #[inline]
    fn tick(&mut self, drive_g: f32, drive_norm: f32) -> (f32, f32) {
        // Each burst is a fast-decaying re-trigger of the same envelope; the
        // last one hands over to the tail.
        if self.next_burst < self.bursts && self.t == self.next_burst * self.spacing {
            self.burst_env = 1.0;
            self.burst_level = 0.7 + 0.3 * self.jitter.uniform();
            if self.next_burst + 1 == self.bursts {
                self.tail.trigger(1.0, 0.000_5, self.sr);
            }
            self.next_burst += 1;
        }
        self.t = self.t.saturating_add(1);

        let burst = self.burst_env * self.burst_level * self.snap_gain;
        self.burst_env *= self.burst_coef;
        let tail = self.tail.tick() * self.room_gain;

        let nl = self.noise_l.white();
        let nr = self.noise_r.white();
        let l = self.burst_l.tick(nl).band_unity * burst + self.tail_l.tick(nl).band_unity * tail;
        let r = self.burst_r.tick(nr).band_unity * burst + self.tail_r.tick(nr).band_unity * tail;
        let f = self.gain * self.fade.tick();
        let l = shape(l, drive_g, drive_norm) * f;
        let r = shape(r, drive_g, drive_norm) * f;
        let finished =
            self.next_burst >= self.bursts && self.burst_env < 1.0e-3 && self.tail.is_below(1.0e-4);
        if self.fade.is_done() || finished {
            self.active = false;
        }
        (l, r)
    }
}

impl PoolVoice for ClapVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.tail.level().max(self.burst_env)
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

pub(crate) struct Clap {
    sr: f32,
    pool: Pool<ClapVoice>,
    ramp_k: f32,
    ctrl: usize,
    hits: u32,
    tone: f32,
    focus: f32,
    bursts: u32,
    spread: f32,
    snap: f32,
    decay: f32,
    room: f32,
    vel_sens: f32,
    drive: Ramp,
    level: Ramp,
    drive_g: f32,
    drive_norm: f32,
    level_gain: f32,
}

impl Clap {
    pub(crate) fn new() -> Clap {
        Clap {
            sr: 48_000.0,
            pool: Pool::new(5, 2, StealPolicy::Oldest, 48_000.0, ClapVoice::new),
            ramp_k: 0.1,
            ctrl: 0,
            hits: 0,
            tone: 1_350.0,
            focus: 0.35,
            bursts: 4,
            spread: 0.0105,
            snap: 0.8,
            decay: 0.3,
            room: 0.5,
            vel_sens: 0.7,
            drive: Ramp::new(0.15),
            level: Ramp::new(0.0),
            drive_g: 1.0,
            drive_norm: 1.0,
            level_gain: 1.0,
        }
    }

    fn derive(&mut self) {
        (self.drive_g, self.drive_norm) = drive_curve(self.drive.get());
        self.level_gain = db_to_gain(self.level.get()) * 0.9;
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.drive.tick(k);
        self.level.tick(k);
        self.derive();
    }
}

impl Synth for Clap {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(5, 2, StealPolicy::Oldest, sample_rate, ClapVoice::new);
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, immediate: bool) {
        match id {
            TONE => self.tone = v,
            FOCUS => self.focus = v,
            BURSTS => self.bursts = v as u32 + 2,
            SPREAD => self.spread = v,
            SNAP => self.snap = v,
            DECAY => self.decay = v,
            ROOM => self.room = v,
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
        let center = (self.tone * ratio * lerp(0.85, 1.0, velocity)).min(0.4 * sr);
        let q = 0.6 + 3.2 * self.focus;
        let hits = self.hits;
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.sr = sr;
        v.noise_l.reseed(hits.wrapping_mul(2));
        v.noise_r.reseed(hits.wrapping_mul(2).wrapping_add(1));
        v.jitter.reseed(hits ^ 0x5bd1);
        v.burst_l.set_q(center, q, sr);
        v.burst_r.set_q(center, q, sr);
        v.tail_l.set_q(center * 0.85, 0.7, sr);
        v.tail_r.set_q(center * 0.85, 0.7, sr);
        v.burst_l.reset();
        v.burst_r.reset();
        v.tail_l.reset();
        v.tail_r.reset();
        v.t = 0;
        v.spacing = ((self.spread * sr) as u32).max(1);
        v.bursts = self.bursts;
        v.next_burst = 0;
        v.burst_env = 0.0;
        v.burst_coef = (-6.9 / (0.012 * sr)).exp();
        v.tail.reset();
        v.tail.set_t60(self.decay, sr);
        v.snap_gain = self.snap * 1.5;
        v.room_gain = self.room * 1.4 * lerp(0.6, 1.0, velocity);
        v.gain = vel_gain(velocity, self.vel_sens) * self.level_gain;
        v.fade.reset();
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, _id: u32) {}

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
            let (mut l, mut r) = (0.0, 0.0);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick(self.drive_g, self.drive_norm);
                l += a;
                r += b;
            }
            left[n] = l;
            right[n] = r;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
