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

use crate::acoustic::{Adaa, HitRng, Mode, Pulse, analog_slot};
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
const ACOUSTIC: ParamId = 11;
const CIRCUIT: ParamId = 12;

/// Frequency ratios of the first three membrane modes.
const MODES: [f32; 3] = [1.0, 1.593, 2.136];

static PARAMS: [ParamDescriptor; 12] = [
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
    ParamDescriptor::toggle(ACOUSTIC, "body.acoustic", "Acoustic", "Body", false),
    ParamDescriptor::toggle(CIRCUIT, "body.circuit", "Circuit", "Body", false),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.drum.tom",
    name: "Tom",
    description: "Tom: electronic, drum-machine circuit or acoustic model",
    category: "Drums",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 12] = [
    Preset {
        name: "Floor",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
    Preset {
        name: "Acoustic Rack",
        params: &[
            ("body.acoustic", 1.0),
            ("body.circuit", 0.0),
            ("pitch.freq", 165.0),
            ("pitch.sweep", 2.0),
            ("pitch.sweep_time", 0.06),
            ("body.decay", 0.55),
            ("body.overtone", 0.5),
            ("body.stick", 0.45),
            ("body.tone", 7_000.0),
            ("body.drive", 0.05),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Acoustic Floor",
        params: &[
            ("body.acoustic", 1.0),
            ("body.circuit", 0.0),
            ("pitch.freq", 92.0),
            ("pitch.sweep", 2.5),
            ("pitch.sweep_time", 0.08),
            ("body.decay", 0.85),
            ("body.overtone", 0.45),
            ("body.stick", 0.4),
            ("body.tone", 5_500.0),
            ("body.drive", 0.08),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "808 Tom",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 1.0),
            ("pitch.freq", 120.0),
            ("pitch.sweep", 5.0),
            ("pitch.sweep_time", 0.12),
            ("body.decay", 0.6),
            ("body.overtone", 0.1),
            ("body.stick", 0.2),
            ("body.tone", 4_000.0),
            ("body.drive", 0.1),
            ("out.velocity", 0.6),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "909 Tom",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 1.0),
            ("pitch.freq", 140.0),
            ("pitch.sweep", 10.0),
            ("pitch.sweep_time", 0.06),
            ("body.decay", 0.4),
            ("body.overtone", 0.35),
            ("body.stick", 0.5),
            ("body.tone", 7_000.0),
            ("body.drive", 0.3),
            ("out.velocity", 0.65),
            ("out.level", 0.0),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Tom::new()))
}

/// Acoustic tom modes relative to the fundamental. Air in the shell pulls
/// a tom's head modes closer together than an ideal membrane's; the second
/// entry is the resonant head a little sharp of the batter, which gives the
/// slow beating of a real tom's sustain.
const AT_RATIO: [f32; 8] = [1.0, 1.012, 1.5, 1.98, 2.44, 2.86, 3.3, 3.74];
const AT_DIAM: [u8; 8] = [0, 0, 1, 2, 0, 3, 1, 4];
const AT_GAIN: [f32; 8] = [1.0, 0.4, 0.62, 0.42, 0.3, 0.24, 0.18, 0.12];
const AT_DECAY: [f32; 8] = [1.0, 1.15, 0.6, 0.46, 0.42, 0.33, 0.3, 0.24];

/// The acoustic tom: a stick pulse driving batter and resonant head modes
/// with a tension glide, a stick click, and the same tone and drive stages
/// as the electronic tom.
#[derive(Clone, Copy)]
struct AcTomVoice {
    active: bool,
    modes: [Mode; 8],
    base: [f32; 8],
    weight: [f32; 8],
    pulse: Pulse,
    span: f32,
    pitch_env: f32,
    pitch_coef: f32,
    snap: f32,
    snap_env: f32,
    snap_coef: f32,
    noise: Noise,
    stick_band: Svf,
    stick: Decay,
    stick_gain: f32,
    lp: OnePole,
    bright: f32,
    push: f32,
    gain: f32,
    fade: Fade,
    xgain: f32,
    bend: f32,
    tone_r: f32,
}

impl AcTomVoice {
    fn new() -> AcTomVoice {
        AcTomVoice {
            active: false,
            modes: [Mode::new(); 8],
            base: [110.0; 8],
            weight: [0.0; 8],
            pulse: Pulse::new(),
            span: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            snap: 0.0,
            snap_env: 0.0,
            snap_coef: 0.0,
            noise: Noise::new(31),
            stick_band: Svf::new(),
            stick: Decay::new(),
            stick_gain: 0.0,
            lp: OnePole::new(),
            bright: 1.0,
            push: 1.0,
            gain: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            bend: 1.0,
            tone_r: 1.0,
        }
    }

