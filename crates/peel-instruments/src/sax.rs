//! `peel.wind.sax`: a saxophone built as a source-filter model.
//!
//! The reed is a band-limited saw blended with a narrow pulse, so every
//! harmonic is present like in a conical bore. A breath envelope drives
//! everything: pressure opens the brightness (a low-pass that follows the
//! fundamental), feeds a soft reed-edge saturation and sets the level, so
//! soft notes are dark and loud notes are brassy. Three resonances
//! (about 500 Hz, 1.5 kHz and 2.7 kHz) give the body of the horn, and shaped
//! breath noise, partly modulated by the tone itself, sits on top.
//! Vibrato is delayed and fades in, the attack scoops up into pitch, and
//! growl is a 20 to 120 Hz amplitude and pitch modulation of the reed.
//!
//! `Mono legato` plays one voice with last-note priority: overlapping notes
//! only change pitch (with optional portamento) and breath pressure.
//! `Poly` plays up to four independent voices.

use peel_dsp::biquad::{Biquad, Coeffs, DcBlocker, FilterKind, OnePoleLowpass};
use peel_dsp::envelope::Adsr;
use peel_dsp::filter::tanh_approx;
use peel_dsp::noise::Noise;
use peel_dsp::osc::{Waveform, eval, fast_sin};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, lerp, ramp_coef,
};

const MODE: ParamId = 1;
const GLIDE: ParamId = 2;
const NOISE: ParamId = 10;
const B_ATTACK: ParamId = 11;
const B_RELEASE: ParamId = 12;
const SCOOP: ParamId = 13;
const BRIGHT: ParamId = 20;
const REED: ParamId = 21;
const EDGE: ParamId = 22;
const BODY: ParamId = 23;
const VIB_DEPTH: ParamId = 30;
const VIB_RATE: ParamId = 31;
const VIB_DELAY: ParamId = 32;
const GROWL: ParamId = 33;
const GROWL_RATE: ParamId = 34;
const LEVEL: ParamId = 40;
const VELOCITY: ParamId = 41;

const MODES: &[&str] = &["Mono legato", "Poly"];

