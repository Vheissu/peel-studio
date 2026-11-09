//! `peel.drum.clap`: a cluster of short band-passed noise bursts (the hands
//! landing a few milliseconds apart) followed by a diffuse noise tail. A
//! second, higher band adds the crack that cuts through a mix; the width
//! control blends common and independent noise, so the bursts stay fairly
//! central while the room tail opens up. Harder hits are brighter.

use peel_dsp::envelope::Decay;
use peel_dsp::filter::Svf;
use peel_dsp::noise::Noise;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::db_to_gain;
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, TIMBRE_OCTAVES, Vx,
    drive_curve, key_offset, lerp, ramp_coef, shape, st_ratio, vel_gain,
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
const AIR: ParamId = 11;
const WIDTH: ParamId = 12;

static PARAMS: [ParamDescriptor; 12] = [
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
    ParamDescriptor::linear(AIR, "tone.air", "Air", "Tone", 0.0, 1.0, 0.4, Unit::Percent),
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
        WIDTH,
        "out.width",
        "Width",
        "Output",
        0.0,
        1.0,
        0.45,
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

pub static PRESETS: [Preset; 8] = [
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
            ("burst.snap", 0.85),
            ("tone.air", 0.5),
            ("out.width", 0.6),
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
            ("out.width", 0.75),
        ],
    },
    Preset {
        name: "EDM",
        params: &[
            ("tone.freq", 1_500.0),
            ("tone.focus", 0.25),
            ("tone.air", 0.6),
            ("burst.count", 2.0),
            ("burst.spread", 0.009_5),
            ("burst.snap", 0.9),
            ("tail.decay", 0.42),
            ("tail.room", 0.6),
            ("out.width", 0.5),
            ("out.drive", 0.25),
        ],
    },
    Preset {
        name: "Layered",
        params: &[
            ("tone.freq", 1_800.0),
            ("tone.focus", 0.2),
            ("tone.air", 0.9),
            ("burst.count", 1.0),
            ("burst.spread", 0.007),
            ("burst.snap", 1.0),
            ("tail.decay", 0.3),
            ("tail.room", 0.45),
            ("out.width", 0.5),
            ("out.drive", 0.3),
        ],
    },
    Preset {
        name: "Dry",
        params: &[
            ("tone.freq", 1_500.0),
            ("burst.count", 2.0),
            ("tail.decay", 0.1),
            ("tail.room", 0.1),
            ("out.width", 0.3),
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
    common: Noise,
    noise_l: Noise,
    noise_r: Noise,
    burst_l: Svf,
    burst_r: Svf,
    tail_l: Svf,
    tail_r: Svf,
    air_l: Svf,
    air_r: Svf,
    air_gain: f32,
    mix_common: f32,
    mix_side: f32,
    /// Samples since the hit.
    t: u32,
    spacing: u32,
    bursts: u32,
    next_burst: u32,
    burst_env: f32,
    burst_age: f32,
    burst_atk: f32,
    burst_coef: f32,
    burst_level: f32,
    tail: Decay,
    snap_gain: f32,
    room_gain: f32,
    gain: f32,
    fade: Fade,
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
    /// Timbre the burst and tail bands are tuned for, their centre and Q.
    tone: f32,
    center: f32,
    q: f32,
    jitter: Noise,
    sr: f32,
}

impl ClapVoice {
    /// Timbre moves the burst and tail bands by up to two octaves.
    fn set_tone(&mut self, x: &Vx) {
        if x.tone != self.tone {
            self.tone = x.tone;
            let (sr, q) = (self.sr, self.q);
            let c = (self.center * x.tone_ratio(TIMBRE_OCTAVES)).min(0.4 * sr);
            self.burst_l.set_q(c, q, sr);
            self.burst_r.set_q(c, q, sr);
            self.tail_l.set_q(c * 0.85, 0.7, sr);
            self.tail_r.set_q(c * 0.85, 0.7, sr);
        }
    }

    fn new() -> ClapVoice {
        ClapVoice {
            active: false,
            common: Noise::new(7),
            noise_l: Noise::new(1),
            noise_r: Noise::new(2),
            burst_l: Svf::new(),
            burst_r: Svf::new(),
            tail_l: Svf::new(),
            tail_r: Svf::new(),
            air_l: Svf::new(),
            air_r: Svf::new(),
            air_gain: 0.0,
            mix_common: 0.6,
            mix_side: 0.8,
            t: 0,
            spacing: 500,
            bursts: 3,
            next_burst: 0,
            burst_env: 0.0,
            burst_age: 0.0,
            burst_atk: 1.0,
            burst_coef: 0.0,
            burst_level: 1.0,
            tail: Decay::new(),
            snap_gain: 1.0,
            room_gain: 1.0,
            gain: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            tone: 0.0,
            center: 1_000.0,
            q: 1.0,
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
            self.burst_age = 0.0;
            self.burst_level = 0.7 + 0.3 * self.jitter.uniform();
            if self.next_burst + 1 == self.bursts {
                self.tail.trigger(1.0, 0.000_5, self.sr);
            }
            self.next_burst += 1;
        }
        self.t = self.t.saturating_add(1);

        // Each strike fades in over ~0.15 ms so a hit never starts with a step.
        let atk = (self.burst_age * self.burst_atk).min(1.0);
        self.burst_age += 1.0;
        let burst = self.burst_env * self.burst_level * self.snap_gain * atk;
        self.burst_env *= self.burst_coef;
        let tail = self.tail.tick() * self.room_gain;

        let c = self.common.white() * self.mix_common;
        let nl = c + self.noise_l.white() * self.mix_side;
        let nr = c + self.noise_r.white() * self.mix_side;
        let al = self.air_l.tick(nl).band_unity * self.air_gain;
        let ar = self.air_r.tick(nr).band_unity * self.air_gain;
        let l = self.burst_l.tick(nl).band_unity * burst
            + al * (burst + 0.25 * tail)
            + self.tail_l.tick(nl).band_unity * tail;
        let r = self.burst_r.tick(nr).band_unity * burst
            + ar * (burst + 0.25 * tail)
            + self.tail_r.tick(nr).band_unity * tail;
        let f = self.gain * self.fade.tick() * self.xgain;
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
    expr: Expression,
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
    air: f32,
    width: f32,
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
            expr: Expression::new(),
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
            air: 0.4,
            width: 0.45,
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
        self.level_gain = db_to_gain(self.level.get()) * 0.85;
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.drive.tick(k);
        self.level.tick(k);
        self.derive();
        self.expr.tick();
        let ex = &self.expr;
        self.pool.for_active_id(|v, key, id| {
            let x = ex.voice(id, key);
            v.xgain = x.gain();
            v.set_tone(&x);
        });
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
            AIR => self.air = v,
            WIDTH => self.width = v,
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
        // Filtered noise has no pitch to bend; pressure and timbre apply.
        let x = self.expr.voice(id, key);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        v.sr = sr;
        v.center = center;
        v.q = q;
        v.tone = 0.0;
        v.common.reseed(hits.wrapping_mul(3));
        v.noise_l.reseed(hits.wrapping_mul(3).wrapping_add(1));
        v.noise_r.reseed(hits.wrapping_mul(3).wrapping_add(2));
        v.jitter.reseed(hits ^ 0x5bd1);
        v.burst_l.set_q(center, q, sr);
        v.burst_r.set_q(center, q, sr);
        v.tail_l.set_q(center * 0.85, 0.7, sr);
        v.tail_r.set_q(center * 0.85, 0.7, sr);
        let air_hz = (center * 2.4).min(0.4 * sr);
        v.air_l.set_q(air_hz, 0.8, sr);
        v.air_r.set_q(air_hz, 0.8, sr);
        v.air_l.reset();
        v.air_r.reset();
        v.set_tone(&x);
        v.air_gain = self.air * 0.6 * lerp(0.3, 1.0, velocity);
        v.mix_common = (1.0 - self.width).sqrt();
        v.mix_side = self.width.sqrt();
        v.burst_l.reset();
        v.burst_r.reset();
        v.tail_l.reset();
        v.tail_r.reset();
        v.t = 0;
        v.spacing = ((self.spread * sr) as u32).max(1);
        v.bursts = self.bursts;
        v.next_burst = 0;
        v.burst_env = 0.0;
        v.burst_age = 0.0;
        v.burst_atk = 1.0 / (0.000_15 * sr);
        v.burst_coef = (-6.9 / (0.012 * sr)).exp();
        v.tail.reset();
        v.tail.set_t60(self.decay, sr);
        v.snap_gain = self.snap * 3.2;
        v.room_gain = self.room * 0.85 * lerp(0.6, 1.0, velocity);
        v.gain = vel_gain(velocity, self.vel_sens);
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
            left[n] = l * self.level_gain;
            right[n] = r * self.level_gain;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