    fn retune(&mut self, sr_inv: f32) {
        let m = (1.0 + self.span * self.pitch_env + self.snap * self.snap_env) * self.bend;
        for (md, &f) in self.modes.iter_mut().zip(self.base.iter()) {
            md.tune(f * m, sr_inv);
            md.flush();
        }
    }

    #[inline]
    fn tick(&mut self, drive_g: f32, drive_norm: f32, drive_mix: f32) -> f32 {
        self.pitch_env *= self.pitch_coef;
        self.snap_env *= self.snap_coef;
        let x = self.pulse.tick();
        let mut body = 0.0;
        for (m, &w) in self.modes.iter_mut().zip(self.weight.iter()) {
            body += m.tick(x * w);
        }
        let stick = self.stick_band.tick(self.noise.white()).band_unity
            * self.stick.tick()
            * self.stick_gain;
        let x = body * self.push + stick;
        let y = self
            .lp
            .low(lerp(x, shape(x, drive_g, drive_norm), drive_mix));
        let out = y * self.gain * self.fade.tick() * self.xgain;
        if self.fade.is_done()
            || (!self.pulse.busy()
                && self.modes[0].amp() < 1.0e-4
                && self.modes[1].amp() < 1.0e-4
                && self.stick.is_below(1.0e-4))
        {
            self.active = false;
        }
        out
    }
}

/// The circuit tom: a bridged-T style resonator (plus a weaker second
/// mode) pinged by a trigger pulse, with an accent-dependent pitch glide, a
/// short filtered-noise skin and anti-aliased drive. A retrigger on the
/// same key re-pings the ringing resonators.
#[derive(Clone, Copy)]
struct CkTomVoice {
    active: bool,
    modes: [Mode; 2],
    base: [f32; 2],
    weight: [f32; 2],
    pulse: Pulse,
    /// Glide of the pitch envelope back to the top after a retrigger in a
    /// ringing voice (0 when not gliding).
    pitch_rise: f32,
    span: f32,
    pitch_env: f32,
    pitch_coef: f32,
    snap: f32,
    snap_env: f32,
    snap_coef: f32,
    noise: Noise,
    skin_lp: OnePole,
    skin_hp: OnePole,
    skin: Decay,
    skin_gain: f32,
    drive: Adaa,
    lp: OnePole,
    bright: f32,
    push: f32,
    gain: f32,
    fade: Fade,
    xgain: f32,
    bend: f32,
    tone_r: f32,
}

impl CkTomVoice {
    fn new() -> CkTomVoice {
        CkTomVoice {
            active: false,
            modes: [Mode::new(); 2],
            base: [110.0; 2],
            weight: [0.0; 2],
            pulse: Pulse::new(),
            pitch_rise: 0.0,
            span: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            snap: 0.0,
            snap_env: 0.0,
            snap_coef: 0.0,
            noise: Noise::new(71),
            skin_lp: OnePole::new(),
            skin_hp: OnePole::new(),
            skin: Decay::new(),
            skin_gain: 0.0,
            drive: Adaa::new(),
            lp: OnePole::new(),
            bright: 1.0,
            push: 1.0,
            gain: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            bend: 1.0,
            tone_r: 1.0,
        }
    }

    fn retune(&mut self, sr_inv: f32) {
        let m = (1.0 + self.span * self.pitch_env + self.snap * self.snap_env) * self.bend;
        for (md, &f) in self.modes.iter_mut().zip(self.base.iter()) {
            md.tune(f * m, sr_inv);
            md.flush();
        }
    }

    #[inline]
    fn tick(&mut self, drive_g: f32, drive_norm: f32, drive_mix: f32) -> f32 {
        if self.pitch_rise > 0.0 {
            self.pitch_env += (1.0 - self.pitch_env) * self.pitch_rise;
            self.snap_env += (1.0 - self.snap_env) * self.pitch_rise;
            if self.pitch_env > 0.995 {
                self.pitch_rise = 0.0;
            }
        } else {
            self.pitch_env *= self.pitch_coef;
            self.snap_env *= self.snap_coef;
        }
        let x = self.pulse.tick();
        let body = self.modes[0].tick(x * self.weight[0]) + self.modes[1].tick(x * self.weight[1]);
        let n = self.skin_lp.low(self.noise.white());
        let skin = self.skin_hp.high(n) * self.skin.tick() * self.skin_gain;
        let x = body * self.push + skin;
        let y = self.lp.low(lerp(
            x,
            self.drive.tick(x * drive_g) * drive_norm,
            drive_mix,
        ));
        let out = y * self.gain * self.fade.tick() * self.xgain;
        if self.fade.is_done()
            || (!self.pulse.busy()
                && self.modes[0].amp() < 1.0e-4
                && self.modes[1].amp() < 1.0e-4
                && self.skin.is_below(1.0e-4))
        {
            self.active = false;
        }
        out
    }
}

