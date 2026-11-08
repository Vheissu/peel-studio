//! `peel.keys.grand`: a synthesized acoustic piano using modal synthesis.
//!
//! Each note is a set of one to three slightly detuned strings (as on a real
//! piano) and each string is a bank of damped two-pole resonators, one per
//! partial. Partials follow the stiff-string law `f_k = k f0 sqrt((1 + B k^2)
//! / (1 + B))` so high partials run sharp, the way a piano's do. Their
//! starting amplitudes come from a hammer model: a velocity-dependent low
//! pass, a strike-position comb (`|sin(pi k x0)|`) and a soundboard emphasis.
//! Higher partials decay faster, bass notes ring longer than treble notes, and
//! an extra prompt/after-sound envelope gives the familiar two-stage decay.
//! A hammer noise burst adds the thump of the attack, and releasing a key
//! drops a damper that closes a low-pass while it silences the string. The
//! sustain pedal is an automatable parameter.

use std::f32::consts::{PI, TAU};

use peel_dsp::filter::{OnePole, Svf};
use peel_dsp::noise::Noise;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz, pan_gains};
use peel_dsp::voice::{StealPolicy, VoiceState};
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{
    CTRL, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, lerp, ramp_coef,
};

const TONE: ParamId = 1;
const STIFFNESS: ParamId = 2;
const STRIKE: ParamId = 3;
const HAMMER: ParamId = 4;
const DYNAMICS: ParamId = 5;
const DETUNE: ParamId = 10;
const DECAY: ParamId = 11;
const AFTERSOUND: ParamId = 12;
const PEDAL: ParamId = 20;
const DAMPER: ParamId = 21;
const WIDTH: ParamId = 30;
const LEVEL: ParamId = 31;