static PARAMS: [ParamDescriptor; 17] = [
    ParamDescriptor::choice(MODE, "voice.mode", "Mode", "Voice", MODES, 0),
    ParamDescriptor::power(
        GLIDE,
        "voice.glide",
        "Glide",
        "Voice",
        0.0,
        1.0,
        0.0,
        Unit::Seconds,
        3.0,
    ),
    ParamDescriptor::linear(
        NOISE,
        "breath.noise",
        "Noise",
        "Breath",
        0.0,
        1.0,
        0.16,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        B_ATTACK,
        "breath.attack",
        "Attack",
        "Breath",
        0.005,
        0.6,
        0.05,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        B_RELEASE,
        "breath.release",
        "Release",
        "Breath",
        0.02,
        1.5,
        0.12,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        SCOOP,
        "breath.scoop",
        "Scoop",
        "Breath",
        0.0,
        1.0,
        0.35,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BRIGHT,
        "tone.bright",
        "Brightness",
        "Tone",
        0.0,
        1.0,
        0.55,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        REED,
        "tone.reed",
        "Reed",
        "Tone",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        EDGE,
        "tone.edge",
        "Edge",
        "Tone",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        BODY,
        "tone.body",
        "Body",
        "Tone",
        0.7,
        1.4,
        1.0,
        Unit::Ratio,
    ),
    ParamDescriptor::linear(
        VIB_DEPTH,
        "vibrato.depth",
        "Vibrato",
        "Expression",
        0.0,
        100.0,
        25.0,
        Unit::Cents,
    ),
    ParamDescriptor::linear(
        VIB_RATE,
        "vibrato.rate",
        "Vib Rate",
        "Expression",
        2.0,
        9.0,
        5.3,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        VIB_DELAY,
        "vibrato.delay",
        "Vib Delay",
        "Expression",
        0.0,
        2.0,
        0.35,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        GROWL,
        "growl.amount",
        "Growl",
        "Expression",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        GROWL_RATE,
        "growl.rate",
        "Growl Rate",
        "Expression",
        20.0,
        120.0,
        70.0,
        Unit::Hertz,
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
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.wind.sax",
    name: "Sax",
    description: "Saxophone: reed and breath model with vibrato, growl and legato",
    category: "Wind",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Alto",
        params: &[
            ("voice.mode", 0.0),
            ("voice.glide", 0.0),
            ("breath.noise", 0.16),
            ("breath.attack", 0.05),
            ("breath.release", 0.12),
            ("breath.scoop", 0.35),
            ("tone.bright", 0.55),
            ("tone.reed", 0.4),
            ("tone.edge", 0.3),
            ("tone.body", 1.0),
            ("vibrato.depth", 25.0),
            ("vibrato.rate", 5.3),
            ("vibrato.delay", 0.35),
            ("growl.amount", 0.0),
            ("growl.rate", 70.0),
            ("out.level", 0.0),
            ("out.velocity", 0.7),
        ],
    },
    Preset {
        name: "Tenor",
        params: &[
            ("breath.noise", 0.18),
            ("breath.attack", 0.065),
            ("breath.release", 0.15),
            ("tone.bright", 0.4),
            ("tone.reed", 0.35),
            ("tone.edge", 0.35),
            ("tone.body", 0.8),
            ("vibrato.depth", 22.0),
            ("vibrato.rate", 4.9),
        ],
    },
    Preset {
        name: "Soprano",
        params: &[
            ("breath.noise", 0.1),
            ("breath.attack", 0.035),
            ("breath.release", 0.09),
            ("breath.scoop", 0.25),
            ("tone.bright", 0.75),
            ("tone.reed", 0.55),
            ("tone.edge", 0.4),
            ("tone.body", 1.3),
            ("vibrato.depth", 30.0),
            ("vibrato.rate", 6.0),
        ],
    },
    Preset {
        name: "Breathy",
        params: &[
            ("breath.noise", 0.75),
            ("breath.attack", 0.12),
            ("breath.release", 0.25),
            ("breath.scoop", 0.5),
            ("tone.bright", 0.3),
            ("tone.reed", 0.25),
            ("tone.edge", 0.05),
            ("vibrato.depth", 18.0),
            ("vibrato.delay", 0.5),
            ("out.velocity", 0.85),
        ],
    },
    Preset {
        name: "Growl",
        params: &[
            ("voice.glide", 0.05),
            ("breath.noise", 0.25),
            ("breath.attack", 0.02),
            ("breath.scoop", 0.5),
            ("tone.bright", 0.8),
            ("tone.reed", 0.7),
            ("tone.edge", 0.8),
            ("tone.body", 0.9),
            ("vibrato.depth", 15.0),
            ("vibrato.delay", 0.6),
            ("growl.amount", 0.65),
            ("growl.rate", 80.0),
            ("out.velocity", 0.8),
        ],
    },
    Preset {
        name: "Section",
        params: &[
            ("voice.mode", 1.0),
            ("breath.noise", 0.12),
            ("breath.attack", 0.07),
            ("breath.release", 0.2),
            ("tone.bright", 0.5),
            ("tone.reed", 0.4),
            ("tone.edge", 0.25),
            ("vibrato.depth", 20.0),
            ("vibrato.delay", 0.5),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Sax::new()))
}

const POLYPHONY: usize = 4;
const CAPACITY: usize = 6;
const STACK: usize = 16;
const DRIFT_HZ: f32 = 5.0;

/// Resonances of the horn: frequency (Hz), gain (dB), Q.
const FORMANTS: [(f32, f32, f32); 3] = [(520.0, 5.0, 1.8), (1500.0, 4.0, 2.2), (2700.0, 3.0, 2.0)];
const NOISE_HZ: f32 = 2200.0;
/// Overall scale that puts a full-velocity mid-register note near -8 dBFS.
const MASTER: f32 = 0.42;

