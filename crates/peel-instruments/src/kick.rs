//! `peel.drum.kick`: a sine body whose pitch falls in two exponential stages
//! (a very fast snap, then a slower sweep, as in analogue kick circuits), a
//! top layer of band-passed noise plus a short tone burst, waveshaping drive,
//! a tone filter, a final soft clipper for loudness and a DC blocker. The
//! body phase starts at zero and the click fades in over a fraction of a
//! millisecond, so a hit never starts with a step. Output is mono.

use peel_dsp::envelope::Decay;
use peel_dsp::filter::{OnePole, Svf, tanh_approx};
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
const SNAP: ParamId = 11;
const CLIP: ParamId = 12;

static PARAMS: [ParamDescriptor; 12] = [
    ParamDescriptor::log(
        PITCH,
        "pitch.freq",
        "Pitch",
        "Pitch",
        25.0,
        150.0,
        52.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        SWEEP,
        "pitch.sweep",
        "Sweep",
        "Pitch",
        0.0,
        48.0,
        22.0,
        Unit::Semitones,
    ),
    ParamDescriptor::linear(
        SNAP,
        "pitch.snap",
        "Snap",
        "Pitch",
        0.0,
        24.0,
        8.0,
        Unit::Semitones,
    ),
    ParamDescriptor::log(
        SWEEP_TIME,
        "pitch.sweep_time",
        "Sweep Time",
        "Pitch",
        0.005,
        0.3,
        0.032,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        DECAY,
        "body.decay",
        "Decay",
        "Body",
        0.05,
        3.0,
        0.42,
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
    ParamDescriptor::linear(
        CLIP,
        "body.clip",
        "Clip",
        "Body",
        0.0,
        1.0,
        0.25,
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
        0.65,
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

pub static PRESETS: [Preset; 10] = [
    Preset {
        name: "Punch",
        params: &[
            ("pitch.freq", 52.0),
            ("pitch.sweep", 22.0),
            ("pitch.snap", 8.0),
            ("pitch.sweep_time", 0.032),
            ("body.decay", 0.42),
            ("body.drive", 0.3),
            ("body.clip", 0.25),
            ("body.tone", 9_000.0),
            ("click.level", 0.65),
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
            ("pitch.snap", 4.0),
            ("pitch.sweep_time", 0.012),
            ("body.decay", 1.8),
            ("body.drive", 0.12),
            ("body.clip", 0.1),
            ("body.tone", 4_500.0),
            ("click.level", 0.2),
            ("click.tone", 2_000.0),
        ],
    },
    Preset {
        name: "Soft",
        params: &[
            ("pitch.freq", 55.0),
            ("pitch.sweep", 18.0),
            ("pitch.snap", 3.0),
            ("pitch.sweep_time", 0.05),
            ("body.decay", 0.3),
            ("body.drive", 0.0),
            ("body.clip", 0.0),
            ("body.tone", 3_000.0),
            ("click.level", 0.05),
            ("out.velocity", 0.5),
        ],
    },
    Preset {
        name: "Techno",
        params: &[
            ("pitch.freq", 51.0),
            ("pitch.sweep", 30.0),
            ("pitch.snap", 10.0),
            ("pitch.sweep_time", 0.022),
            ("body.decay", 0.36),
            ("body.drive", 0.65),
            ("body.clip", 0.5),
            ("body.tone", 14_000.0),
            ("click.level", 0.55),
            ("click.tone", 4_200.0),
        ],
    },
    Preset {
        name: "Deep",
        params: &[
            ("pitch.freq", 42.0),
            ("pitch.sweep", 20.0),
            ("pitch.snap", 5.0),
            ("pitch.sweep_time", 0.06),
            ("body.decay", 0.9),
            ("body.drive", 0.2),
            ("body.clip", 0.1),
            ("body.tone", 2_500.0),
            ("click.level", 0.08),
        ],
    },
    Preset {
        name: "Tight",
        params: &[
            ("pitch.freq", 60.0),
            ("pitch.sweep", 22.0),
            ("pitch.snap", 8.0),
            ("pitch.sweep_time", 0.018),
            ("body.decay", 0.2),
            ("body.drive", 0.35),
            ("body.clip", 0.3),
            ("body.tone", 10_000.0),
            ("click.level", 0.7),
            ("click.tone", 3_800.0),
        ],
    },
    Preset {
        name: "Big Room",
        params: &[
            ("pitch.freq", 50.0),
            ("pitch.sweep", 26.0),
            ("pitch.snap", 12.0),
            ("pitch.sweep_time", 0.04),
            ("body.decay", 0.5),
            ("body.drive", 0.5),
            ("body.clip", 0.6),
            ("body.tone", 12_000.0),
            ("click.level", 0.55),
            ("click.tone", 2_800.0),
            ("out.velocity", 0.5),
        ],
    },
    Preset {
        name: "Sub 808",
        params: &[
            ("pitch.freq", 48.0),
            ("pitch.sweep", 9.0),
            ("pitch.snap", 3.0),
            ("pitch.sweep_time", 0.02),
            ("body.decay", 1.4),
            ("body.drive", 0.0),
            ("body.clip", 0.0),
            ("body.tone", 1_800.0),
            ("click.level", 0.04),
            ("click.tone", 1_200.0),
            ("out.velocity", 0.4),
        ],
    },
    Preset {
        name: "Hard Dance",
        params: &[
            ("pitch.freq", 56.0),
            ("pitch.sweep", 32.0),
            ("pitch.snap", 14.0),
            ("pitch.sweep_time", 0.05),
            ("body.decay", 0.3),
            ("body.drive", 0.85),
            ("body.clip", 0.9),
            ("body.tone", 16_000.0),
            ("click.level", 0.6),
            ("click.tone", 3_600.0),
            ("out.velocity", 0.4),
        ],
    },
    Preset {
        name: "Club",
        params: &[
            ("pitch.freq", 54.0),
            ("pitch.sweep", 20.0),
            ("pitch.snap", 6.0),
            ("pitch.sweep_time", 0.026),
            ("body.decay", 0.28),
            ("body.drive", 0.4),
            ("body.clip", 0.35),
            ("body.tone", 8_000.0),
            ("click.level", 0.5),
            ("click.tone", 2_400.0),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Kick::new()))
}

/// Soft clipper: unity gain for small signals, rounding off peaks smoothly
/// toward full scale. `g` is the drive into the curve.
#[inline]
fn soft_clip(x: f32, g: f32, norm: f32) -> f32 {
    tanh_approx(x * g) * norm
}

#[derive(Clone, Copy)]
struct KickVoice {
    active: bool,
    phase: f32,
    f0: f32,
    f_span: f32,
    f_snap: f32,
    pitch_env: f32,
    pitch_coef: f32,
    snap_env: f32,
    snap_coef: f32,
    amp: Decay,
    click_env: f32,
    click_coef: f32,
    click_gain: f32,
    click_atk: f32,
    click_atk_inc: f32,
    burst_phase: f32,
    burst_dt: f32,
    burst_env: f32,
    burst_coef: f32,
    band: Svf,
    noise: Noise,
    lp: OnePole,
    dc: OnePole,
    bright: f32,
    push: f32,
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
            f_snap: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            snap_env: 0.0,
            snap_coef: 0.0,
            amp: Decay::new(),
            click_env: 0.0,
            click_coef: 0.0,
            click_gain: 0.0,
            click_atk: 0.0,
            click_atk_inc: 1.0,
            burst_phase: 0.0,
            burst_dt: 0.0,
            burst_env: 0.0,
            burst_coef: 0.0,
            band: Svf::new(),
            noise: Noise::new(1),
            lp: OnePole::new(),
            dc: OnePole::new(),
            bright: 1.0,
            push: 1.0,
            gain: 1.0,
            fade: Fade::new(),
        }
    }

    #[inline]
    fn tick(&mut self, sr_inv: f32, p: &Stage) -> f32 {
        let freq = self.f0 + self.f_span * self.pitch_env + self.f_snap * self.snap_env;
        self.pitch_env *= self.pitch_coef;
        self.snap_env *= self.snap_coef;
        // Read the phase before advancing it so the first sample is exactly 0.
        let amp = self.amp.tick();
        let sine = fast_sin(self.phase) * amp;
        self.phase += (freq * sr_inv).min(0.45);
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        // Harder hits drive the shaper harder (more harmonics, not only level).
        let body = lerp(
            sine,
            shape(sine * self.push, p.drive_g, p.drive_norm),
            p.drive_mix,
        );

        // Click: noise band plus a short, quickly falling tone burst. Both
        // fade in over about 0.3 ms so the hit starts from zero.
        let atk = self.click_atk;
        self.click_atk = (atk + self.click_atk_inc).min(1.0);
        let noise = self.band.tick(self.noise.white()).band_unity * self.click_env;
        self.click_env *= self.click_coef;
        self.burst_phase += self.burst_dt;
        if self.burst_phase >= 1.0 {
            self.burst_phase -= 1.0;
        }
        let burst = fast_sin(self.burst_phase) * self.burst_env;
        self.burst_env *= self.burst_coef;
        let click = (noise * 0.7 + burst * 0.5) * self.click_gain * atk;

        let y = self.lp.low(body + click);
        // Final soft clip pushes the level up without exceeding full scale.
        let y = lerp(y, soft_clip(y, p.clip_g, p.clip_norm), p.clip_mix);
        let y = self.dc.high(y);
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

/// Block-level (smoothed) shaping settings shared by every voice.
#[derive(Clone, Copy)]
struct Stage {
    drive_g: f32,
    drive_norm: f32,
    drive_mix: f32,
    clip_g: f32,
    clip_norm: f32,
    clip_mix: f32,
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
    snap: f32,
    sweep_time: f32,
    decay: f32,
    click: f32,
    click_tone: f32,
    vel_sens: f32,
    // Smoothed parameters.
    drive: Ramp,
    clip: Ramp,
    tone: Ramp,
    level: Ramp,
    // Control-rate values.
    stage: Stage,
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
            pitch: 52.0,
            sweep: 22.0,
            snap: 8.0,
            sweep_time: 0.032,
            decay: 0.42,
            click: 0.65,
            click_tone: 3_200.0,
            vel_sens: 0.6,
            drive: Ramp::new(0.3),
            clip: Ramp::new(0.25),
            tone: Ramp::new(9_000.0f32.ln()),
            level: Ramp::new(0.0),
            stage: Stage {
                drive_g: 1.0,
                drive_norm: 1.0,
                drive_mix: 0.0,
                clip_g: 1.0,
                clip_norm: 1.0,
                clip_mix: 0.0,
            },
            tone_hz: 9_000.0,
            level_gain: 1.0,
        }
    }

    /// Recomputes values derived from the smoothed parameters.
    fn derive(&mut self) {
        let drive = self.drive.get();
        let (g, n) = drive_curve(drive);
        self.stage.drive_g = g;
        self.stage.drive_norm = n;
        // At zero drive the body is a pure sine; the shaper fades in fast.
        self.stage.drive_mix = (drive * 5.0).min(1.0);
        let clip = self.clip.get();
        // Up to about +10 dB into the clipper, normalized to unity at full scale.
        self.stage.clip_g = 1.0 + 2.2 * clip;
        self.stage.clip_norm = 1.0 / tanh_approx(self.stage.clip_g);
        self.stage.clip_mix = (clip * 5.0).min(1.0);
        self.tone_hz = self.tone.get().exp();
        self.level_gain = db_to_gain(self.level.get()) * 0.85;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.lp.set_cutoff(self.tone_hz * v.bright, self.sr);
        }
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.drive.tick(k);
        self.clip.tick(k);
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
            SNAP => self.snap = v,
            SWEEP_TIME => self.sweep_time = v,
            DECAY => self.decay = v,
            DRIVE => self.drive.set(v, immediate),
            CLIP => self.clip.set(v, immediate),
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
        let snap_ratio = st_ratio(self.snap * lerp(0.7, 1.0, velocity));
        let f_span = (f0 * (sweep_ratio - 1.0)).min(0.4 * sr - f0).max(0.0);
        let f_snap = (f0 * sweep_ratio * (snap_ratio - 1.0))
            .min(0.4 * sr - f0 - f_span)
            .max(0.0);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.phase = 0.0;
        v.f0 = f0;
        v.f_span = f_span;
        v.f_snap = f_snap;
        v.pitch_env = 1.0;
        v.pitch_coef = (-1.0 / (self.sweep_time * sr)).exp();
        v.snap_env = 1.0;
        v.snap_coef = (-1.0 / (0.004 * sr)).exp();
        v.amp.set_t60(self.decay, sr);
        v.amp.trigger(1.0, 0.000_4, sr);
        v.click_env = 1.0;
        v.click_coef = (-1.0 / (0.002 * sr)).exp();
        v.click_gain = self.click * 0.9 * lerp(0.35, 1.0, velocity);
        v.click_atk = 0.0;
        v.click_atk_inc = 1.0 / (0.000_3 * sr);
        v.band
            .set(self.click_tone * lerp(0.8, 1.15, velocity), 0.55, sr);
        v.band.reset();
        v.burst_phase = 0.0;
        v.burst_dt = (0.55 * self.click_tone / sr).min(0.4);
        v.burst_env = 1.0;
        v.burst_coef = (-1.0 / (0.001_6 * sr)).exp();
        v.noise.reseed(self.hits);
        v.bright = lerp(0.3, 1.0, velocity.sqrt());
        v.push = lerp(0.7, 1.0, velocity);
        v.lp.set_cutoff(self.tone_hz * v.bright, sr);
        v.lp.reset();
        v.dc.set_cutoff(18.0, sr);
        v.dc.reset();
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
                out += v.tick(sr_inv, &self.stage);
            }
            left[n] = out * self.level_gain;
            right[n] = out * self.level_gain;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
