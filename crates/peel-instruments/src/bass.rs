//! `peel.synth.bass`: a monophonic bass with two oscillators, a sine sub, a
//! saturating 24 dB ladder filter, glide and accent.
//!
//! Plays last-note priority. In `Retrigger` mode every note restarts the
//! envelopes; in `Legato` overlapping notes only change pitch; `Slide` is
//! `Legato` where glide happens only between overlapping notes, the classic
//! acid behavior. Notes with high velocity are accented: a bigger, shorter
//! filter sweep and a louder level.

use peel_dsp::envelope::Adsr;
use peel_dsp::filter::Ladder;
use peel_dsp::osc::{Osc, Waveform};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz};
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::common::{CTRL, Instrument, Ramp, Synth, lerp, ramp_coef, vel_gain};

const WAVE: ParamId = 1;
const PW: ParamId = 2;
const OSC2: ParamId = 3;
const OSC2_DETUNE: ParamId = 4;
const SUB: ParamId = 5;
const GLIDE: ParamId = 6;
const MODE: ParamId = 7;
const CUTOFF: ParamId = 10;
const RESONANCE: ParamId = 11;
const ENV_AMOUNT: ParamId = 12;
const FLT_DECAY: ParamId = 13;
const FLT_SUSTAIN: ParamId = 14;
const KEY_TRACK: ParamId = 15;
const DRIVE: ParamId = 16;
const ACCENT: ParamId = 17;
const AMP_A: ParamId = 20;
const AMP_D: ParamId = 21;
const AMP_S: ParamId = 22;
const AMP_R: ParamId = 23;
const GAIN: ParamId = 24;
const VELOCITY: ParamId = 25;

const WAVES: &[&str] = &["Saw", "Square", "Triangle", "Sine"];
const MODES: &[&str] = &["Retrigger", "Legato", "Slide"];