impl PoolVoice for CkTomVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.modes[0].amp()
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

impl PoolVoice for AcTomVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.modes[0].amp()
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
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
    ac: Pool<AcTomVoice>,
    ck: Pool<CkTomVoice>,
    acoustic: bool,
    circuit: bool,
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
            ac: Pool::new(5, 2, StealPolicy::Oldest, 48_000.0, AcTomVoice::new),
            ck: Pool::new(5, 2, StealPolicy::Oldest, 48_000.0, CkTomVoice::new),
            acoustic: false,
            circuit: false,
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
        for v in self.ac.voices.iter_mut().filter(|v| v.active) {
            v.lp.set_cutoff(self.tone_hz * v.bright * v.tone_r, self.sr);
        }
        for v in self.ck.voices.iter_mut().filter(|v| v.active) {
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
        let sr_inv = 1.0 / self.sr;
        self.ac.for_active_id(|v, key, id| {
            let x = ex.voice(id, key);
            v.xgain = x.gain();
            v.bend = x.ratio;
            v.tone_r = x.tone_ratio(TIMBRE_OCTAVES);
            v.retune(sr_inv);
        });
        self.ck.for_active_id(|v, key, id| {
            let x = ex.voice(id, key);
            v.xgain = x.gain();
            v.bend = x.ratio;
            v.tone_r = x.tone_ratio(TIMBRE_OCTAVES);
            v.retune(sr_inv);
        });
    }

    /// Starts a circuit hit, re-pinging a voice already ringing on the key.
    fn ck_note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let sr = self.sr;
        let (slot, warm) = analog_slot(&mut self.ck, key, id, false, |v| v.fade.is_fading());
        self.hits = self.hits.wrapping_add(1);
        let f0 = (self.pitch * st_ratio(key_offset(key))).clamp(20.0, 0.08 * sr);
        let tone = self.tone_hz;
        let hits = self.hits;
        let x = self.expr.voice(id, key);
        let v = &mut self.ck.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        v.bend = x.ratio;
        v.tone_r = x.tone_ratio(TIMBRE_OCTAVES);
        v.base = [f0, f0 * 1.6];
        v.weight = [0.85, 0.5 * self.overtone];
        let decays = [self.decay, self.decay * 0.45];
        for ((m, &f), &d) in v.modes.iter_mut().zip(v.base.iter()).zip(decays.iter()) {
            if warm {
                m.retarget(d.max(0.02), sr);
            } else {
                m.start(f, d.max(0.02), sr);
            }
        }
        // Accent: harder hits start further above the resting pitch.
        v.span = st_ratio(self.sweep * lerp(0.6, 1.05, velocity)) - 1.0;
        if warm {
            v.pitch_rise = 1.0 - (-1.0 / (0.000_7 * sr)).exp();
        } else {
            v.pitch_env = 1.0;
            v.snap_env = 1.0;
            v.pitch_rise = 0.0;
        }
        v.pitch_coef = (-1.0 / (self.sweep_time * sr)).exp();
        v.snap = 0.04 * velocity;
        v.snap_coef = (-1.0 / (0.004 * sr)).exp();
        v.retune(1.0 / sr);
        v.pulse.start(0.000_3, sr);
        v.noise.reseed(hits.wrapping_add(1_300));
        v.skin_lp
            .set_cutoff((f0 * 30.0).clamp(1_500.0, 7_000.0), sr);
        v.skin_hp.set_cutoff(400.0, sr);
        v.skin.set_t60(0.05, sr);
        v.skin.trigger(1.0, 0.000_3, sr);
        v.skin_gain = self.stick * 0.6 * velocity * v.tone_r.sqrt();
        if !warm {
            v.skin_lp.reset();
            v.skin_hp.reset();
            v.skin.reset();
            v.drive.reset();
            v.lp.reset();
        }
        v.push = lerp(0.65, 1.1, velocity);
        v.bright = lerp(0.4, 1.0, velocity.sqrt());
        v.lp.set_cutoff(tone * v.bright * v.tone_r, sr);
        v.gain = vel_gain(velocity, self.vel_sens);
        v.fade.reset();
    }

    /// Starts an acoustic stroke.
    fn ac_note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.ac.begin(key, id);
        self.hits = self.hits.wrapping_add(1);
        let sr = self.sr;
        let mut rng = HitRng::new(self.hits, 0x7041);
        let f0 = (self.pitch * st_ratio(key_offset(key))).clamp(20.0, 0.08 * sr);
        let radius = 0.3 + 0.12 * rng.next();
        let contact = lerp(0.001_1, 0.000_4, velocity) * (1.0 + 0.1 * rng.next());
        let over = lerp(0.25, 1.75, self.overtone);
        let tone = self.tone_hz;
        let x = self.expr.voice(id, key);
        let v = &mut self.ac.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        v.bend = x.ratio;
        v.tone_r = x.tone_ratio(TIMBRE_OCTAVES);
        for k in 0..8 {
            v.base[k] = f0 * AT_RATIO[k];
            let pos = if AT_DIAM[k] == 0 {
                1.2 - 0.9 * radius
            } else {
                (0.3 + 2.1 * radius).min(1.6)
            };
            let g = if k >= 2 {
                AT_GAIN[k] * over
            } else {
                AT_GAIN[k]
            };
            v.weight[k] = 0.62 * g * pos * (1.0 + 0.08 * rng.next());
            v.modes[k].start(v.base[k], (self.decay * AT_DECAY[k]).max(0.02), sr);
        }
        v.span = st_ratio(self.sweep * velocity.powf(1.5) * (1.0 + 0.1 * rng.next())) - 1.0;
        v.pitch_env = 1.0;
        v.pitch_coef = (-1.0 / (self.sweep_time * sr)).exp();
        v.snap = 0.03 * velocity * velocity;
        v.snap_env = 1.0;
        v.snap_coef = (-1.0 / (0.005 * sr)).exp();
        v.retune(1.0 / sr);
        v.pulse.start(contact, sr);
        v.noise.reseed(self.hits.wrapping_add(41));
        let stick_hz = (f0 * 14.0).clamp(1_500.0, 6_000.0) * (1.0 + 0.1 * rng.next());
        v.stick_band.set_q(stick_hz.min(0.4 * sr), 1.0, sr);
        v.stick_band.reset();
        v.stick.set_t60(0.014, sr);
        v.stick.trigger(1.0, 0.000_2, sr);
        v.stick_gain =
            self.stick * 1.1 * velocity.powf(1.3) * v.tone_r.sqrt() * (1.0 + 0.15 * rng.next());
        v.push = lerp(0.75, 1.0, velocity);
        v.bright = lerp(0.4, 1.0, velocity.sqrt());
        v.lp.set_cutoff(tone * v.bright * v.tone_r, sr);
        v.lp.reset();
        v.gain = vel_gain(velocity, self.vel_sens) * (1.0 + 0.03 * rng.next());
        v.fade.reset();
    }
}

