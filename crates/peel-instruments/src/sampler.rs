//! `peel.sampler`: a pitched sample player.
//!
//! Plays the `SampleBuffer` installed with `set_sample` relative to its root
//! key, with cubic Hermite interpolation, a start offset, reverse playback,
//! forward or ping-pong looping over the buffer's `loop_range`, an ADSR, a
//! low-pass filter, and gate or one-shot behavior. Sixteen voices. With no
//! sample loaded it outputs silence. The previous sample is always handed
//! back from `set_sample`, never dropped on the audio thread.

use std::sync::Arc;

use peel_dsp::envelope::{Adsr, Stage};
use peel_dsp::filter::Svf;
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, pan_gains};
use peel_dsp::voice::StealPolicy;
use peel_dsp::{
    Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext, SampleBuffer,
};

use crate::common::{
    CTRL, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth, ramp_coef, st_ratio,
    vel_gain,
};

const START: ParamId = 1;
const REVERSE: ParamId = 2;
const MODE: ParamId = 3;
const LOOP: ParamId = 4;
const TUNE: ParamId = 5;
const FINE: ParamId = 6;
const KEYTRACK: ParamId = 7;
const ATTACK: ParamId = 10;
const DECAY: ParamId = 11;
const SUSTAIN: ParamId = 12;
const RELEASE: ParamId = 13;
const CUTOFF: ParamId = 20;
const RESONANCE: ParamId = 21;
const GAIN: ParamId = 30;
const PAN: ParamId = 31;
const VELOCITY: ParamId = 32;