static PARAMS: [ParamDescriptor; 12] = [
    ParamDescriptor::linear(
        TONE,
        "tone.brightness",
        "Tone",
        "Tone",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        STIFFNESS,
        "tone.stiffness",
        "Stiffness",
        "Tone",
        0.25,
        4.0,
        1.0,
        Unit::None,
    ),
    ParamDescriptor::linear(
        STRIKE,
        "tone.strike",
        "Strike Point",
        "Tone",
        0.06,
        0.25,
        0.125,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        HAMMER,
        "tone.hammer",
        "Hammer Noise",
        "Tone",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        DYNAMICS,
        "tone.dynamics",
        "Dynamics",
        "Tone",
        0.0,
        1.0,
        0.8,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        DETUNE,
        "string.detune",
        "Detune",
        "Strings",
        0.0,
        4.0,
        1.0,
        Unit::Cents,
    ),
    ParamDescriptor::log(
        DECAY,
        "string.decay",
        "Decay",
        "Strings",
        0.25,
        4.0,
        1.0,
        Unit::None,
    ),
    ParamDescriptor::linear(
        AFTERSOUND,
        "string.aftersound",
        "Aftersound",
        "Strings",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::toggle(PEDAL, "pedal.sustain", "Sustain Pedal", "Pedal", false),
    ParamDescriptor::log(
        DAMPER,
        "pedal.damper",
        "Damper Time",
        "Pedal",
        0.05,
        2.0,
        0.3,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        WIDTH,
        "out.width",
        "Width",
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
    type_id: "peel.keys.grand",
    name: "Grand",
    description: "Synthesized grand piano with inharmonic strings and hammer model",
    category: "Keys",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Natural",
        params: &[
            ("tone.brightness", 0.5),
            ("tone.stiffness", 1.0),
            ("tone.strike", 0.125),
            ("tone.hammer", 0.4),
            ("tone.dynamics", 0.8),
            ("string.detune", 1.0),
            ("string.decay", 1.0),
            ("string.aftersound", 0.5),
            ("pedal.sustain", 0.0),
            ("pedal.damper", 0.3),
            ("out.width", 0.6),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Bright",
        params: &[
            ("tone.brightness", 0.8),
            ("tone.hammer", 0.55),
            ("tone.strike", 0.11),
            ("string.decay", 0.9),
        ],
    },
    Preset {
        name: "Soft",
        params: &[
            ("tone.brightness", 0.2),
            ("tone.hammer", 0.2),
            ("tone.dynamics", 0.6),
            ("string.decay", 1.2),
            ("string.aftersound", 0.6),
            ("pedal.damper", 0.4),
        ],
    },
    Preset {
        name: "Close",
        params: &[
            ("tone.brightness", 0.65),
            ("tone.hammer", 0.7),
            ("tone.strike", 0.1),
            ("tone.dynamics", 0.9),
            ("string.decay", 0.8),
            ("string.aftersound", 0.35),
            ("out.width", 0.35),
        ],
    },
    Preset {
        name: "Upright",
        params: &[
            ("tone.brightness", 0.4),
            ("tone.stiffness", 1.8),
            ("tone.strike", 0.14),
            ("tone.hammer", 0.65),
            ("string.detune", 2.2),
            ("string.decay", 0.55),
            ("string.aftersound", 0.25),
            ("pedal.damper", 0.15),
            ("out.width", 0.3),
        ],
    },
    Preset {
        name: "Honky-Tonk",
        params: &[
            ("tone.brightness", 0.7),
            ("tone.stiffness", 2.5),
            ("tone.hammer", 0.8),
            ("string.detune", 3.8),
            ("string.decay", 0.6),
            ("string.aftersound", 0.2),
            ("pedal.damper", 0.12),
            ("out.width", 0.7),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Grand::new()))
}

const MAX_MODES: usize = 72;
const MAX_PARTIALS: usize = 40;
const MAX_STRINGS: usize = 3;
/// Mode amplitudes below this (about -90 dBFS) are inaudible.
const AUDIBLE: f32 = 3.0e-5;

/// A damped resonator `y[n] = a1 y[n-1] + a2 y[n-2]`.
#[derive(Clone, Copy, Default)]
struct Mode {
    a1: f32,
    a2: f32,
    s1: f32,
    s2: f32,
}

/// Note-independent settings snapshot used to start a voice.
#[derive(Clone, Copy)]
struct Settings {
    sr: f32,
    tone: f32,
    stiffness: f32,
    strike: f32,
    hammer: f32,
    dynamics: f32,
    detune: f32,
    decay: f32,
    after: f32,
    damper: f32,
    width: f32,
}

/// Inharmonicity coefficient of a string by key.
fn inharmonicity(key: f32) -> f32 {
    if key >= 60.0 {
        2.5e-4 * 2f32.powf((key - 60.0) / 12.0 * 1.2)
    } else {
        2.5e-4 * 2f32.powf((60.0 - key) / 12.0 * 0.15)
    }
}

/// Soundboard emphasis: rolls off the lowest octave and adds some presence.
fn body_response(f: f32) -> f32 {
    let hp = 1.0 / (1.0 + (110.0 / f).powi(2));
    let x = (f / 2_000.0).ln();
    hp * (1.0 + 0.35 * (-x * x / 0.72).exp())
}

#[derive(Clone, Copy)]
struct GrandVoice {
    active: bool,
    releasing: bool,
    pending_release: bool,
    modes: [Mode; MAX_MODES],
    n_str: usize,
    n_p: usize,
    n_live: usize,
    end_t: [u32; MAX_PARTIALS],
    age: u32,
    flush: u32,
    gl: [f32; MAX_STRINGS],
    gr: [f32; MAX_STRINGS],
    after: f32,
    pr_env: f32,
    pr_coef: f32,
    slow_env: f32,
    slow_coef: f32,
    dmp: f32,
    dmp_coef: f32,
    lp_l: OnePole,
    lp_r: OnePole,
    noise: Noise,
    noise_bp: Svf,
    noise_env: f32,
    noise_coef: f32,
    noise_gain: f32,
    fade: Fade,
}

impl GrandVoice {
    fn new() -> GrandVoice {
        GrandVoice {
            active: false,
            releasing: false,
            pending_release: false,
            modes: [Mode::default(); MAX_MODES],
            n_str: 1,
            n_p: 1,
            n_live: 1,
            end_t: [0; MAX_PARTIALS],
            age: 0,
            flush: 0,
            gl: [1.0; MAX_STRINGS],
            gr: [1.0; MAX_STRINGS],
            after: 0.4,
            pr_env: 1.0,
            pr_coef: 0.999,
            slow_env: 1.0,
            slow_coef: 0.9999,
            dmp: 1.0,
            dmp_coef: 1.0,
            lp_l: OnePole::new(),
            lp_r: OnePole::new(),
            noise: Noise::new(1),
            noise_bp: Svf::new(),
            noise_env: 0.0,
            noise_coef: 0.0,
            noise_gain: 0.0,
            fade: Fade::new(),
        }
    }

    fn start(&mut self, s: &Settings, key: u8, velocity: f32, seed: u32) {
        let sr = s.sr;
        let inv_sr = 1.0 / sr;
        let kf = key as f32;
        let f0 = key_to_hz(kf);
        let n_str = if key < 33 {
            1
        } else if key < 47 {
            2
        } else {
            3
        };
        let b = (inharmonicity(kf) * s.stiffness).min(0.06);
        let x0 = s.strike;
        let vel = velocity.clamp(0.0, 1.0);

        // Hammer low-pass corner: harder blows reach higher partials.
        let tone_scale = 0.35 + 1.3 * s.tone;
        let fc = 1_800.0 * tone_scale * (0.5 + 2.4 * vel.powf(1.2));
        let fc_ref = 1_800.0 * tone_scale * 1.8;
        let hammer = |f: f32, fc: f32| 1.0 / (1.0 + (f / fc).powi(2));

        let max_p = (MAX_MODES / n_str).min(MAX_PARTIALS);
        let mut amps = [0.0f32; MAX_PARTIALS];
        let mut freqs = [0.0f32; MAX_PARTIALS];
        let mut e_ref = 0.0f32;
        let mut n_p = 0;
        for k in 1..=max_p {
            let kk = k as f32;
            let fk = kk * f0 * ((1.0 + b * kk * kk) / (1.0 + b)).sqrt();
            if fk > 0.45 * sr {
                break;
            }
            let envelope = body_response(fk) / kk.powf(1.15);
            let comb = (PI * kk * x0).sin().abs();
            amps[k - 1] = envelope * hammer(fk, fc) * comb;
            let a_ref = envelope * hammer(fk, fc_ref) * comb;
            e_ref += a_ref * a_ref;
            freqs[k - 1] = fk;
            n_p = k;
            if k >= 4 && envelope * hammer(fk, fc) < 0.004 {
                break;
            }
        }
        let n_p = n_p.max(1);

        // Loudness: normalize on a fixed reference spectrum so that harder
        // hits are both louder and brighter.
        let level = lerp(0.65, 0.12 + 0.88 * vel.powf(1.6), s.dynamics);
        let norm = 0.075 / (e_ref * n_str as f32 * 0.5).sqrt().max(1.0e-6) * level;

        // Decay rates (1/s): slow for bass, fast for treble, with a frequency
        // dependent loss term.
        let t60 = (22.0 * 2f32.powf(-(kf - 48.0) / 18.0)).clamp(1.5, 40.0) * s.decay;
        let sigma0 = 6.908 / t60;
        let sigma_hf = 0.35;
        let n0 = (0.5 + 0.35 * (1.0 - vel)) * 0.001 * sr; // hammer contact half-length

        let mut rng = Noise::new(seed.wrapping_mul(2_654_435_761) ^ key as u32);
        self.n_str = n_str;
        self.n_p = n_p;
        for string in 0..n_str {
            let cents = match (n_str, string) {
                (1, _) => 0.0,
                (2, 0) => -0.5 * s.detune,
                (2, _) => 0.5 * s.detune,
                (_, 0) => 0.0,
                (_, 1) => s.detune,
                _ => -s.detune,
            };
            let ratio = (cents * (std::f32::consts::LN_2 / 1200.0)).exp();
            let var = 1.0 + 0.08 * rng.white();
            for k in 0..n_p {
                let fk = freqs[k] * ratio;
                let w = TAU * fk * inv_sr;
                let sigma = sigma0 + sigma_hf * (fk * 0.001).powf(1.5);
                let r = (-sigma * inv_sr).exp();
                let amp = amps[k] * norm * var;
                // The radiated sound is not a clean impulse: soundboard
                // dispersion scatters the phases of the upper partials, which
                // keeps the attack from being a spike.
                let scatter = rng.white() * 0.65 * PI * fk / (fk + 700.0);
                let y = |n: f32| amp * r.powf(n) * (w * (n - n0) + scatter).cos();
                self.modes[string * n_p + k] = Mode {
                    a1: 2.0 * r * w.cos(),
                    a2: -r * r,
                    s1: y(-1.0),
                    s2: y(-2.0),
                };
                if string == 0 {
                    let life = if amp > AUDIBLE {
                        (amp / AUDIBLE).ln() / sigma
                    } else {
                        0.0
                    };
                    self.end_t[k] = (life * sr) as u32;
                }
            }
        }
        self.n_live = n_p;
        while self.n_live > 1 && self.end_t[self.n_live - 1] == 0 {
            self.n_live -= 1;
        }

        // Stereo: strings fan out around the key's position on the keyboard.
        let spread: [f32; 3] = match n_str {
            1 => [0.0, 0.0, 0.0],
            2 => [-0.4, 0.4, 0.0],
            _ => [-0.5, 0.0, 0.5],
        };
        let key_pan = (kf - 60.0) / 48.0 * 0.35;
        for string in 0..MAX_STRINGS {
            let (l, r) = pan_gains(((spread[string] + key_pan) * s.width).clamp(-1.0, 1.0));
            self.gl[string] = l;
            self.gr[string] = r;
        }

        // Two-stage decay of the whole note.
        self.after = lerp(0.2, 0.8, s.after);
        self.pr_env = 1.0;
        let tau_prompt = (0.7 * 2f32.powf(-(kf - 60.0) / 30.0)).clamp(0.15, 2.0);
        self.pr_coef = (-1.0 / (tau_prompt * sr)).exp();
        self.slow_env = 1.0;
        self.slow_coef = (-(sigma0 + sigma_hf * (f0 * 0.001).powf(1.5)) * inv_sr).exp();

        self.dmp = 1.0;
        self.dmp_coef = 1.0;
        self.releasing = false;
        self.pending_release = false;
        self.lp_l.reset();
        self.lp_r.reset();
        self.lp_l.set_cutoff(18_000.0, sr);
        self.lp_r.set_cutoff(18_000.0, sr);

        // Hammer thump.
        self.noise.reseed(seed ^ 0x9E37);
        self.noise_bp
            .set_q((f0 * 4.0).clamp(400.0, 3_500.0), 0.9, sr);
        self.noise_bp.reset();
        self.noise_env = 1.0;
        self.noise_coef = (-6.9 / (0.03 * sr)).exp();
        self.noise_gain = s.hammer * vel.powf(1.5) * 0.22 * level.max(0.3);

        self.age = 0;
        self.flush = 0;
        self.fade.reset();
        self.active = true;
    }

    fn release(&mut self, sr: f32, damper: f32, key: f32) {
        if self.releasing {
            return;
        }
        self.releasing = true;
        self.pending_release = false;
        let t = (damper * 2f32.powf(-(key - 60.0) / 30.0)).clamp(0.03, 4.0);
        self.dmp_coef = (-6.908 / (t * sr)).exp();
    }

    /// Control-rate bookkeeping: drop inaudible partials, close the damper
    /// filter and flush denormals.
    fn control(&mut self, sr: f32) {
        self.age = self.age.saturating_add(CTRL as u32);
        while self.n_live > 1 && self.age > self.end_t[self.n_live - 1] {
            self.n_live -= 1;
        }
        if self.releasing {
            let cutoff = 400.0 + 16_000.0 * self.dmp.powf(2.5);
            self.lp_l.set_cutoff(cutoff, sr);
            self.lp_r.set_cutoff(cutoff, sr);
        }
        self.flush += 1;
        if self.flush >= 8 {
            self.flush = 0;
            for m in self.modes[..self.n_str * self.n_p].iter_mut() {
                if m.s1.abs() < 1.0e-20 && m.s2.abs() < 1.0e-20 {
                    m.s1 = 0.0;
                    m.s2 = 0.0;
                }
            }
        }
    }

    #[inline]
    fn tick(&mut self) -> (f32, f32) {
        let mut l = 0.0f32;
        let mut r = 0.0f32;
        for string in 0..self.n_str {
            let base = string * self.n_p;
            let mut acc = 0.0f32;
            for m in self.modes[base..base + self.n_live].iter_mut() {
                let y = m.a1 * m.s1 + m.a2 * m.s2;
                m.s2 = m.s1;
                m.s1 = y;
                acc += y;
            }
            l += acc * self.gl[string];
            r += acc * self.gr[string];
        }
        if self.noise_env > 1.0e-4 {
            let n = self.noise_bp.tick(self.noise.white()).band_unity
                * self.noise_env
                * self.noise_gain;
            self.noise_env *= self.noise_coef;
            l += n;
            r += n;
        }
        self.pr_env *= self.pr_coef;
        self.slow_env *= self.slow_coef;
        let prompt = self.after + (1.0 - self.after) * self.pr_env;
        let mut g = prompt * self.fade.tick();
        if self.releasing {
            self.dmp *= self.dmp_coef;
            g *= self.dmp;
            l = self.lp_l.low(l);
            r = self.lp_r.low(r);
        }
        let level = prompt * self.slow_env * self.dmp;
        if level < 1.0e-4 || self.fade.is_done() {
            self.active = false;
        }
        (l * g, r * g)
    }
}

impl PoolVoice for GrandVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        (self.after + (1.0 - self.after) * self.pr_env) * self.slow_env * self.dmp
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
    }
}