impl Synth for Tom {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(5, 2, StealPolicy::Oldest, sample_rate, TomVoice::new);
        self.ac = Pool::new(5, 2, StealPolicy::Oldest, sample_rate, AcTomVoice::new);
        self.ck = Pool::new(5, 2, StealPolicy::Oldest, sample_rate, CkTomVoice::new);
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ac.kill_all();
        self.ck.kill_all();
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
            ACOUSTIC => self.acoustic = v >= 0.5,
            CIRCUIT => self.circuit = v >= 0.5,
            _ => {}
        }
        if immediate {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        if self.acoustic {
            self.ac_note_on(key, velocity, id);
            return;
        }
        if self.circuit {
            self.ck_note_on(key, velocity, id);
            return;
        }
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
        self.ac.kill_all();
        self.ck.kill_all();
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
            for v in self.ac.voices.iter_mut().filter(|v| v.active) {
                out += v.tick(self.drive_g, self.drive_norm, self.drive_mix);
            }
            for v in self.ck.voices.iter_mut().filter(|v| v.active) {
                out += v.tick(self.drive_g, self.drive_norm, self.drive_mix);
            }
            left[n] = out * self.level_gain;
            right[n] = out * self.level_gain;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent() && self.ac.is_silent() && self.ck.is_silent()
    }
}