/// Settings shared by all voices, refreshed once per control interval.
#[derive(Clone, Copy)]
struct Shared {
    sr: f32,
    ramp_k: f32,
    glide_k: f32,
    scoop_decay: f32,
    drift_scale: f32,
    bright: f32,
    reed: f32,
    edge: f32,
    noise: f32,
    growl: f32,
    growl_inc: f32,
    vib_depth: f32,
    vib_rate: f32,
    vib_delay: f32,
    scoop: f32,
    out_gain: f32,
}

struct SaxVoice {
    active: bool,
    gate: bool,
    env: Adsr,
    fade: Fade,
    cur_key: f32,
    target_key: f32,
    glide_on: bool,
    press: f32,
    press_target: f32,
    age: f32,
    scoop_env: f32,
    phase: f32,
    dt: f32,
    pw: f32,
    vib_phase: f32,
    growl_phase: f32,
    drift: OnePoleLowpass,
    rng: Noise,
    lp: Biquad,
    nbp: Biquad,
    form: [Biquad; 3],
}

/// Breath envelope times captured at note start.
struct Breath {
    attack: f32,
    release: f32,
}

impl SaxVoice {
    fn new(sr: f32, seed: u32) -> SaxVoice {
        SaxVoice {
            active: false,
            gate: false,
            env: Adsr::new(sr),
            fade: Fade::new(),
            cur_key: 60.0,
            target_key: 60.0,
            glide_on: false,
            press: 0.0,
            press_target: 0.0,
            age: 0.0,
            scoop_env: 0.0,
            phase: 0.0,
            dt: 0.0,
            pw: 0.4,
            vib_phase: 0.0,
            growl_phase: 0.0,
            drift: OnePoleLowpass::new(sr / CTRL as f32, DRIFT_HZ),
            rng: Noise::new(seed),
            lp: Biquad::default(),
            nbp: Biquad::default(),
            form: [Biquad::default(); 3],
        }
    }

    fn clear_state(&mut self) {
        self.lp.reset();
        self.nbp.reset();
        for f in &mut self.form {
            f.reset();
        }
        self.drift.reset();
        self.phase = 0.0;
        self.vib_phase = 0.0;
        self.growl_phase = 0.0;
    }

    /// Starts a note. `retrigger` restarts the breath envelope, vibrato
    /// timing and scoop; otherwise only pitch and pressure change.
    fn start(&mut self, key: u8, velocity: f32, vel_sens: f32, retrigger: bool, b: &Breath) {
        let vel_press = (1.0 - vel_sens) * 0.85 + vel_sens * (0.25 + 0.75 * velocity);
        self.press_target = vel_press;
        self.target_key = key as f32;
        if retrigger {
            if !self.active {
                self.clear_state();
                self.cur_key = key as f32;
                self.press = vel_press;
            }
            self.env.set(b.attack, 0.15, 0.88, b.release, false);
            self.env.note_on();
            self.age = 0.0;
            self.scoop_env = 1.0;
        }
        self.fade.reset();
        self.gate = true;
        self.active = true;
    }

    fn release(&mut self) {
        self.gate = false;
        self.env.note_off();
    }

