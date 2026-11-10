//! `peel.bass.808`: a tuned sub-bass drum in the lineage of the TR-808 kick
//! played as a bass line.
//!
//! A sine that can lean towards a triangle, with an optional phase-locked
//! octave partial, starts at its zero crossing and drops onto its pitch from
//! a few semitones above for the punch. It then holds, decays and (if the
//! key is still down) sustains. That signal runs through a waveshaper at 4x
//! the sample rate, a DC blocker and a 12 dB tone filter; a short band-passed
//! click is added after the filter so it stays crisp however dark the body
//! is. Output is mono.
//!
//! The instrument is monophonic with last-note priority. `Glide` decides
//! what a new note does:
//!
//! - `Off`: every note restarts the drum on its own pitch.
//! - `Legato`: a note played while another is held slides to the new pitch
//!   without restarting (the classic 808 slide); separate notes restart.
//! - `Always`: as `Legato`, and separate notes also slide in from the
//!   previous pitch.
//!
//! Slides take `Glide Time` whatever the interval and move in a straight
//! line in pitch. A restart over a note that is still sounding crossfades
//! the old one out over a few milliseconds, so retriggers never click.
//!
//! `Note Off` is `Release` (the key gates the drum, with its own release
//! time, down to a hard choke) or `Ring` (the decay always runs out, like a
//! one-shot sample). `Decay Track` shortens the decay as notes go up, the
//! way a sample played faster would.

use peel_dsp::filter::{Svf, tanh_approx};
use peel_dsp::noise::Noise;
use peel_dsp::osc::{self, Waveform, fast_sin};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{db_to_gain, key_to_hz};
use peel_dsp::{Device, DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};

use crate::analog::{DcBlock, Down2, PitchEnv, Up2, advance, shape_by, soft_ceiling};
use crate::common::{CTRL, Expression, Instrument, Ramp, Synth, ramp_coef, safe_bend, vel_gain};

const SHAPE: ParamId = 1;
const OCTAVE: ParamId = 2;
const PUNCH: ParamId = 3;
const PUNCH_TIME: ParamId = 4;
const FINE: ParamId = 5;
const GLIDE_MODE: ParamId = 6;
const GLIDE_TIME: ParamId = 7;
const ATTACK: ParamId = 8;
const HOLD: ParamId = 9;
const DECAY: ParamId = 10;
const SUSTAIN: ParamId = 11;
const RELEASE: ParamId = 12;
const NOTE_OFF: ParamId = 13;
const DECAY_TRACK: ParamId = 14;
const DRIVE_TYPE: ParamId = 15;
const DRIVE: ParamId = 16;
const TONE: ParamId = 17;
const CLICK: ParamId = 18;
const CLICK_TONE: ParamId = 19;
const GAIN: ParamId = 20;
const VELOCITY: ParamId = 21;

const GLIDES: &[&str] = &["Off", "Legato", "Always"];
const NOTE_OFFS: &[&str] = &["Release", "Ring"];
const DRIVES: &[&str] = &["Warm", "Tube", "Clip", "Fold"];