static PARAMS: [ParamDescriptor; 16] = [
    ParamDescriptor::linear(
        START,
        "sample.start",
        "Start",
        "Sample",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::toggle(REVERSE, "sample.reverse", "Reverse", "Sample", false),
    ParamDescriptor::choice(
        MODE,
        "sample.mode",
        "Mode",
        "Sample",
        &["Gate", "One-Shot"],
        1,
    ),
    ParamDescriptor::choice(
        LOOP,
        "sample.loop",
        "Loop",
        "Sample",
        &["Off", "Forward", "Ping-Pong"],
        0,
    ),
    ParamDescriptor::linear(
        TUNE,
        "sample.tune",
        "Tune",
        "Sample",
        -24.0,
        24.0,
        0.0,
        Unit::Semitones,
    ),
    ParamDescriptor::linear(
        FINE,
        "sample.fine",
        "Fine",
        "Sample",
        -100.0,
        100.0,
        0.0,
        Unit::Cents,
    ),
    ParamDescriptor::toggle(KEYTRACK, "sample.keytrack", "Key Track", "Sample", true),
    ParamDescriptor::log(
        ATTACK,
        "amp.attack",
        "Attack",
        "Envelope",
        0.000_5,
        5.0,
        0.001,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        DECAY,
        "amp.decay",
        "Decay",
        "Envelope",
        0.005,
        5.0,
        0.3,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        SUSTAIN,
        "amp.sustain",
        "Sustain",
        "Envelope",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        RELEASE,
        "amp.release",
        "Release",
        "Envelope",
        0.005,
        5.0,
        0.08,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        CUTOFF,
        "filter.cutoff",
        "Cutoff",
        "Filter",
        40.0,
        20_000.0,
        20_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        RESONANCE,
        "filter.resonance",
        "Resonance",
        "Filter",
        0.0,
        1.0,
        0.1,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        GAIN,
        "out.gain",
        "Gain",
        "Output",
        -24.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(PAN, "out.pan", "Pan", "Output", -1.0, 1.0, 0.0, Unit::Pan),
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
    type_id: "peel.sampler",
    name: "Sampler",
    description: "Pitched sample player with loop, reverse and envelope",
    category: "Sampler",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "One-Shot",
        params: &[
            ("sample.start", 0.0),
            ("sample.reverse", 0.0),
            ("sample.mode", 1.0),
            ("sample.loop", 0.0),
            ("sample.tune", 0.0),
            ("sample.fine", 0.0),
            ("sample.keytrack", 1.0),
            ("amp.attack", 0.001),
            ("amp.decay", 0.3),
            ("amp.sustain", 1.0),
            ("amp.release", 0.08),
            ("filter.cutoff", 20_000.0),
            ("filter.resonance", 0.1),
            ("out.gain", 0.0),
            ("out.pan", 0.0),
            ("out.velocity", 0.7),
        ],
    },
    Preset {
        name: "Gate",
        params: &[("sample.mode", 0.0), ("amp.release", 0.15)],
    },
    Preset {
        name: "Looped Pad",
        params: &[
            ("sample.mode", 0.0),
            ("sample.loop", 1.0),
            ("amp.attack", 0.2),
            ("amp.release", 0.9),
            ("filter.cutoff", 6_000.0),
        ],
    },
    Preset {
        name: "Reverse",
        params: &[
            ("sample.reverse", 1.0),
            ("sample.mode", 1.0),
            ("amp.attack", 0.002),
        ],
    },
    Preset {
        name: "Ping-Pong",
        params: &[
            ("sample.mode", 0.0),
            ("sample.loop", 2.0),
            ("amp.attack", 0.05),
            ("amp.release", 0.5),
        ],
    },
    Preset {
        name: "Muffled",
        params: &[
            ("sample.mode", 1.0),
            ("filter.cutoff", 1_400.0),
            ("filter.resonance", 0.25),
            ("amp.release", 0.2),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Sampler::new()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoopMode {
    Off,
    Forward,
    PingPong,
}

/// Cubic Hermite (Catmull-Rom) interpolation between `x1` and `x2`.
#[inline]
fn hermite(x0: f32, x1: f32, x2: f32, x3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x2 - x0);
    let c2 = x0 - 2.5 * x1 + 2.0 * x2 - 0.5 * x3;
    let c3 = 0.5 * (x3 - x0) + 1.5 * (x1 - x2);
    ((c3 * t + c2) * t + c1) * t + x1
}

/// Reads a frame, treating everything outside the buffer as silence and
/// wrapping inside the loop region when looping.
#[inline]
fn fetch(data: &[f32], i: isize, looping: Option<(usize, usize)>) -> f32 {
    let mut i = i;
    if let Some((ls, le)) = looping {
        let len = (le - ls) as isize;
        if i >= le as isize {
            i = ls as isize + (i - ls as isize).rem_euclid(len);
        }
    }
    if i < 0 || i as usize >= data.len() {
        0.0
    } else {
        data[i as usize]
    }
}

#[derive(Clone, Copy)]
struct SamplerVoice {
    active: bool,
    pos: f64,
    /// Source frames advanced per output sample, signed for direction.
    step: f64,
    loop_mode: LoopMode,
    loop_start: usize,
    loop_end: usize,
    honors_off: bool,
    env: Adsr,
    filter_l: Svf,
    filter_r: Svf,
    gain: f32,
    tail_fade: f32,
    fade: Fade,
}

impl SamplerVoice {
    fn new(sr: f32) -> SamplerVoice {
        SamplerVoice {
            active: false,
            pos: 0.0,
            step: 1.0,
            loop_mode: LoopMode::Off,
            loop_start: 0,
            loop_end: 0,
            honors_off: true,
            env: Adsr::new(sr),
            filter_l: Svf::new(),
            filter_r: Svf::new(),
            gain: 1.0,
            tail_fade: 1.0,
            fade: Fade::new(),
        }
    }

    #[inline]
    fn tick(&mut self, sample: &SampleBuffer, filter_on: bool) -> (f32, f32) {
        let loop_region = if self.loop_mode == LoopMode::Off {
            None
        } else {
            Some((self.loop_start, self.loop_end))
        };
        let left = sample.left();
        let right = sample.right();
        let i = self.pos.floor();
        let t = (self.pos - i) as f32;
        let i = i as isize;
        let mut l = hermite(
            fetch(left, i - 1, loop_region),
            fetch(left, i, loop_region),
            fetch(left, i + 1, loop_region),
            fetch(left, i + 2, loop_region),
            t,
        );
        let mut r = hermite(
            fetch(right, i - 1, loop_region),
            fetch(right, i, loop_region),
            fetch(right, i + 1, loop_region),
            fetch(right, i + 2, loop_region),
            t,
        );

        // Advance and handle the end of the sample or loop.
        self.pos += self.step;
        let frames = left.len() as f64;
        match self.loop_mode {
            LoopMode::Off => {
                if self.pos >= frames || self.pos < 0.0 {
                    self.active = false;
                }
            }
            LoopMode::Forward => {
                let (ls, le) = (self.loop_start as f64, self.loop_end as f64);
                if (self.step > 0.0 && self.pos >= le) || (self.step < 0.0 && self.pos < ls) {
                    self.pos = ls + (self.pos - ls).rem_euclid(le - ls);
                }
            }
            LoopMode::PingPong => {
                let (ls, le) = (self.loop_start as f64, self.loop_end as f64);
                if self.step > 0.0 && self.pos >= le {
                    self.pos = 2.0 * le - self.pos;
                    self.step = -self.step;
                } else if self.step < 0.0 && self.pos < ls {
                    self.pos = 2.0 * ls - self.pos;
                    self.step = -self.step;
                }
            }
        }

        // Short fade over the last few milliseconds of a non-looping sample so
        // the end never clicks.
        let mut tail = 1.0;
        if self.loop_mode == LoopMode::Off && self.active {
            let remaining = if self.step > 0.0 {
                frames - self.pos
            } else {
                self.pos
            };
            tail = (remaining as f32 / self.tail_fade).clamp(0.0, 1.0);
        }

        if filter_on {
            l = self.filter_l.process(l, peel_dsp::filter::SvfMode::LowPass);
            r = self.filter_r.process(r, peel_dsp::filter::SvfMode::LowPass);
        }
        let env = self.env.tick();
        let g = env * self.gain * tail * self.fade.tick();
        if self.env.is_idle() || self.fade.is_done() {
            self.active = false;
        }
        // Zero sustain with a decayed envelope also ends the voice.
        if self.env.stage() == Stage::Sustain && self.env.level() < 1.0e-4 {
            self.active = false;
        }
        (l * g, r * g)
    }
}

impl PoolVoice for SamplerVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.env.level()
    }
    fn fade_out(&mut self, sample_rate: f32) {
        self.fade.start(sample_rate, STEAL_FADE_MS);
    }
    fn kill(&mut self) {
        self.active = false;
        self.env.reset();
    }
}

pub(crate) struct Sampler {
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    pool: Pool<SamplerVoice>,
    sample: Option<Arc<SampleBuffer>>,
    // Unsmoothed settings.
    start: f32,
    reverse: bool,
    gate_mode: bool,
    loop_mode: LoopMode,
    tune: f32,
    fine: f32,
    keytrack: bool,
    adsr: [f32; 4],
    vel_sens: f32,
    // Smoothed settings.
    cutoff: Ramp,
    res: Ramp,
    gain: Ramp,
    pan: Ramp,
    // Control-rate values.
    filter_on: bool,
    cutoff_hz: f32,
    resonance: f32,
    out_gain: f32,
    pan_l: f32,
    pan_r: f32,
}

impl Sampler {
    pub(crate) fn new() -> Sampler {
        Sampler {
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            pool: Pool::new(20, 16, StealPolicy::Oldest, 48_000.0, || {
                SamplerVoice::new(48_000.0)
            }),
            sample: None,
            start: 0.0,
            reverse: false,
            gate_mode: false,
            loop_mode: LoopMode::Off,
            tune: 0.0,
            fine: 0.0,
            keytrack: true,
            adsr: [0.001, 0.3, 1.0, 0.08],
            vel_sens: 0.7,
            cutoff: Ramp::new(20_000.0f32.ln()),
            res: Ramp::new(0.1),
            gain: Ramp::new(0.0),
            pan: Ramp::new(0.0),
            filter_on: false,
            cutoff_hz: 20_000.0,
            resonance: 0.1,
            out_gain: 1.0,
            pan_l: 1.0,
            pan_r: 1.0,
        }
    }

    fn apply_envelopes(&mut self) {
        let a = self.adsr;
        for v in &mut self.pool.voices {
            v.env.set(a[0], a[1], a[2], a[3], false);
        }
    }

    fn derive(&mut self) {
        self.cutoff_hz = self.cutoff.get().exp();
        self.resonance = self.res.get();
        self.filter_on = self.cutoff_hz < 19_500.0;
        self.out_gain = db_to_gain(self.gain.get());
        (self.pan_l, self.pan_r) = pan_gains(self.pan.get());
        if self.filter_on {
            let (c, r, sr) = (self.cutoff_hz, self.resonance, self.sr);
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                v.filter_l.set(c, r, sr);
                v.filter_r.copy_coeffs(&v.filter_l);
            }
        }
    }

    fn control(&mut self) {
        let k = self.ramp_k;
        self.cutoff.tick(k);
        self.res.tick(k);
        self.gain.tick(k);
        self.pan.tick(k);
        self.derive();
        self.pool.sync();
    }
}