pub(crate) struct Grand {
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    notes: u32,
    pool: Pool<GrandVoice>,
    set: Settings,
    pedal: bool,
    level: Ramp,
    out_gain: f32,
}

impl Grand {
    pub(crate) fn new() -> Grand {
        Grand {
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            notes: 0,
            pool: Pool::new(28, 24, StealPolicy::Quietest, 48_000.0, GrandVoice::new),
            set: Settings {
                sr: 48_000.0,
                tone: 0.5,
                stiffness: 1.0,
                strike: 0.125,
                hammer: 0.4,
                dynamics: 0.8,
                detune: 1.0,
                decay: 1.0,
                after: 0.5,
                damper: 0.3,
                width: 0.6,
            },
            pedal: false,
            level: Ramp::new(0.0),
            out_gain: 1.0,
        }
    }

    fn derive(&mut self) {
        self.out_gain = db_to_gain(self.level.get());
    }

    fn control(&mut self) {
        self.level.tick(self.ramp_k);
        self.derive();
        let sr = self.sr;
        for v in self.pool.voices.iter_mut().filter(|v| v.active) {
            v.control(sr);
        }
        self.pool.sync();
    }

    fn release_pending(&mut self) {
        let (sr, damper) = (self.sr, self.set.damper);
        for i in 0..self.pool.voices.len() {
            if self.pool.voices[i].pending_release
                && self.pool.alloc.slot(i).state == VoiceState::Held
            {
                let key = self.pool.alloc.slot(i).key as f32;
                self.pool.alloc.release(i);
                self.pool.voices[i].release(sr, damper, key);
            }
            self.pool.voices[i].pending_release = false;
        }
    }
}

