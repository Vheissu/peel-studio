//! `peel.drum.kick`: sine body with an exponential pitch sweep, a noise click
//! transient, waveshaping drive and a tone filter. Long decays with a small
//! sweep give 808-style kicks.

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
const SWEEP: ParamId = 2;
const SWEEP_TIME: ParamId = 3;
const DECAY: ParamId = 4;
const DRIVE: ParamId = 5;
const TONE: ParamId = 6;
const CLICK: ParamId = 7;
const CLICK_TONE: ParamId = 8;
const VELOCITY: ParamId = 9;
const LEVEL: ParamId = 10;

static PARAMS: [ParamDescriptor; 10] = [
    ParamDescriptor::log(
        PITCH,
        "pitch.freq",
        "Pitch",
        "Pitch",
        25.0,
        150.0,
        50.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        SWEEP,
        "pitch.sweep",
        "Sweep",
        "Pitch",
        0.0,
        48.0,
        26.0,
        Unit::Semitones,
    ),
    ParamDescriptor::log(
        SWEEP_TIME,
        "pitch.sweep_time",
        "Sweep Time",
        "Pitch",
        0.005,
        0.3,
        0.03,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        DECAY,
        "body.decay",
        "Decay",
        "Body",
        0.05,
        3.0,
        0.45,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        DRIVE,
        "body.drive",
        "Drive",
        "Body",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        TONE,
        "body.tone",
        "Tone",
        "Body",
        200.0,
        16_000.0,
        9_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        CLICK,
        "click.level",
        "Click",
        "Click",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        CLICK_TONE,
        "click.tone",
        "Click Tone",
        "Click",
        800.0,
        8_000.0,
        3_200.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        VELOCITY,
        "out.velocity",
        "Velocity",
        "Output",
        0.0,
        1.0,
        0.6,
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
    type_id: "peel.drum.kick",
    name: "Kick",
    description: "Synthesized kick drum with pitch sweep, click and drive",
    category: "Drums",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Punch",
        params: &[
            ("pitch.freq", 50.0),
            ("pitch.sweep", 26.0),
            ("pitch.sweep_time", 0.03),
            ("body.decay", 0.45),
            ("body.drive", 0.3),
            ("body.tone", 9_000.0),
            ("click.level", 0.3),
            ("click.tone", 3_200.0),
            ("out.velocity", 0.6),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "808 Long",
        params: &[
            ("pitch.freq", 45.0),
            ("pitch.sweep", 12.0),
            ("pitch.sweep_time", 0.012),
            ("body.decay", 1.8),
            ("body.drive", 0.15),
            ("body.tone", 4_500.0),
            ("click.level", 0.1),
        ],
    },
    Preset {
        name: "Soft",
        params: &[
            ("pitch.freq", 55.0),
            ("pitch.sweep", 18.0),
            ("pitch.sweep_time", 0.05),
            ("body.decay", 0.3),
            ("body.drive", 0.0),
            ("body.tone", 3_000.0),
            ("click.level", 0.05),
            ("out.velocity", 0.5),
        ],
    },
    Preset {
        name: "Techno",
        params: &[
            ("pitch.freq", 52.0),
            ("pitch.sweep", 36.0),
            ("pitch.sweep_time", 0.022),
            ("body.decay", 0.38),
            ("body.drive", 0.7),
            ("body.tone", 14_000.0),
            ("click.level", 0.5),
            ("click.tone", 4_200.0),
        ],
    },
    Preset {
        name: "Deep",
        params: &[
            ("pitch.freq", 40.0),
            ("pitch.sweep", 20.0),
            ("pitch.sweep_time", 0.06),
            ("body.decay", 0.9),
            ("body.drive", 0.2),
            ("body.tone", 2_500.0),
            ("click.level", 0.08),
        ],
    },
    Preset {
        name: "Tight",
        params: &[
            ("pitch.freq", 62.0),
            ("pitch.sweep", 24.0),
            ("pitch.sweep_time", 0.018),
            ("body.decay", 0.2),
            ("body.drive", 0.35),
            ("body.tone", 10_000.0),
            ("click.level", 0.45),
            ("click.tone", 3_800.0),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Kick::new()))
}

#[derive(Clone, Copy)]
struct KickVoice {
    active: bool,
    phase: f32,
    f0: f32,
    f_span: f32,
    pitch_env: f32,
    pitch_coef: f32,
    amp: Decay,
    click_env: f32,
    click_coef: f32,
    click_gain: f32,
    band: Svf,
    noise: Noise,
    lp: OnePole,
    bright: f32,
    gain: f32,
    fade: Fade,
}

impl KickVoice {
    fn new() -> KickVoice {
        KickVoice {
            active: false,
            phase: 0.0,
            f0: 50.0,
            f_span: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            amp: Decay::new(),
            click_env: 0.0,
            click_coef: 0.0,
            click_gain: 0.0,
            band: Svf::new(),
            noise: Noise::new(1),
            lp: OnePole::new(),
            bright: 1.0,
            gain: 1.0,
            fade: Fade::new(),
        }
    }

    #[inline]
    fn tick(&mut self, sr_inv: f32, drive_g: f32, drive_norm: f32) -> f32 {
        let freq = self.f0 + self.f_span * self.pitch_env;
        self.pitch_env *= self.pitch_coef;
        self.phase += (freq * sr_inv).min(0.45);
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let amp = self.amp.tick();
        let sine = fast_sin(self.phase) * amp;
        let body = shape(sine, drive_g, drive_norm);
        let click = self.band.tick(self.noise.white()).band * self.click_env * self.click_gain;
        self.click_env *= self.click_coef;
        let y = self.lp.low(body + click);
        let out = y * self.gain * self.fade.tick();
        if self.fade.is_done() || (self.amp.is_below(1.0e-4) && self.click_env < 1.0e-3) {
            self.active = false;
        }
        out
    }
}

impl PoolVoice for KickVoice {
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
    }
}

