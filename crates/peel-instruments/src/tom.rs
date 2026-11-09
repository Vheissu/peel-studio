//! `peel.drum.tom`: a pitched membrane built from three inharmonic modes
//! (ratios 1, 1.59 and 2.14) sharing one downward pitch sweep, a slightly
//! detuned copy of the fundamental that makes the tone beat as a real head
//! does, and a stick impact transient that fades in from zero. Harder hits
//! start a little sharp, drive the shaper harder and are brighter.

use peel_dsp::envelope::Decay;
use peel_dsp::filter::{OnePole, Svf};
use peel_dsp::noise::Noise;
use peel_dsp::osc::fast_sin;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::db_to_gain;
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth,
    TIMBRE_OCTAVES, drive_curve, key_offset, lerp, ramp_coef, shape, st_ratio, vel_gain,
};

const PITCH: ParamId = 1;
const SWEEP: ParamId = 2;
const SWEEP_TIME: ParamId = 3;
const DECAY: ParamId = 4;
const OVERTONE: ParamId = 5;
const STICK: ParamId = 6;
const TONE: ParamId = 7;
const DRIVE: ParamId = 8;
const VELOCITY: ParamId = 9;
const LEVEL: ParamId = 10;

/// Frequency ratios of the first three membrane modes.
const MODES: [f32; 3] = [1.0, 1.593, 2.136];