    fn control(&mut self, sh: &Shared) {
        let dt = CTRL as f32 / sh.sr;
        self.age += dt;
        if self.glide_on {
            self.cur_key += (self.target_key - self.cur_key) * sh.glide_k;
        } else {
            self.cur_key = self.target_key;
        }
        self.press += (self.press_target - self.press) * sh.ramp_k;

        self.vib_phase += sh.vib_rate * dt;
        self.vib_phase -= self.vib_phase.floor();
        let f = ((self.age - sh.vib_delay) / 0.6).clamp(0.0, 1.0);
        let vib = fast_sin(self.vib_phase) * sh.vib_depth * 0.01 * f * f * (3.0 - 2.0 * f);
        let drift = self.drift.process(self.rng.white()) * sh.drift_scale;
        self.scoop_env *= sh.scoop_decay;
        let growl_fm = sh.growl * 0.12 * fast_sin(self.growl_phase);
        let key = self.cur_key + vib + drift * 0.05 + growl_fm - sh.scoop * 0.8 * self.scoop_env;
        self.dt = (key_to_hz(key) / sh.sr).clamp(1.0e-5, 0.45);

        let pe = (self.env.level() * self.press).clamp(0.0, 1.0);
        self.pw = lerp(0.45, 0.3, pe);
        let ratio = (2.0 + 24.0 * sh.bright * pe.powf(1.3)) * (1.0 + 0.4 * sh.growl);
        let cutoff = (self.dt * sh.sr * ratio).clamp(400.0, 0.45 * sh.sr);
        self.lp.coeffs = Coeffs::design(FilterKind::LowPass, sh.sr, cutoff, 0.65, 0.0);
    }

    #[inline]
    fn tick(&mut self, sh: &Shared) -> f32 {
        let e = self.env.tick();
        let pe = (e * self.press).clamp(0.0, 1.0);
        let amp = pe * pe.sqrt();

        // Reed: saw blended with a narrow pulse (the difference of two saws).
        let a = eval(Waveform::Saw, self.phase, self.dt, 0.5);
        let mut p2 = self.phase + self.pw;
        if p2 >= 1.0 {
            p2 -= 1.0;
        }
        let b = eval(Waveform::Saw, p2, self.dt, 0.5);
        self.phase += self.dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let raw = lerp(a, 1.2 * (a - b), sh.reed * 0.8);
        let mut y = self.lp.process(raw);

        // Reed edge: pressure-dependent soft clipping.
        let k = (sh.edge + 0.4 * sh.growl).min(1.0) * (0.3 + 0.7 * pe);
        y += k * (tanh_approx(2.5 * y) * 0.5 - y);

        // Breath noise, partly following the tone's own envelope.
        let n = self.nbp.process(self.rng.white());
        let nz = sh.noise * 0.9 * (0.08 + 0.92 * pe) * (0.6 + 0.8 * y.abs().min(1.0));
        let mut x = y * (1.0 - 0.35 * sh.noise) + n * nz * (1.0 + sh.growl);

        // Growl: pulsed amplitude modulation of the whole reed signal.
        self.growl_phase += sh.growl_inc;
        self.growl_phase -= self.growl_phase.floor();
        let m = 0.5 - 0.5 * fast_sin(self.growl_phase);
        x *= 1.0 - sh.growl * 0.9 * m * m;

        for f in &mut self.form {
            x = f.process(x);
        }
        let out = x * amp * self.fade.tick() * sh.out_gain;
        if self.env.is_idle() || self.fade.is_done() {
            self.active = false;
        }
        out
    }
}

impl PoolVoice for SaxVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.env.level() * self.press
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
        self.gate = false;
        self.env.reset();
    }
}

#[derive(Clone, Copy)]
struct Held {
    id: u32,
    key: u8,
}

pub(crate) struct Sax {
    sr: f32,
    ctrl: usize,
    pool: Pool<SaxVoice>,
    dc: DcBlocker,
    sh: Shared,
    stack: [Held; STACK],
    stack_len: usize,
    // Unsmoothed settings.
    mono: bool,
    attack: f32,
    release: f32,
    vel_sens: f32,
    glide_time: f32,
    // Smoothed settings.
    noise: Ramp,
    bright: Ramp,
    reed: Ramp,
    edge: Ramp,
    body: Ramp,
    vib_depth: Ramp,
    vib_rate: Ramp,
    growl: Ramp,
    growl_rate: Ramp,
    scoop: Ramp,
    level: Ramp,
    formant_body: f32,
}