pub(crate) struct Kick {
    sr: f32,
    pool: Pool<KickVoice>,
    ramp_k: f32,
    ctrl: usize,
    hits: u32,
    // Trigger-time parameters.
    pitch: f32,
    sweep: f32,
    sweep_time: f32,
    decay: f32,
    click: f32,
    click_tone: f32,
    vel_sens: f32,
    // Smoothed parameters.
    drive: Ramp,
    tone: Ramp,
    level: Ramp,
    // Control-rate values.
    drive_g: f32,
    drive_norm: f32,
    tone_hz: f32,
    level_gain: f32,
}

impl Kick {
    pub(crate) fn new() -> Kick {
        Kick {
            sr: 48_000.0,
            pool: Pool::new(4, 1, StealPolicy::Oldest, 48_000.0, KickVoice::new),
            ramp_k: 0.1,
            ctrl: 0,
            hits: 0,
            pitch: 50.0,
            sweep: 26.0,
            sweep_time: 0.03,
            decay: 0.45,
            click: 0.3,
            click_tone: 3_200.0,
            vel_sens: 0.6,
            drive: Ramp::new(0.3),
            tone: Ramp::new(9_000.0f32.ln()),
            level: Ramp::new(0.0),
            drive_g: 1.0,
            drive_norm: 1.0,
            tone_hz: 9_000.0,
            level_gain: 1.0,
        }
    }

    /// Recomputes values derived from the smoothed parameters.
    fn derive(&mut self) {
        let drive = self.drive.get();
        (self.drive_g, self.drive_norm) = drive_curve(drive);
        self.tone_hz = self.tone.get().exp();
        self.level_gain = db_to_gain(self.level.get()) * 0.9;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.lp.set_cutoff(self.tone_hz * v.bright, self.sr);
        }
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.drive.tick(k);
        self.tone.tick(k);
        self.level.tick(k);
        self.derive();
    }
}

impl Synth for Kick {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(4, 1, StealPolicy::Oldest, sample_rate, KickVoice::new);
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
            DRIVE => self.drive.set(v, immediate),
            TONE => self.tone.set(v.ln(), immediate),
            CLICK => self.click = v,
            CLICK_TONE => self.click_tone = v,
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
        let f0 = (self.pitch * ratio).clamp(15.0, 0.4 * sr);
        let sweep_ratio = st_ratio(self.sweep * lerp(0.85, 1.0, velocity));
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.phase = 0.0;
        v.f0 = f0;
        v.f_span = (f0 * (sweep_ratio - 1.0)).min(0.4 * sr - f0).max(0.0);
        v.pitch_env = 1.0;
        v.pitch_coef = (-1.0 / (self.sweep_time * sr)).exp();
        v.amp.set_t60(self.decay, sr);
        v.amp.trigger(1.0, 0.000_4, sr);
        v.click_env = 1.0;
        v.click_coef = (-1.0 / (0.002 * sr)).exp();
        v.click_gain = self.click * 0.9 * lerp(0.4, 1.0, velocity);
        v.band.set(self.click_tone, 0.55, sr);
        v.band.reset();
        v.noise.reseed(self.hits);
        v.bright = lerp(0.3, 1.0, velocity.sqrt());
        v.lp.set_cutoff(self.tone_hz * v.bright, sr);
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
                out += v.tick(sr_inv, self.drive_g, self.drive_norm);
            }
            left[n] = out * self.level_gain;
            right[n] = out * self.level_gain;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