impl Synth for Grand {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.set.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(28, 24, StealPolicy::Quietest, sample_rate, GrandVoice::new);
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            TONE => self.set.tone = v,
            STIFFNESS => self.set.stiffness = v,
            STRIKE => self.set.strike = v,
            HAMMER => self.set.hammer = v,
            DYNAMICS => self.set.dynamics = v,
            DETUNE => self.set.detune = v,
            DECAY => self.set.decay = v,
            AFTERSOUND => self.set.after = v,
            PEDAL => {
                let down = v >= 0.5;
                if self.pedal && !down {
                    self.pedal = false;
                    self.release_pending();
                }
                self.pedal = down;
            }
            DAMPER => self.set.damper = v,
            WIDTH => self.set.width = v,
            LEVEL => self.level.set(v, imm),
            _ => {}
        }
        if imm {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.pool.begin(key, id);
        self.notes = self.notes.wrapping_add(1);
        let set = self.set;
        let seed = self.notes;
        self.pool.voices[slot].start(&set, key, velocity, seed);
    }

    fn note_off(&mut self, key: u8, _velocity: f32, id: u32) {
        if self.pedal {
            let mut found = [0usize; 4];
            let n = self.pool.alloc.find_held(id, &mut found);
            for &slot in &found[..n] {
                self.pool.voices[slot].pending_release = true;
            }
        } else {
            let (sr, damper) = (self.sr, self.set.damper);
            self.pool
                .release_id(id, |v| v.release(sr, damper, key as f32));
        }
    }

    fn all_notes_off(&mut self) {
        let (sr, damper) = (self.sr, self.set.damper);
        for i in 0..self.pool.voices.len() {
            if self.pool.alloc.slot(i).state == VoiceState::Held {
                let key = self.pool.alloc.slot(i).key as f32;
                self.pool.alloc.release(i);
                self.pool.voices[i].release(sr, damper, key);
            }
            self.pool.voices[i].pending_release = false;
        }
    }

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
                let (a, b) = v.tick();
                l += a;
                r += b;
            }
            left[n] = l * self.out_gain;
            right[n] = r * self.out_gain;
        }
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }
}