static PARAMS: [ParamDescriptor; 21] = [
    ParamDescriptor::linear(
        SHAPE,
        "osc.shape",
        "Shape",
        "Oscillator",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        OCTAVE,
        "osc.octave",
        "Octave",
        "Oscillator",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        PUNCH,
        "pitch.punch",
        "Punch",
        "Oscillator",
        0.0,
        48.0,
        12.0,
        Unit::Semitones,
    ),
    ParamDescriptor::log(
        PUNCH_TIME,
        "pitch.punch_time",
        "Punch Time",
        "Oscillator",
        0.005,
        0.5,
        0.045,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        FINE,
        "pitch.fine",
        "Fine",
        "Oscillator",
        -100.0,
        100.0,
        0.0,
        Unit::Cents,
    ),
    ParamDescriptor::choice(GLIDE_MODE, "glide.mode", "Glide", "Glide", GLIDES, 1),
    ParamDescriptor::power(
        GLIDE_TIME,
        "glide.time",
        "Glide Time",
        "Glide",
        0.0,
        2.0,
        0.08,
        Unit::Seconds,
        3.0,
    ),
    ParamDescriptor::log(
        ATTACK,
        "amp.attack",
        "Attack",
        "Amp",
        0.0005,
        0.05,
        0.001,
        Unit::Seconds,
    ),
    ParamDescriptor::power(
        HOLD,
        "amp.hold",
        "Hold",
        "Amp",
        0.0,
        2.0,
        0.06,
        Unit::Seconds,
        3.0,
    ),
    ParamDescriptor::log(
        DECAY,
        "amp.decay",
        "Decay",
        "Amp",
        0.05,
        12.0,
        1.8,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        SUSTAIN,
        "amp.sustain",
        "Sustain",
        "Amp",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        RELEASE,
        "amp.release",
        "Release",
        "Amp",
        0.005,
        4.0,
        0.08,
        Unit::Seconds,
    ),
    ParamDescriptor::choice(NOTE_OFF, "amp.note_off", "Note Off", "Amp", NOTE_OFFS, 0),
    ParamDescriptor::linear(
        DECAY_TRACK,
        "amp.decay_track",
        "Decay Track",
        "Amp",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::choice(DRIVE_TYPE, "drive.type", "Type", "Tone", DRIVES, 0),
    ParamDescriptor::linear(
        DRIVE,
        "drive.amount",
        "Drive",
        "Tone",
        0.0,
        1.0,
        0.2,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        TONE,
        "tone.cutoff",
        "Tone",
        "Tone",
        60.0,
        18_000.0,
        5_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        CLICK,
        "click.level",
        "Click",
        "Tone",
        0.0,
        1.0,
        0.15,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        CLICK_TONE,
        "click.tone",
        "Click Tone",
        "Tone",
        400.0,
        8_000.0,
        2_500.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        GAIN,
        "out.gain",
        "Gain",
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
        0.4,
        Unit::Percent,
    ),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.bass.808",
    name: "808",
    description: "Tuned 808 sub bass: punch, long decay, drive and slides between notes",
    category: "Bass",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 14] = [
    Preset {
        name: "Classic",
        params: &[
            ("osc.shape", 0.0),
            ("pitch.punch", 12.0),
            ("pitch.punch_time", 0.045),
            ("glide.mode", 1.0),
            ("glide.time", 0.08),
            ("amp.hold", 0.06),
            ("amp.decay", 1.8),
            ("amp.release", 0.08),
            ("drive.type", 0.0),
            ("drive.amount", 0.2),
            ("tone.cutoff", 5_000.0),
            ("click.level", 0.15),
        ],
    },
    Preset {
        name: "Clean Sub",
        params: &[
            ("pitch.punch", 5.0),
            ("pitch.punch_time", 0.03),
            ("amp.hold", 0.1),
            ("amp.decay", 2.4),
            ("drive.amount", 0.0),
            ("tone.cutoff", 1_200.0),
            ("click.level", 0.0),
            ("out.gain", 1.0),
        ],
    },
    Preset {
        name: "Long",
        params: &[
            ("pitch.punch", 10.0),
            ("amp.hold", 0.25),
            ("amp.decay", 5.5),
            ("amp.note_off", 1.0),
            ("amp.decay_track", 0.15),
            ("drive.amount", 0.28),
            ("tone.cutoff", 3_200.0),
            ("click.level", 0.1),
        ],
    },
    Preset {
        name: "Punchy",
        params: &[
            ("pitch.punch", 22.0),
            ("pitch.punch_time", 0.03),
            ("amp.hold", 0.03),
            ("amp.decay", 0.7),
            ("drive.type", 2.0),
            ("drive.amount", 0.3),
            ("tone.cutoff", 7_000.0),
            ("click.level", 0.4),
            ("click.tone", 3_200.0),
            ("out.gain", -1.0),
        ],
    },
    Preset {
        name: "Distorted",
        params: &[
            ("osc.shape", 0.2),
            ("pitch.punch", 14.0),
            ("amp.hold", 0.12),
            ("amp.decay", 2.2),
            ("drive.type", 2.0),
            ("drive.amount", 0.7),
            ("tone.cutoff", 4_200.0),
            ("click.level", 0.2),
            ("out.gain", -4.5),
        ],
    },
    Preset {
        name: "Fuzz",
        params: &[
            ("osc.shape", 0.35),
            ("pitch.punch", 9.0),
            ("amp.hold", 0.15),
            ("amp.decay", 2.0),
            ("drive.type", 3.0),
            ("drive.amount", 0.75),
            ("tone.cutoff", 2_600.0),
            ("click.level", 0.05),
            ("out.gain", -5.0),
        ],
    },
    Preset {
        name: "Slide",
        params: &[
            ("pitch.punch", 10.0),
            ("glide.mode", 1.0),
            ("glide.time", 0.16),
            ("amp.hold", 0.3),
            ("amp.decay", 4.0),
            ("amp.sustain", 0.35),
            ("amp.release", 0.25),
            ("amp.decay_track", 0.1),
            ("drive.type", 1.0),
            ("drive.amount", 0.35),
            ("tone.cutoff", 3_600.0),
            ("click.level", 0.12),
            ("out.gain", -1.5),
        ],
    },
    Preset {
        name: "Portamento",
        params: &[
            ("pitch.punch", 6.0),
            ("glide.mode", 2.0),
            ("glide.time", 0.11),
            ("amp.hold", 0.2),
            ("amp.decay", 3.0),
            ("amp.release", 0.15),
            ("drive.type", 1.0),
            ("drive.amount", 0.3),
            ("tone.cutoff", 3_000.0),
            ("click.level", 0.08),
            ("out.gain", -1.0),
        ],
    },
    Preset {
        name: "Deep",
        params: &[
            ("pitch.punch", 7.0),
            ("pitch.punch_time", 0.07),
            ("amp.hold", 0.15),
            ("amp.decay", 3.2),
            ("drive.amount", 0.12),
            ("tone.cutoff", 420.0),
            ("click.level", 0.0),
            ("out.gain", 1.5),
        ],
    },
    Preset {
        name: "Bright",
        params: &[
            ("osc.shape", 0.3),
            ("osc.octave", 0.35),
            ("pitch.punch", 16.0),
            ("amp.hold", 0.08),
            ("amp.decay", 1.5),
            ("drive.type", 1.0),
            ("drive.amount", 0.55),
            ("tone.cutoff", 9_000.0),
            ("click.level", 0.35),
            ("click.tone", 4_200.0),
            ("out.gain", -4.0),
        ],
    },
    Preset {
        name: "Tape",
        params: &[
            ("osc.shape", 0.12),
            ("pitch.punch", 9.0),
            ("pitch.punch_time", 0.06),
            ("amp.hold", 0.1),
            ("amp.decay", 2.6),
            ("drive.type", 1.0),
            ("drive.amount", 0.4),
            ("tone.cutoff", 1_700.0),
            ("click.level", 0.06),
            ("click.tone", 1_600.0),
            ("out.gain", -2.0),
        ],
    },
    Preset {
        name: "Thump",
        params: &[
            ("pitch.punch", 30.0),
            ("pitch.punch_time", 0.035),
            ("glide.mode", 0.0),
            ("amp.hold", 0.02),
            ("amp.decay", 0.38),
            ("amp.note_off", 1.0),
            ("drive.type", 2.0),
            ("drive.amount", 0.35),
            ("tone.cutoff", 6_000.0),
            ("click.level", 0.5),
            ("click.tone", 2_800.0),
            ("out.gain", -1.5),
        ],
    },
    Preset {
        name: "Small Speaker",
        params: &[
            ("osc.shape", 0.55),
            ("osc.octave", 0.6),
            ("pitch.punch", 12.0),
            ("amp.hold", 0.1),
            ("amp.decay", 1.9),
            ("drive.type", 1.0),
            ("drive.amount", 0.6),
            ("tone.cutoff", 6_500.0),
            ("click.level", 0.25),
            ("out.gain", -5.0),
        ],
    },
    Preset {
        name: "Held Sub",
        params: &[
            ("pitch.punch", 4.0),
            ("pitch.punch_time", 0.02),
            ("glide.mode", 1.0),
            ("glide.time", 0.06),
            ("amp.attack", 0.004),
            ("amp.hold", 0.0),
            ("amp.decay", 1.2),
            ("amp.sustain", 0.85),
            ("amp.release", 0.12),
            ("amp.decay_track", 0.0),
            ("drive.amount", 0.15),
            ("tone.cutoff", 2_000.0),
            ("click.level", 0.0),
        ],
    },
];

pub(crate) fn create() -> Box<dyn Device> {
    Box::new(Instrument::new(&DESC, Sub808::new()))
}

const STACK: usize = 16;
/// Crossfade applied to the outgoing strike when a note retriggers.
const XFADE_MS: f32 = 4.0;
/// Level below which a decaying strike is finished.
const IDLE: f32 = 1.0e-5;
/// Key the decay time is quoted at; `Decay Track` scales around it.
const TRACK_KEY: f32 = 36.0;
/// Highest note frequency that gets the full `Fold` gain.
const FOLD_FULL_HZ: f32 = 260.0;
/// Bias of the `Tube` shaper: the asymmetry that makes even harmonics.
const TUBE_BIAS: f32 = 0.3;

#[derive(Clone, Copy)]
struct Held {
    id: u32,
    key: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Hold,
    Decay,
    Release,
}

/// Attack, hold, exponential decay to a sustain level, release.
#[derive(Clone, Copy)]
struct AmpEnv {
    stage: Stage,
    level: f32,
    attack_step: f32,
    hold_left: u32,
    decay_coef: f32,
    sustain: f32,
    release_coef: f32,
}

impl AmpEnv {
    const fn new() -> AmpEnv {
        AmpEnv {
            stage: Stage::Idle,
            level: 0.0,
            attack_step: 1.0,
            hold_left: 0,
            decay_coef: 0.999,
            sustain: 0.0,
            release_coef: 0.99,
        }
    }

    fn start(&mut self, attack: f32, hold: f32, sr: f32) {
        self.attack_step = 1.0 / (attack.max(0.0002) * sr).max(1.0);
        self.hold_left = (hold.max(0.0) * sr) as u32;
        self.stage = Stage::Attack;
    }

    /// The key came up. `ring` lets the decay run out instead of releasing.
    fn note_off(&mut self, ring: bool) {
        if self.stage == Stage::Idle {
            return;
        }
        if ring {
            // A sustained note has nothing left to hold it up.
            self.sustain = 0.0;
        } else {
            self.stage = Stage::Release;
        }
    }

    fn release(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.level += self.attack_step;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = if self.hold_left > 0 {
                        Stage::Hold
                    } else {
                        Stage::Decay
                    };
                }
            }
            Stage::Hold => {
                self.hold_left -= 1;
                if self.hold_left == 0 {
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = self.sustain + (self.level - self.sustain) * self.decay_coef;
                if self.level < IDLE {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Release => {
                self.level *= self.release_coef;
                if self.level < IDLE {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

/// Per-sample multiplier that falls 60 dB in `seconds`.
fn t60(seconds: f32, sr: f32) -> f32 {
    (-6.907_755 / (seconds.max(0.001) * sr)).exp()
}

pub(crate) struct Sub808 {
    expr: Expression,
    sr: f32,
    ramp_k: f32,
    ctrl: usize,
    notes: u32,
    // The sounding strike.
    active: bool,
    phase: f32,
    dt: f32,
    dt_step: f32,
    env: AmpEnv,
    pitch_env: PitchEnv,
    vel_level: f32,
    /// The velocity gain in use, slewed so a retrigger at another velocity
    /// crosses over with the outgoing strike.
    vel_cur: f32,
    vel_k: f32,
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
    // The strike a retrigger replaced, fading out.
    ghost_phase: f32,
    ghost_dt: f32,
    ghost_amp: f32,
    ghost_step: f32,
    // Click layer.
    click_env: f32,
    click_coef: f32,
    click_phase: f32,
    click_dt: f32,
    click_bp: Svf,
    noise: Noise,
    // Pitch.
    cur_key: f32,
    target_key: f32,
    glide_step: f32,
    glide_left: u32,
    last_key: Option<f32>,
    stack: [Held; STACK],
    stack_len: usize,
    /// Runtime voice ID of the note on top of the stack; its per-note
    /// expression drives the single voice.
    voice_id: u32,
    // Tone path.
    up_a: Up2,
    up_b: Up2,
    down_a: Down2,
    down_b: Down2,
    dc: DcBlock,
    tone_lp: Svf,
    drive_gain: f32,
    drive_mix: f32,
    // Unsmoothed settings.
    glide_mode: usize,
    glide_time: f32,
    attack: f32,
    hold: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    ring: bool,
    decay_track: f32,
    drive_kind: usize,
    punch_time: f32,
    vel_sens: f32,
    // Smoothed settings.
    shape: Ramp,
    octave: Ramp,
    punch: Ramp,
    fine: Ramp,
    drive: Ramp,
    tone: Ramp,
    click: Ramp,
    click_tone: Ramp,
    gain: Ramp,
    out_gain: f32,
}

impl Sub808 {
    pub(crate) fn new() -> Sub808 {
        Sub808 {
            expr: Expression::new(),
            sr: 48_000.0,
            ramp_k: 0.1,
            ctrl: 0,
            notes: 0,
            active: false,
            phase: 0.0,
            dt: 0.0,
            dt_step: 0.0,
            env: AmpEnv::new(),
            pitch_env: PitchEnv::new(),
            vel_level: 1.0,
            vel_cur: 1.0,
            vel_k: 0.01,
            xgain: 1.0,
            ghost_phase: 0.0,
            ghost_dt: 0.0,
            ghost_amp: 0.0,
            ghost_step: 0.0,
            click_env: 0.0,
            click_coef: 0.99,
            click_phase: 0.0,
            click_dt: 0.0,
            click_bp: Svf::new(),
            noise: Noise::new(0x808),
            cur_key: 36.0,
            target_key: 36.0,
            glide_step: 0.0,
            glide_left: 0,
            last_key: None,
            stack: [Held { id: 0, key: 0 }; STACK],
            stack_len: 0,
            voice_id: 0,
            up_a: Up2::new(),
            up_b: Up2::new(),
            down_a: Down2::new(),
            down_b: Down2::new(),
            dc: DcBlock::new(48_000.0),
            tone_lp: Svf::new(),
            drive_gain: 1.0,
            drive_mix: 0.0,
            glide_mode: 1,
            glide_time: 0.08,
            attack: 0.001,
            hold: 0.06,
            decay: 1.8,
            sustain: 0.0,
            release: 0.08,
            ring: false,
            decay_track: 0.3,
            drive_kind: 0,
            punch_time: 0.045,
            vel_sens: 0.4,
            shape: Ramp::new(0.0),
            octave: Ramp::new(0.0),
            punch: Ramp::new(12.0),
            fine: Ramp::new(0.0),
            drive: Ramp::new(0.2),
            tone: Ramp::new(5_000.0f32.ln()),
            click: Ramp::new(0.15),
            click_tone: Ramp::new(2_500.0f32.ln()),
            gain: Ramp::new(0.0),
            out_gain: 1.0,
        }
    }

    fn sounding(&self) -> bool {
        self.active || self.ghost_amp > 0.0 || self.click_env > 0.0
    }

    /// Decay and release rates for the note on `key`.
    fn apply_times(&mut self, key: f32) {
        let scale = (-(key - TRACK_KEY) / 12.0 * self.decay_track).exp2();
        let decay = (self.decay * scale).clamp(0.03, 24.0);
        self.env.decay_coef = t60(decay, self.sr);
        self.env.release_coef = t60(self.release, self.sr);
        // A note already let go in `Ring` keeps decaying to silence.
        if self.stack_len > 0 || !self.ring {
            self.env.sustain = self.sustain;
        }
    }

    /// Phase increment for the current key, drop, fine tune and bend.
    fn pitch_dt(&mut self) -> f32 {
        let key = self.target_key.round().clamp(0.0, 127.0) as u8;
        let x = self.expr.voice(self.voice_id, key);
        self.xgain = x.gain();
        let base =
            self.cur_key + self.fine.get() * 0.01 + self.pitch_env.level() * self.punch.get();
        let base = base + safe_bend(base + 1.0, x.bend, 0.45 * self.sr);
        (key_to_hz(base) / self.sr).clamp(0.0, 0.45)
    }

    fn derive(&mut self) {
        let key = self.target_key.round().clamp(0.0, 127.0) as u8;
        let x = self.expr.voice(self.voice_id, key);
        // Timbre leans on the drive as well as the tone filter; at neutral
        // it is exactly the knob.
        let amount = (self.drive.get() + 0.35 * x.tone).clamp(0.0, 1.0);
        let a2 = amount * amount;
        self.drive_gain = match self.drive_kind {
            // Folding multiplies the harmonic count by its gain, so it eases
            // off above the bass register to stay inside the oversampler.
            3 => 1.0 + 9.0 * a2 * (FOLD_FULL_HZ / (self.dt * self.sr).max(1.0)).min(1.0),
            2 => 1.0 + 19.0 * a2,
            _ => 1.0 + 15.0 * a2,
        };
        // The shaper fades in over the first part of the knob, so a clean
        // setting is exactly clean and automation never steps.
        self.drive_mix = (amount * 6.0).min(1.0);
        self.out_gain = db_to_gain(self.gain.get());
        let cutoff = (self.tone.get().exp() * x.cutoff_oct().exp2()).clamp(40.0, 0.45 * self.sr);
        self.tone_lp
            .set_q(cutoff, std::f32::consts::FRAC_1_SQRT_2, self.sr);
        let click = self.click_tone.get().exp().min(0.4 * self.sr);
        self.click_bp.set_q(click, 1.2, self.sr);
        self.click_dt = click * 0.5 / self.sr;
    }

    /// Smoothing, glide and pitch, once per control interval.
    fn control(&mut self) {
        let k = self.ramp_k;
        self.shape.tick(k);
        self.octave.tick(k);
        self.punch.tick(k);
        self.fine.tick(k);
        self.drive.tick(k);
        self.tone.tick(k);
        self.click.tick(k);
        self.click_tone.tick(k);
        self.gain.tick(k);
        self.expr.tick();
        self.derive();
        if self.glide_left > 0 {
            self.glide_left -= 1;
            self.cur_key = if self.glide_left == 0 {
                self.target_key
            } else {
                self.cur_key + self.glide_step
            };
        }
        if self.active {
            let target = self.pitch_dt();
            self.dt_step = (target - self.dt) / CTRL as f32;
        }
    }

    /// Points the voice at `key`, sliding there when `glide` is set.
    fn aim(&mut self, key: f32, glide: bool) {
        self.target_key = key;
        let ticks = (self.glide_time * self.sr / CTRL as f32).round() as u32;
        if glide && ticks > 0 && (key - self.cur_key).abs() > 1.0e-4 {
            self.glide_left = ticks;
            self.glide_step = (key - self.cur_key) / ticks as f32;
        } else {
            self.glide_left = 0;
            self.cur_key = key;
        }
    }

    /// Restarts the drum: the outgoing strike fades, the new one begins at
    /// its zero crossing.
    fn strike(&mut self, velocity: f32) {
        let level = self.env.level;
        let over = self.active && level > 1.0e-4;
        if over {
            let n = (XFADE_MS * 0.001 * self.sr).max(1.0);
            self.ghost_phase = self.phase;
            self.ghost_dt = self.dt;
            self.ghost_amp = level;
            self.ghost_step = level / n;
        }
        self.notes = self.notes.wrapping_add(1);
        self.noise.reseed(self.notes ^ 0x0808_5EED);
        self.phase = 0.0;
        self.vel_level = vel_gain(velocity, self.vel_sens);
        if !over {
            self.vel_cur = self.vel_level;
        }
        self.env.level = 0.0;
        self.env.start(self.attack, self.hold, self.sr);
        self.pitch_env.set_decay(self.punch_time, self.sr);
        self.pitch_env.trigger();
        self.click_env = 1.0;
        self.click_coef = t60(0.018, self.sr);
        self.click_phase = 0.0;
        self.active = true;
    }

    /// One sample of the oscillator at `phase`.
    #[inline]
    fn osc(&self, phase: f32, dt: f32) -> f32 {
        let sine = fast_sin(phase);
        let shape = self.shape.get();
        let mut s = sine;
        if shape > 1.0e-4 {
            // The triangle a quarter turn on, so it rises through zero with
            // the sine.
            let mut q = phase + 0.25;
            if q >= 1.0 {
                q -= 1.0;
            }
            let tri = osc::eval(Waveform::Triangle, q, dt, 0.5);
            s += shape * (tri - sine);
        }
        let oct = self.octave.get();
        if oct > 1.0e-4 {
            s = (s + 0.5 * oct * fast_sin(2.0 * phase)) / (1.0 + 0.25 * oct);
        }
        s
    }

    #[inline]
    fn saturate(&self, x: f32) -> f32 {
        let g = self.drive_gain;
        let y = match self.drive_kind {
            1 => {
                let norm = tanh_approx(g - TUBE_BIAS) + tanh_approx(TUBE_BIAS);
                (tanh_approx(g * x + TUBE_BIAS) - tanh_approx(TUBE_BIAS)) / norm
            }
            2 => shape_by(1, x, g),
            3 => shape_by(2, x, g),
            _ => shape_by(0, x, g),
        };
        x + self.drive_mix * (y - x)
    }

    /// The waveshaper at four times the sample rate: one sample in, one out.
    #[inline]
    fn drive_stage(&mut self, x: f32) -> f32 {
        let (a, b) = self.up_a.tick(x);
        let (a0, a1) = self.up_b.tick(a);
        let (b0, b1) = self.up_b.tick(b);
        let y = [
            self.saturate(a0),
            self.saturate(a1),
            self.saturate(b0),
            self.saturate(b1),
        ];
        let d0 = self.down_b.tick(y[0], y[1]);
        let d1 = self.down_b.tick(y[2], y[3]);
        self.down_a.tick(d0, d1)
    }

    fn clear_tone_path(&mut self) {
        self.up_a.reset();
        self.up_b.reset();
        self.down_a.reset();
        self.down_b.reset();
        self.dc.reset();
        self.tone_lp.reset();
        self.click_bp.reset();
    }
}

impl Synth for Sub808 {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.dc = DcBlock::new(sample_rate);
        self.vel_k = 1.0 - (-1.0 / (0.0015 * sample_rate)).exp();
        self.ctrl = 0;
    }

    fn reset(&mut self) {
        self.active = false;
        self.env.reset();
        self.pitch_env.reset();
        self.ghost_amp = 0.0;
        self.click_env = 0.0;
        self.stack_len = 0;
        self.last_key = None;
        self.glide_left = 0;
        self.dt_step = 0.0;
        self.clear_tone_path();
        self.ctrl = 0;
    }

    fn param_changed(&mut self, id: ParamId, v: f32, imm: bool) {
        match id {
            SHAPE => self.shape.set(v, imm),
            OCTAVE => self.octave.set(v, imm),
            PUNCH => self.punch.set(v, imm),
            PUNCH_TIME => {
                self.punch_time = v;
                self.pitch_env.set_decay(v, self.sr);
            }
            FINE => self.fine.set(v, imm),
            GLIDE_MODE => self.glide_mode = (v as usize).min(2),
            GLIDE_TIME => self.glide_time = v,
            ATTACK => self.attack = v,
            HOLD => self.hold = v,
            DECAY => self.decay = v,
            SUSTAIN => self.sustain = v,
            RELEASE => self.release = v,
            NOTE_OFF => self.ring = v >= 0.5,
            DECAY_TRACK => self.decay_track = v,
            DRIVE_TYPE => self.drive_kind = (v as usize).min(3),
            DRIVE => self.drive.set(v, imm),
            TONE => self.tone.set(v.ln(), imm),
            CLICK => self.click.set(v, imm),
            CLICK_TONE => self.click_tone.set(v.ln(), imm),
            GAIN => self.gain.set(v, imm),
            VELOCITY => self.vel_sens = v,
            _ => {}
        }
        if matches!(
            id,
            DECAY | SUSTAIN | RELEASE | NOTE_OFF | DECAY_TRACK | ATTACK | HOLD
        ) {
            self.apply_times(self.target_key);
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
        self.voice_id = id;
        let key = key as f32;

        // A slide needs something still sounding to bend.
        let legato = self.glide_mode != 0 && overlapping && self.active;
        if legato {
            self.aim(key, true);
            self.apply_times(key);
        } else {
            let from = self.last_key.filter(|_| self.glide_mode == 2);
            if let Some(from) = from {
                self.cur_key = from;
            }
            self.aim(key, from.is_some());
            if !self.sounding() {
                self.clear_tone_path();
            }
            self.strike(velocity);
            self.apply_times(key);
            self.dt = self.pitch_dt();
            self.dt_step = 0.0;
        }
        self.last_key = Some(key);
    }

    fn note_off(&mut self, _key: u8, _velocity: f32, id: u32) {
        let Some(pos) = self.stack[..self.stack_len].iter().position(|h| h.id == id) else {
            return;
        };
        let was_top = pos + 1 == self.stack_len;
        self.stack.copy_within(pos + 1..self.stack_len, pos);
        self.stack_len -= 1;
        if self.stack_len == 0 {
            self.env.note_off(self.ring);
        } else if was_top {
            // Fall back to the note still held underneath.
            let back = self.stack[self.stack_len - 1];
            self.voice_id = back.id;
            self.aim(back.key as f32, self.glide_mode != 0);
            self.last_key = Some(back.key as f32);
        }
    }

    fn all_notes_off(&mut self) {
        self.stack_len = 0;
        self.env.release();
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
            if !self.sounding() {
                left[n] = 0.0;
                right[n] = 0.0;
                continue;
            }
            let mut s = 0.0;
            if self.active {
                let amp = self.env.tick();
                self.pitch_env.tick();
                self.dt = (self.dt + self.dt_step).clamp(0.0, 0.45);
                s = self.osc(self.phase, self.dt) * amp;
                advance(&mut self.phase, self.dt);
                if self.env.stage == Stage::Idle {
                    self.active = false;
                }
            }
            if self.ghost_amp > 0.0 {
                s += self.osc(self.ghost_phase, self.ghost_dt) * self.ghost_amp;
                advance(&mut self.ghost_phase, self.ghost_dt);
                self.ghost_amp = (self.ghost_amp - self.ghost_step).max(0.0);
            }
            let driven = self.drive_stage(s);
            let mut y = self.tone_lp.tick(self.dc.tick(driven)).low;
            if self.click_env > 0.0 {
                let level = self.click.get();
                if level > 1.0e-4 {
                    let hiss = self.click_bp.tick(self.noise.white()).band_unity;
                    let blip = fast_sin(self.click_phase);
                    advance(&mut self.click_phase, self.click_dt);
                    // Fades in over the first samples so it never steps.
                    let fade = 1.0 - self.click_env.powi(8);
                    y += (0.55 * hiss + 0.45 * blip) * self.click_env * fade * level * 0.9;
                }
                self.click_env *= self.click_coef;
                if self.click_env < 1.0e-4 {
                    self.click_env = 0.0;
                }
            }
            self.vel_cur += (self.vel_level - self.vel_cur) * self.vel_k;
            let out = soft_ceiling(y * 0.75 * self.vel_cur * self.out_gain * self.xgain);
            left[n] = out;
            right[n] = out;
        }
    }

    fn expression(&mut self) -> &mut Expression {
        &mut self.expr
    }

    fn is_silent(&self) -> bool {
        !self.sounding()
    }
}