impl Sax {
    pub(crate) fn new() -> Sax {
        let sr = 48_000.0;
        Sax {
            sr,
            ctrl: 0,
            pool: Self::make_pool(sr),
            dc: DcBlocker::new(sr, 20.0),
            sh: Self::make_shared(sr),
            stack: [Held { id: 0, key: 0 }; STACK],
            stack_len: 0,
            mono: true,
            attack: 0.05,
            release: 0.12,
            vel_sens: 0.7,
            glide_time: 0.0,
            noise: Ramp::new(0.16),
            bright: Ramp::new(0.55),
            reed: Ramp::new(0.4),
            edge: Ramp::new(0.3),
            body: Ramp::new(1.0),
            vib_depth: Ramp::new(25.0),
            vib_rate: Ramp::new(5.3),
            growl: Ramp::new(0.0),
            growl_rate: Ramp::new(70.0),
            scoop: Ramp::new(0.35),
            level: Ramp::new(0.0),
            formant_body: f32::NAN,
        }
    }

    fn make_pool(sr: f32) -> Pool<SaxVoice> {
        let seed = std::cell::Cell::new(1u32);
        Pool::new(CAPACITY, POLYPHONY, StealPolicy::Quietest, sr, || {
            let s = seed.get();
            seed.set(s + 1);
            SaxVoice::new(sr, s)
        })
    }

    fn make_shared(sr: f32) -> Shared {
        let a = 1.0 - (-std::f32::consts::TAU * DRIFT_HZ * CTRL as f32 / sr).exp();
        // Normalizes the low-passed uniform noise to unit deviation.
        let drift_scale = 1.0 / (a / (2.0 - a) / 3.0).sqrt();
        Shared {
            sr,
            ramp_k: ramp_coef(sr),
            glide_k: 1.0,
            scoop_decay: (-(CTRL as f32) / (0.06 * sr)).exp(),
            drift_scale,
            bright: 0.55,
            reed: 0.4,
            edge: 0.3,
            noise: 0.16,
            growl: 0.0,
            growl_inc: 70.0 / sr,
            vib_depth: 25.0,
            vib_rate: 5.3,
            vib_delay: 0.35,
            scoop: 0.35,
            out_gain: MASTER,
        }
    }

    fn breath(&self) -> Breath {
        Breath {
            attack: self.attack,
            release: self.release,
        }
    }

    /// Copies the smoothed settings into the shared block, advancing each
    /// ramp by `k`.
    fn refresh(&mut self, k: f32) {
        let sh = &mut self.sh;
        sh.noise = self.noise.tick(k);
        sh.bright = self.bright.tick(k);
        sh.reed = self.reed.tick(k);
        sh.edge = self.edge.tick(k);
        sh.vib_depth = self.vib_depth.tick(k);
        sh.vib_rate = self.vib_rate.tick(k);
        sh.growl = self.growl.tick(k);
        sh.growl_inc = self.growl_rate.tick(k) / self.sr;
        sh.scoop = self.scoop.tick(k);
        sh.out_gain = MASTER * db_to_gain(self.level.tick(k));
        self.body.tick(k);
    }

    fn set_formants(&mut self) {
        let body = self.body.get();
        self.formant_body = body;
        let sr = self.sr;
        let mut co = [Coeffs::IDENTITY; 3];
        for (c, &(hz, db, q)) in co.iter_mut().zip(FORMANTS.iter()) {
            *c = Coeffs::design(FilterKind::Peak, sr, hz * body, q, db);
        }
        let noise = Coeffs::design(FilterKind::BandPass, sr, NOISE_HZ, 0.45, 0.0);
        for v in &mut self.pool.voices {
            for (f, c) in v.form.iter_mut().zip(co.iter()) {
                f.coeffs = *c;
            }
            v.nbp.coeffs = noise;
        }
    }

    fn control(&mut self) {
        self.refresh(self.sh.ramp_k);
        if self.formant_body.is_nan() || (self.body.get() - self.formant_body).abs() > 1.0e-4 {
            self.set_formants();
        }
        let sh = self.sh;
        for v in &mut self.pool.voices {
            if v.active {
                v.control(&sh);
            }
        }
    }