static PARAMS: [ParamDescriptor; 21] = [
    ParamDescriptor::choice(WAVE, "osc.wave", "Wave", "Oscillator", WAVES, 3),
    ParamDescriptor::linear(
        PW,
        "osc.pw",
        "Pulse Width",
        "Oscillator",
        0.05,
        0.95,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        OSC2,
        "osc2.level",
        "Osc 2 Saw",
        "Oscillator",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        OSC2_DETUNE,
        "osc2.detune",
        "Osc 2 Detune",
        "Oscillator",
        -50.0,
        50.0,
        10.0,
        Unit::Cents,
    ),
    ParamDescriptor::linear(
        SUB,
        "osc.sub",
        "Sub",
        "Oscillator",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::power(
        GLIDE,
        "voice.glide",
        "Glide",
        "Oscillator",
        0.0,
        1.0,
        0.0,
        Unit::Seconds,
        3.0,
    ),
    ParamDescriptor::choice(MODE, "voice.mode", "Mode", "Oscillator", MODES, 1),
    ParamDescriptor::log(
        CUTOFF,
        "filter.cutoff",
        "Cutoff",
        "Filter",
        30.0,
        16_000.0,
        900.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        RESONANCE,
        "filter.resonance",
        "Resonance",
        "Filter",
        0.0,
        1.0,
        0.05,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ENV_AMOUNT,
        "filter.env_amount",
        "Env Amount",
        "Filter",
        -6.0,
        6.0,
        0.8,
        Unit::Octaves,
    ),
    ParamDescriptor::log(
        FLT_DECAY,
        "filter.decay",
        "Env Decay",
        "Filter",
        0.02,
        2.0,
        0.3,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        FLT_SUSTAIN,
        "filter.sustain",
        "Env Sustain",
        "Filter",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        KEY_TRACK,
        "filter.key_track",
        "Key Track",
        "Filter",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        DRIVE,
        "filter.drive",
        "Drive",
        "Filter",
        0.0,
        1.0,
        0.15,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ACCENT,
        "filter.accent",
        "Accent",
        "Filter",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        AMP_A,
        "amp.attack",
        "Attack",
        "Amp",
        0.001,
        1.0,
        0.004,
        Unit::Seconds,
    ),
    ParamDescriptor::log(
        AMP_D,
        "amp.decay",
        "Decay",
        "Amp",
        0.02,
        3.0,
        0.5,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        AMP_S,
        "amp.sustain",
        "Sustain",
        "Amp",
        0.0,
        1.0,
        0.9,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        AMP_R,
        "amp.release",
        "Release",
        "Amp",
        0.01,
        3.0,
        0.15,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        GAIN,
        "out.gain",
        "Gain",
        "Amp",
        -24.0,
        6.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        VELOCITY,
        "out.velocity",
        "Velocity",
        "Amp",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.synth.bass",
    name: "Bass",
    description: "Monophonic bass: sub, acid and reese with glide and drive",
    category: "Synth",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Sub",
        params: &[
            ("osc.wave", 3.0),
            ("osc.pw", 0.5),
            ("osc2.level", 0.0),
            ("osc2.detune", 10.0),
            ("osc.sub", 0.3),
            ("voice.glide", 0.0),
            ("voice.mode", 1.0),
            ("filter.cutoff", 900.0),
            ("filter.resonance", 0.05),
            ("filter.env_amount", 0.8),
            ("filter.decay", 0.3),
            ("filter.sustain", 0.0),
            ("filter.key_track", 0.3),
            ("filter.drive", 0.15),
            ("filter.accent", 0.4),
            ("amp.attack", 0.004),
            ("amp.decay", 0.5),
            ("amp.sustain", 0.9),
            ("amp.release", 0.15),
            ("out.gain", 0.0),
            ("out.velocity", 0.5),
        ],
    },
    Preset {
        name: "Acid",
        params: &[
            ("osc.wave", 0.0),
            ("osc.sub", 0.0),
            ("voice.glide", 0.07),
            ("voice.mode", 2.0),
            ("filter.cutoff", 360.0),
            ("filter.resonance", 0.78),
            ("filter.env_amount", 3.2),
            ("filter.decay", 0.2),
            ("filter.key_track", 0.4),
            ("filter.drive", 0.45),
            ("filter.accent", 0.9),
            ("amp.attack", 0.002),
            ("amp.decay", 0.3),
            ("amp.sustain", 0.8),
            ("amp.release", 0.05),
            ("out.velocity", 0.8),
        ],
    },
    Preset {
        name: "Reese",
        params: &[
            ("osc.wave", 0.0),
            ("osc2.level", 0.9),
            ("osc2.detune", 14.0),
            ("osc.sub", 0.3),
            ("voice.mode", 1.0),
            ("voice.glide", 0.04),
            ("filter.cutoff", 650.0),
            ("filter.resonance", 0.2),
            ("filter.env_amount", 0.5),
            ("filter.decay", 0.5),
            ("filter.sustain", 0.4),
            ("filter.drive", 0.4),
            ("amp.attack", 0.01),
            ("amp.sustain", 1.0),
            ("amp.release", 0.3),
        ],
    },
    Preset {
        name: "Pluck",
        params: &[
            ("osc.wave", 1.0),
            ("osc.pw", 0.35),
            ("osc.sub", 0.2),
            ("voice.mode", 0.0),
            ("filter.cutoff", 240.0),
            ("filter.resonance", 0.35),
            ("filter.env_amount", 3.5),
            ("filter.decay", 0.15),
            ("filter.drive", 0.2),
            ("amp.decay", 0.22),
            ("amp.sustain", 0.0),
            ("amp.release", 0.1),
        ],
    },
    Preset {
        name: "Funk",
        params: &[
            ("osc.wave", 1.0),
            ("osc2.level", 0.4),
            ("osc2.detune", 5.0),
            ("osc.sub", 0.2),
            ("voice.mode", 0.0),
            ("filter.cutoff", 480.0),
            ("filter.resonance", 0.4),
            ("filter.env_amount", 2.6),
            ("filter.decay", 0.12),
            ("filter.key_track", 0.6),
            ("filter.drive", 0.3),
            ("amp.attack", 0.002),
            ("amp.decay", 0.2),
            ("amp.sustain", 0.6),
            ("amp.release", 0.06),
        ],
    },
    Preset {
        name: "Growl",
        params: &[
            ("osc.wave", 0.0),
            ("osc2.level", 0.7),
            ("osc2.detune", 25.0),
            ("osc.sub", 0.25),
            ("voice.glide", 0.03),
            ("voice.mode", 1.0),
            ("filter.cutoff", 560.0),
            ("filter.resonance", 0.55),
            ("filter.env_amount", 2.0),
            ("filter.decay", 0.35),
            ("filter.drive", 0.85),
            ("filter.accent", 0.7),
            ("amp.sustain", 0.9),
            ("amp.release", 0.12),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Bass::new()))
}

const STACK: usize = 16;

#[derive(Clone, Copy)]
struct Held {
    id: u32,
    key: u8,
}

pub(crate) struct Bass {
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    // Voice.
    active: bool,
    gate: bool,
    cur_key: f32,
    target_key: f32,
    glide_on: bool,
    osc1: Osc,
    osc2: Osc,
    sub: Osc,
    dt1: f32,
    dt2: f32,
    dt_sub: f32,
    ladder: Ladder,
    amp_env: Adsr,
    flt_env: Adsr,
    velocity: f32,
    accent_amt: f32,
    vel_level: f32,
    stack: [Held; STACK],
    stack_len: usize,
    last_key: Option<f32>,
    // Unsmoothed settings.
    wave: Waveform,
    mode: usize,
    amp_adsr: [f32; 4],
    flt_decay: f32,
    flt_sustain: f32,
    vel_sens: f32,
    // Smoothed settings.
    pw: Ramp,
    osc2_level: Ramp,
    detune2: Ramp,
    sub_level: Ramp,
    glide: Ramp,
    cutoff: Ramp,
    res: Ramp,
    env_amt: Ramp,
    key_track: Ramp,
    drive: Ramp,
    accent: Ramp,
    gain: Ramp,
    // Derived.
    glide_k: f32,
    out_gain: f32,
}

impl Bass {
    pub(crate) fn new() -> Bass {
        Bass {
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            active: false,
            gate: false,
            cur_key: 36.0,
            target_key: 36.0,
            glide_on: false,
            osc1: Osc::new(),
            osc2: Osc::new(),
            sub: Osc::new(),
            dt1: 0.0,
            dt2: 0.0,
            dt_sub: 0.0,
            ladder: Ladder::new(),
            amp_env: Adsr::new(48_000.0),
            flt_env: Adsr::new(48_000.0),
            velocity: 0.0,
            accent_amt: 0.0,
            vel_level: 1.0,
            stack: [Held { id: 0, key: 0 }; STACK],
            stack_len: 0,
            last_key: None,
            wave: Waveform::Sine,
            mode: 1,
            amp_adsr: [0.004, 0.5, 0.9, 0.15],
            flt_decay: 0.3,
            flt_sustain: 0.0,
            vel_sens: 0.5,
            pw: Ramp::new(0.5),
            osc2_level: Ramp::new(0.0),
            detune2: Ramp::new(10.0),
            sub_level: Ramp::new(0.3),
            glide: Ramp::new(0.0),
            cutoff: Ramp::new(900.0f32.ln()),
            res: Ramp::new(0.05),
            env_amt: Ramp::new(0.8),
            key_track: Ramp::new(0.3),
            drive: Ramp::new(0.15),
            accent: Ramp::new(0.4),
            gain: Ramp::new(0.0),
            glide_k: 1.0,
            out_gain: 1.0,
        }
    }

    fn apply_envelopes(&mut self) {
        let a = self.amp_adsr;
        self.amp_env.set(a[0], a[1], a[2], a[3], false);
        // The filter envelope shortens with accent; the decay used here is
        // refreshed per note, but keep the idle value sensible too.
        self.flt_env
            .set(0.001, self.flt_decay, self.flt_sustain, a[3], false);
    }

    fn derive(&mut self) {
        let glide = self.glide.get();
        self.glide_k = if glide <= 0.001 {
            1.0
        } else {
            1.0 - (-3.0 * CTRL as f32 / (glide * self.sr)).exp()
        };
        self.out_gain = db_to_gain(self.gain.get());
    }

    /// Pitch, filter and glide update, once per control interval.
    fn control(&mut self) {
        let k = self.ramp_k;
        self.pw.tick(k);
        self.osc2_level.tick(k);
        self.detune2.tick(k);
        self.sub_level.tick(k);
        self.glide.tick(k);
        self.cutoff.tick(k);
        self.res.tick(k);
        self.env_amt.tick(k);
        self.key_track.tick(k);
        self.drive.tick(k);
        self.accent.tick(k);
        self.gain.tick(k);
        self.derive();
        if self.active {
            self.update_voice();
        }
    }

    fn update_voice(&mut self) {
        if self.glide_on {
            self.cur_key += (self.target_key - self.cur_key) * self.glide_k;
        } else {
            self.cur_key = self.target_key;
        }
        let inv_sr = 1.0 / self.sr;
        self.dt1 = key_to_hz(self.cur_key) * inv_sr;
        self.dt2 = key_to_hz(self.cur_key + self.detune2.get() * 0.01) * inv_sr;
        self.dt_sub = key_to_hz(self.cur_key - 12.0) * inv_sr;
        let acc = self.accent_amt;
        let env_amt = self.env_amt.get() * (1.0 + 0.6 * acc) + acc * 0.8;
        let oct =
            env_amt * self.flt_env.level() + self.key_track.get() * (self.cur_key - 48.0) / 12.0;
        let cutoff =
            (self.cutoff.get().exp() * oct.clamp(-12.0, 12.0).exp2()).clamp(20.0, 0.45 * self.sr);
        let drive = self.drive.get();
        let drive_g = 1.0 + 7.0 * drive * drive + 0.8 * acc;
        let res = (self.res.get() + 0.12 * acc).min(1.0);
        self.ladder.set(cutoff, res, drive_g, self.sr);
    }

    fn start_note(&mut self, key: u8, velocity: f32, retrigger: bool) {
        self.velocity = velocity;
        self.accent_amt = self.accent.get() * ((velocity - 0.7) / 0.25).clamp(0.0, 1.0);
        self.vel_level = vel_gain(velocity, self.vel_sens) * (1.0 + 0.5 * self.accent_amt);
        self.target_key = key as f32;
        if retrigger {
            let decay = self.flt_decay * (1.0 - 0.5 * self.accent_amt);
            let a = self.amp_adsr;
            self.amp_env.set(a[0], a[1], a[2], a[3], false);
            self.flt_env
                .set(0.001, decay, self.flt_sustain, a[3], false);
            self.amp_env.note_on();
            self.flt_env.note_on();
        }
        self.gate = true;
        if !self.active {
            self.active = true;
        }
        self.update_voice();
    }
}

impl Synth for Bass {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.amp_env = Adsr::new(sample_rate);
        self.flt_env = Adsr::new(sample_rate);
        self.ctrl = 0;
        self.apply_envelopes();
    }

    fn reset(&mut self) {
        self.active = false;
        self.gate = false;
        self.stack_len = 0;
        self.last_key = None;
        self.amp_env.reset();
        self.flt_env.reset();
        self.ladder.reset();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            WAVE => {
                self.wave = match v as usize {
                    0 => Waveform::Saw,
                    1 => Waveform::Square,
                    2 => Waveform::Triangle,
                    _ => Waveform::Sine,
                }
            }
            PW => self.pw.set(v, imm),
            OSC2 => self.osc2_level.set(v, imm),
            OSC2_DETUNE => self.detune2.set(v, imm),
            SUB => self.sub_level.set(v, imm),
            GLIDE => self.glide.set(v, imm),
            MODE => self.mode = v as usize,
            CUTOFF => self.cutoff.set(v.ln(), imm),
            RESONANCE => self.res.set(v, imm),
            ENV_AMOUNT => self.env_amt.set(v, imm),
            FLT_DECAY => {
                self.flt_decay = v;
                self.apply_envelopes();
            }
            FLT_SUSTAIN => {
                self.flt_sustain = v;
                self.apply_envelopes();
            }
            KEY_TRACK => self.key_track.set(v, imm),
            DRIVE => self.drive.set(v, imm),
            ACCENT => self.accent.set(v, imm),
            AMP_A => {
                self.amp_adsr[0] = v;
                self.apply_envelopes();
            }
            AMP_D => {
                self.amp_adsr[1] = v;
                self.apply_envelopes();
            }
            AMP_S => {
                self.amp_adsr[2] = v;
                self.apply_envelopes();
            }
            AMP_R => {
                self.amp_adsr[3] = v;
                self.apply_envelopes();
            }
            GAIN => self.gain.set(v, imm),
            VELOCITY => self.vel_sens = v,
            _ => {}
        }
        if imm {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let overlapping = self.stack_len > 0;
        if self.stack_len == STACK {
            self.stack.copy_within(1.., 0);
            self.stack_len -= 1;
        }
        self.stack[self.stack_len] = Held { id, key };
        self.stack_len += 1;

        let legato = self.mode != 0 && overlapping;
        // Glide: always in Retrigger/Legato; only between overlapping notes
        // in Slide.
        self.glide_on = match self.mode {
            2 => overlapping && self.glide_k < 1.0,
            _ => self.glide_k < 1.0 && self.last_key.is_some(),
        };
        if !self.active && self.glide_on {
            self.cur_key = self.last_key.unwrap_or(key as f32);
        }
        self.last_key = Some(key as f32);
        self.start_note(key, velocity, !legato);
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        let Some(pos) = self.stack[..self.stack_len].iter().position(|h| h.id == id) else {
            return;
        };
        let was_top = pos + 1 == self.stack_len;
        self.stack.copy_within(pos + 1..self.stack_len, pos);
        self.stack_len -= 1;
        if self.stack_len == 0 {
            self.gate = false;
            self.amp_env.note_off();
            self.flt_env.note_off();
        } else if was_top {
            // Fall back to the previous held note without retriggering.
            let back = self.stack[self.stack_len - 1];
            self.glide_on = self.glide_k < 1.0;
            self.target_key = back.key as f32;
            self.last_key = Some(back.key as f32);
        }
    }

    fn all_notes_off(&mut self) {
        self.stack_len = 0;
        self.gate = false;
        self.amp_env.note_off();
        self.flt_env.note_off();
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
            if !self.active {
                left[n] = 0.0;
                right[n] = 0.0;
                continue;
            }
            let amp = self.amp_env.tick();
            self.flt_env.tick();
            let pw = self.pw.get();
            let mut s = self.osc1.tick(self.wave, self.dt1, pw);
            let o2 = self.osc2_level.get();
            let osc2 = self.osc2.tick(Waveform::Saw, self.dt2, 0.5);
            s = s * (1.0 - 0.4 * o2) + osc2 * o2 * 0.6;
            s += self.sub.tick(Waveform::Sine, self.dt_sub, 0.5) * self.sub_level.get() * 0.8;
            let y = self.ladder.tick(s * 0.7);
            let out = y * amp * self.vel_level * self.out_gain;
            left[n] = out;
            right[n] = out;
            if self.amp_env.is_idle() {
                self.active = false;
            }
        }
    }

    fn is_silent(&self) -> bool {
        !self.active
    }
}