impl Synth for Sampler {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(20, 16, StealPolicy::Oldest, sample_rate, || {
            SamplerVoice::new(sample_rate)
        });
        self.ctrl = 0;
        self.apply_envelopes();
    }

    fn reset(&mut self) {
        self.pool.kill_all();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            START => self.start = v,
            REVERSE => self.reverse = v >= 0.5,
            MODE => self.gate_mode = v < 0.5,
            LOOP => {
                self.loop_mode = match v as usize {
                    0 => LoopMode::Off,
                    1 => LoopMode::Forward,
                    _ => LoopMode::PingPong,
                }
            }
            TUNE => self.tune = v,
            FINE => self.fine = v,
            KEYTRACK => self.keytrack = v >= 0.5,
            ATTACK => {
                self.adsr[0] = v;
                self.apply_envelopes();
            }
            DECAY => {
                self.adsr[1] = v;
                self.apply_envelopes();
            }
            SUSTAIN => {
                self.adsr[2] = v;
                self.apply_envelopes();
            }
            RELEASE => {
                self.adsr[3] = v;
                self.apply_envelopes();
            }
            CUTOFF => self.cutoff.set(v.ln(), imm),
            RESONANCE => self.res.set(v, imm),
            GAIN => self.gain.set(v, imm),
            PAN => self.pan.set(v, imm),
            VELOCITY => self.vel_sens = v,
            _ => {}
        }
        if imm {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let slot = self.pool.begin(key, id);
        let Some(sample) = self.sample.as_deref() else {
            self.pool.voices[slot].active = false;
            return;
        };
        let frames = sample.frames();
        if frames < 2 || sample.sample_rate <= 0.0 {
            self.pool.voices[slot].active = false;
            return;
        }
        let semis = self.tune
            + self.fine * 0.01
            + if self.keytrack {
                key as f32 - sample.root_key as f32
            } else {
                0.0
            };
        let ratio = st_ratio(semis) as f64 * sample.sample_rate as f64 / self.sr as f64;
        if !ratio.is_finite() || ratio <= 0.0 {
            self.pool.voices[slot].active = false;
            return;
        }
        let (ls, le) = match sample.loop_range {
            Some((a, b)) if b > a + 1 && b <= frames => (a, b),
            _ => (0, frames),
        };
        let offset = (self.start as f64 * frames as f64).min(frames as f64 - 2.0);
        let vel = vel_gain(velocity, self.vel_sens) * 0.9;
        let loop_mode = self.loop_mode;
        let reverse = self.reverse;
        let honors_off = self.gate_mode || loop_mode != LoopMode::Off;
        let (c, r, sr, adsr) = (self.cutoff_hz, self.resonance, self.sr, self.adsr);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.step = if reverse { -ratio } else { ratio };
        v.pos = if reverse {
            frames as f64 - 1.0 - offset
        } else {
            offset
        };
        v.loop_mode = loop_mode;
        v.loop_start = ls;
        v.loop_end = le;
        v.honors_off = honors_off;
        v.tail_fade = (0.003 * sample.sample_rate).max(2.0);
        v.gain = vel;
        v.filter_l.set(c, r, sr);
        v.filter_r.copy_coeffs(&v.filter_l);
        v.filter_l.reset();
        v.filter_r.reset();
        v.env.reset();
        v.env.set(adsr[0], adsr[1], adsr[2], adsr[3], false);
        v.env.note_on();
        v.fade.reset();
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        let mut found = [0usize; 8];
        let n = self.pool.alloc.find_held(id, &mut found);
        for &slot in &found[..n] {
            if self.pool.voices[slot].honors_off {
                self.pool.alloc.release(slot);
                self.pool.voices[slot].env.note_off();
            }
        }
    }

    fn all_notes_off(&mut self) {
        self.pool.release_all(|v| v.env.note_off());
    }

    fn all_sound_off(&mut self) {
        self.pool.kill_all();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        // Move the sample out of `self` while rendering so the control code
        // can borrow `self` freely; it is put back below (no refcount change).
        let held = self.sample.take();
        let Some(sample) = held.as_deref() else {
            left[..frames].fill(0.0);
            right[..frames].fill(0.0);
            // Keep the control counter in step so behavior does not depend on
            // when a sample arrives.
            for _ in 0..frames {
                if self.ctrl == 0 {
                    self.ctrl = CTRL;
                }
                self.ctrl -= 1;
            }
            self.sample = held;
            return;
        };
        for n in 0..frames {
            if self.ctrl == 0 {
                self.control();
                self.ctrl = CTRL;
            }
            self.ctrl -= 1;
            let (mut l, mut r) = (0.0, 0.0);
            let filter_on = self.filter_on;
            for v in self.pool.voices.iter_mut().filter(|v| v.active) {
                let (a, b) = v.tick(sample, filter_on);
                l += a;
                r += b;
            }
            left[n] = l * self.out_gain * self.pan_l;
            right[n] = r * self.out_gain * self.pan_r;
        }
        self.sample = held;
    }

    fn is_silent(&self) -> bool {
        self.pool.is_silent()
    }

    fn set_sample(&mut self, sample: Option<Arc<SampleBuffer>>) -> Option<Arc<SampleBuffer>> {
        // Voices read the old buffer, so stop them before swapping.
        self.pool.kill_all();
        std::mem::replace(&mut self.sample, sample)
    }
}