    fn update_glide(&mut self) {
        self.sh.glide_k = if self.glide_time <= 0.001 {
            1.0
        } else {
            1.0 - (-3.0 * CTRL as f32 / (self.glide_time * self.sr)).exp()
        };
    }

    fn release_everything(&mut self) {
        self.stack_len = 0;
        for v in &mut self.pool.voices {
            if v.gate {
                v.release();
            }
        }
        self.pool.release_all(|_| {});
    }
}

impl Synth for Sax {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.pool = Self::make_pool(sample_rate);
        self.dc = DcBlocker::new(sample_rate, 20.0);
        let delay = self.sh.vib_delay;
        self.sh = Self::make_shared(sample_rate);
        self.sh.vib_delay = delay;
        self.stack_len = 0;
        self.ctrl = 0;
        self.formant_body = f32::NAN;
        self.update_glide();
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        for v in &mut self.pool.voices {
            v.clear_state();
        }
        self.stack_len = 0;
        self.dc.reset();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            MODE => {
                let mono = v < 0.5;
                if mono != self.mono {
                    self.mono = mono;
                    self.release_everything();
                }
            }
            GLIDE => {
                self.glide_time = v;
                self.update_glide();
            }
            NOISE => self.noise.set(v, imm),
            B_ATTACK => self.attack = v,
            B_RELEASE => self.release = v,
            SCOOP => self.scoop.set(v, imm),
            BRIGHT => self.bright.set(v, imm),
            REED => self.reed.set(v, imm),
            EDGE => self.edge.set(v, imm),
            BODY => self.body.set(v, imm),
            VIB_DEPTH => self.vib_depth.set(v, imm),
            VIB_RATE => self.vib_rate.set(v, imm),
            VIB_DELAY => self.sh.vib_delay = v,
            GROWL => self.growl.set(v, imm),
            GROWL_RATE => self.growl_rate.set(v, imm),
            LEVEL => self.level.set(v, imm),
            VELOCITY => self.vel_sens = v,
            _ => {}
        }
        if imm {
            self.refresh(1.0);
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let breath = self.breath();
        let sens = self.vel_sens;
        if self.mono {
            let overlapping = self.stack_len > 0;
            if self.stack_len == STACK {
                self.stack.copy_within(1.., 0);
                self.stack_len -= 1;
            }
            self.stack[self.stack_len] = Held { id, key };
            self.stack_len += 1;
            let glide = self.sh.glide_k < 1.0;
            let v = &mut self.pool.voices[0];
            v.glide_on = glide && v.active;
            v.start(key, velocity, sens, !overlapping, &breath);
        } else {
            let slot = self.pool.begin(key, id);
            let v = &mut self.pool.voices[slot];
            v.active = false;
            v.glide_on = false;
            v.start(key, velocity, sens, true, &breath);
        }
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        if self.mono {
            let Some(pos) = self.stack[..self.stack_len].iter().position(|h| h.id == id) else {
                return;
            };
            let was_top = pos + 1 == self.stack_len;
            self.stack.copy_within(pos + 1..self.stack_len, pos);
            self.stack_len -= 1;
            let glide = self.sh.glide_k < 1.0;
            let v = &mut self.pool.voices[0];
            if self.stack_len == 0 {
                v.release();
            } else if was_top {
                // Fall back to the previous held note without retriggering.
                let back = self.stack[self.stack_len - 1];
                v.glide_on = glide;
                v.target_key = back.key as f32;
            }
        } else {
            self.pool.release_id(id, SaxVoice::release);
        }
    }

    fn all_notes_off(&mut self) {
        self.release_everything();
    }

    fn all_sound_off(&mut self) {
        self.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        for n in 0..left.len().min(right.len()) {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let mut sum = 0.0;
            let mut any = false;
            for v in &mut self.pool.voices {
                if v.active {
                    any = true;
                    sum += v.tick(&self.sh);
                }
            }
            let out = if any {
                self.dc.process(sum)
            } else {
                self.dc.reset();
                0.0
            };
            left[n] = out;
            right[n] = out;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