static PARAMS: [ParamDescriptor; 10] = [
    ParamDescriptor::log(
        PITCH,
        "pitch.freq",
        "Pitch",
        "Pitch",
        60.0,
        400.0,
        110.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        SWEEP,
        "pitch.sweep",
        "Sweep",
        "Pitch",
        0.0,
        24.0,
        9.0,
        Unit::Semitones,
    ),
    ParamDescriptor::log(
        SWEEP_TIME,
        "pitch.sweep_time",
        "Sweep Time",
        "Pitch",
        0.01,
        0.2,
        0.05,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        DECAY,
        "body.decay",
        "Decay",
        "Body",
        0.08,
        1.5,
        0.45,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        OVERTONE,
        "body.overtone",
        "Overtone",
        "Body",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        STICK,
        "body.stick",
        "Stick",
        "Body",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        TONE,
        "body.tone",
        "Tone",
        "Body",
        400.0,
        12_000.0,
        6_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        DRIVE,
        "body.drive",
        "Drive",
        "Body",
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
    type_id: "peel.drum.tom",
    name: "Tom",
    description: "Pitched tom with membrane modes and pitch sweep",
    category: "Drums",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 8] = [
    Preset {
        name: "Floor",
        params: &[
            ("pitch.freq", 110.0),
            ("pitch.sweep", 9.0),
            ("pitch.sweep_time", 0.05),
            ("body.decay", 0.45),
            ("body.overtone", 0.35),
            ("body.stick", 0.35),
            ("body.tone", 6_000.0),
            ("body.drive", 0.15),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Rack",
        params: &[
            ("pitch.freq", 190.0),
            ("pitch.sweep", 7.0),
            ("pitch.sweep_time", 0.035),
            ("body.decay", 0.3),
            ("body.stick", 0.45),
        ],
    },
    Preset {
        name: "Electro",
        params: &[
            ("pitch.freq", 150.0),
            ("pitch.sweep", 16.0),
            ("pitch.sweep_time", 0.08),
            ("body.decay", 0.55),
            ("body.overtone", 0.05),
            ("body.stick", 0.15),
            ("body.tone", 4_000.0),
            ("body.drive", 0.1),
        ],
    },
    Preset {
        name: "Deep",
        params: &[
            ("pitch.freq", 75.0),
            ("pitch.sweep", 11.0),
            ("pitch.sweep_time", 0.07),
            ("body.decay", 0.9),
            ("body.overtone", 0.45),
            ("body.tone", 3_500.0),
        ],
    },
    Preset {
        name: "Tribal",
        params: &[
            ("pitch.freq", 130.0),
            ("pitch.sweep", 5.0),
            ("pitch.sweep_time", 0.03),
            ("body.decay", 0.2),
            ("body.overtone", 0.6),
            ("body.stick", 0.6),
            ("body.drive", 0.3),
        ],
    },
    Preset {
        name: "Big Room",
        params: &[
            ("pitch.freq", 95.0),
            ("pitch.sweep", 14.0),
            ("pitch.sweep_time", 0.06),
            ("body.decay", 0.65),
            ("body.overtone", 0.3),
            ("body.stick", 0.45),
            ("body.tone", 7_000.0),
            ("body.drive", 0.3),
        ],
    },
    Preset {
        name: "Synth Tom",
        params: &[
            ("pitch.freq", 140.0),
            ("pitch.sweep", 24.0),
            ("pitch.sweep_time", 0.1),
            ("body.decay", 0.4),
            ("body.overtone", 0.0),
            ("body.stick", 0.1),
            ("body.tone", 5_000.0),
            ("body.drive", 0.2),
        ],
    },
    Preset {
        name: "Mellow",
        params: &[
            ("pitch.freq", 140.0),
            ("pitch.sweep", 6.0),
            ("body.decay", 0.35),
            ("body.overtone", 0.15),
            ("body.stick", 0.1),
            ("body.tone", 2_200.0),
            ("body.drive", 0.0),
            ("out.velocity", 0.5),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Tom::new()))
}

#[derive(Clone, Copy)]
struct TomVoice {
    active: bool,
    phase: [f32; 4],
    freq: [f32; 4],
    amp: [Decay; 3],
    mode_gain: [f32; 4],
    drop: f32,
    snap_drop: f32,
    snap_env: f32,
    snap_coef: f32,
    stick_atk: f32,
    stick_atk_inc: f32,
    push: f32,
    pitch_env: f32,
    pitch_coef: f32,
    noise: Noise,
    stick_band: Svf,
    stick_env: f32,
    stick_coef: f32,
    stick_gain: f32,
    lp: OnePole,
    bright: f32,
    gain: f32,
    fade: Fade,
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
    /// Pitch bend ratio, 1.0 when centred.
    bend: f32,
    /// Tone filter multiplier from timbre, 1.0 at neutral.
    tone_r: f32,
}

impl TomVoice {
    fn new() -> TomVoice {
        TomVoice {
            active: false,
            phase: [0.0; 4],
            freq: [110.0; 4],
            amp: [Decay::new(); 3],
            mode_gain: [1.0; 4],
            drop: 0.0,
            snap_drop: 0.0,
            snap_env: 0.0,
            snap_coef: 0.0,
            stick_atk: 0.0,
            stick_atk_inc: 1.0,
            push: 1.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            noise: Noise::new(1),
            stick_band: Svf::new(),
            stick_env: 0.0,
            stick_coef: 0.0,
            stick_gain: 0.0,
            lp: OnePole::new(),
            bright: 1.0,
            gain: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            bend: 1.0,
            tone_r: 1.0,
        }
    }

    #[inline]
    #[allow(clippy::needless_range_loop)]
    fn tick(&mut self, sr_inv: f32, drive_g: f32, drive_norm: f32, drive_mix: f32) -> f32 {
        let m = 1.0 + self.drop * self.pitch_env + self.snap_drop * self.snap_env;
        self.pitch_env *= self.pitch_coef;
        self.snap_env *= self.snap_coef;
        let mut body = 0.0;
        let a = [
            self.amp[0].tick(),
            self.amp[0].level(),
            self.amp[1].tick(),
            self.amp[2].tick(),
        ];
        for i in 0..4 {
            self.phase[i] += (self.freq[i] * m * self.bend * sr_inv).min(0.45);
            if self.phase[i] >= 1.0 {
                self.phase[i] -= 1.0;
            }
            body += fast_sin(self.phase[i]) * a[i] * self.mode_gain[i];
        }
        let atk = self.stick_atk;
        self.stick_atk = (atk + self.stick_atk_inc).min(1.0);
        let stick = self.stick_band.tick(self.noise.white()).band_unity
            * self.stick_env
            * self.stick_gain
            * atk;
        self.stick_env *= self.stick_coef;
        let x = body * self.push + stick;
        let y = self
            .lp
            .low(lerp(x, shape(x, drive_g, drive_norm), drive_mix));
        let out = y * self.gain * self.fade.tick() * self.xgain;
        if self.fade.is_done() || self.amp[0].is_below(1.0e-4) {
            self.active = false;
        }
        out
    }
}

impl PoolVoice for TomVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.amp[0].level()
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

pub(crate) struct Tom {
    expr: Expression,
    sr: f32,
    pool: Pool<TomVoice>,
    ramp_k: f32,
    ctrl: usize,
    hits: u32,
    pitch: f32,
    sweep: f32,
    sweep_time: f32,
    decay: f32,
    overtone: f32,
    stick: f32,
    vel_sens: f32,
    tone: Ramp,
    drive: Ramp,
    level: Ramp,
    tone_hz: f32,
    drive_g: f32,
    drive_norm: f32,
    drive_mix: f32,
    level_gain: f32,
}

impl Tom {
    pub(crate) fn new() -> Tom {
        Tom {
            expr: Expression::new(),
            sr: 48_000.0,
            pool: Pool::new(5, 2, StealPolicy::Oldest, 48_000.0, TomVoice::new),
            ramp_k: 0.1,
            ctrl: 0,
            hits: 0,
            pitch: 110.0,
            sweep: 9.0,
            sweep_time: 0.05,
            decay: 0.45,
            overtone: 0.35,
            stick: 0.35,
            vel_sens: 0.7,
            tone: Ramp::new(6_000.0f32.ln()),
            drive: Ramp::new(0.15),
            level: Ramp::new(0.0),
            tone_hz: 6_000.0,
            drive_g: 1.0,
            drive_norm: 1.0,
            drive_mix: 0.0,
            level_gain: 1.0,
        }
    }

    fn derive(&mut self) {
        self.tone_hz = self.tone.get().exp();
        let drive = self.drive.get();
        (self.drive_g, self.drive_norm) = drive_curve(drive);
        self.drive_mix = (drive * 5.0).min(1.0);
        self.level_gain = db_to_gain(self.level.get()) * 0.66;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.lp.set_cutoff(self.tone_hz * v.bright * v.tone_r, self.sr);
        }
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.tone.tick(k);
        self.drive.tick(k);
        self.level.tick(k);
        self.derive();
        self.expr.tick();
        let ex = &self.expr;
        self.pool.for_active_id(|v, key, id| {
            let x = ex.voice(id, key);
            v.xgain = x.gain();
            v.bend = x.ratio;
            v.tone_r = x.tone_ratio(TIMBRE_OCTAVES);
        });
    }
}

impl Synth for Tom {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(5, 2, StealPolicy::Oldest, sample_rate, TomVoice::new);
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, immediate: bool) {
        match id {
            PITCH => self.pitch = v,
            SWEEP => self.sweep = v,
            SWEEP_TIME => self.sweep_time = v,
            DECAY => self.decay = v,
            OVERTONE => self.overtone = v,
            STICK => self.stick = v,
            TONE => self.tone.set(v.ln(), immediate),
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
        let f0 = (self.pitch * ratio).clamp(20.0, 0.2 * sr);
        let drop = st_ratio(self.sweep * lerp(0.8, 1.0, velocity)) - 1.0;
        let tone = self.tone_hz;
        let x = self.expr.voice(id, key);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        v.bend = x.ratio;
        v.tone_r = x.tone_ratio(TIMBRE_OCTAVES);
        for i in 0..3 {
            v.amp[i].set_t60(self.decay * [1.0, 0.55, 0.4][i], sr);
            v.amp[i].trigger(1.0, 0.000_4, sr);
        }
        // Fundamental, its slightly detuned twin (beating), modes 2 and 3.
        let ratios = [MODES[0], MODES[0] * 1.004, MODES[1], MODES[2]];
        for (i, r) in ratios.iter().enumerate() {
            v.phase[i] = 0.0;
            v.freq[i] = (f0 * r).min(0.4 * sr);
        }
        v.mode_gain = [1.0, 0.3, 0.55 * self.overtone, 0.4 * self.overtone];
        v.snap_drop = 0.035 * velocity;
        v.snap_env = 1.0;
        v.snap_coef = (-1.0 / (0.005 * sr)).exp();
        v.push = lerp(0.75, 1.0, velocity);
        v.stick_atk = 0.0;
        v.stick_atk_inc = 1.0 / (0.000_3 * sr);
        v.drop = drop;
        v.pitch_env = 1.0;
        v.pitch_coef = (-1.0 / (self.sweep_time * sr)).exp();
        v.noise.reseed(self.hits);
        v.stick_band
            .set_q((f0 * 12.0).clamp(1_200.0, 6_000.0), 1.2, sr);
        v.stick_band.reset();
        v.stick_env = 1.0;
        v.stick_coef = (-1.0 / (0.003 * sr)).exp();
        // Timbre also sets how hard the stick lands.
        v.stick_gain = self.stick * 0.9 * lerp(0.4, 1.0, velocity) * v.tone_r.sqrt();
        v.bright = lerp(0.35, 1.0, velocity.sqrt());
        v.lp.set_cutoff(tone * v.bright * v.tone_r, sr);
        v.lp.reset();
        v.gain = vel_gain(velocity, self.vel_sens);
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
                out += v.tick(sr_inv, self.drive_g, self.drive_norm, self.drive_mix);
            }
            left[n] = out * self.level_gain;
            right[n] = out * self.level_gain;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
