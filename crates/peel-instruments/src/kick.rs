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

use crate::acoustic::{Adaa, HitRng, Mode, Pulse, analog_slot};
use crate::common::{
    CTRL, Expression, Fade, Instrument, Pool, PoolVoice, Ramp, STEAL_FADE_MS, Synth,
    TIMBRE_OCTAVES, drive_curve, key_offset, lerp, ramp_coef, shape, st_ratio, vel_gain,
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
const ACOUSTIC: ParamId = 13;
const CIRCUIT: ParamId = 14;

static PARAMS: [ParamDescriptor; 14] = [
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
    ParamDescriptor::toggle(ACOUSTIC, "body.acoustic", "Acoustic", "Body", false),
    ParamDescriptor::toggle(CIRCUIT, "body.circuit", "Circuit", "Body", false),
];

pub static DESC: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.drum.kick",
    name: "Kick",
    description: "Kick drum: electronic, drum-machine circuit or acoustic model",
    category: "Drums",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 15] = [
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
        ],
    },
    Preset {
        name: "808 Long",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
            ("body.acoustic", 0.0),
            ("body.circuit", 0.0),
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
    Preset {
        name: "Acoustic Kick",
        params: &[
            ("body.acoustic", 1.0),
            ("body.circuit", 0.0),
            ("pitch.freq", 55.0),
            ("pitch.sweep", 2.5),
            ("pitch.snap", 2.0),
            ("pitch.sweep_time", 0.05),
            ("body.decay", 0.4),
            ("body.drive", 0.08),
            ("body.clip", 0.15),
            ("body.tone", 6_500.0),
            ("click.level", 0.45),
            ("click.tone", 2_600.0),
            ("out.velocity", 0.7),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Metal Kick",
        params: &[
            ("body.acoustic", 1.0),
            ("body.circuit", 0.0),
            ("pitch.freq", 62.0),
            ("pitch.sweep", 2.0),
            ("pitch.snap", 3.0),
            ("pitch.sweep_time", 0.035),
            ("body.decay", 0.26),
            ("body.drive", 0.15),
            ("body.clip", 0.3),
            ("body.tone", 11_000.0),
            ("click.level", 0.85),
            ("click.tone", 4_500.0),
            ("out.velocity", 0.45),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "909 Kick",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 1.0),
            ("pitch.freq", 53.0),
            ("pitch.sweep", 24.0),
            ("pitch.snap", 10.0),
            ("pitch.sweep_time", 0.03),
            ("body.decay", 0.45),
            ("body.drive", 0.35),
            ("body.clip", 0.3),
            ("body.tone", 10_000.0),
            ("click.level", 0.6),
            ("click.tone", 3_600.0),
            ("out.velocity", 0.65),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "808 Kick",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 1.0),
            ("pitch.freq", 47.0),
            ("pitch.sweep", 7.0),
            ("pitch.snap", 5.0),
            ("pitch.sweep_time", 0.02),
            ("body.decay", 1.1),
            ("body.drive", 0.1),
            ("body.clip", 0.1),
            ("body.tone", 5_000.0),
            ("click.level", 0.35),
            ("click.tone", 2_200.0),
            ("out.velocity", 0.55),
            ("out.level", 0.0),
        ],
    },
    Preset {
        name: "Hard Kick",
        params: &[
            ("body.acoustic", 0.0),
            ("body.circuit", 1.0),
            ("pitch.freq", 55.0),
            ("pitch.sweep", 30.0),
            ("pitch.snap", 14.0),
            ("pitch.sweep_time", 0.04),
            ("body.decay", 0.32),
            ("body.drive", 0.8),
            ("body.clip", 0.75),
            ("body.tone", 14_000.0),
            ("click.level", 0.55),
            ("click.tone", 4_000.0),
            ("out.velocity", 0.5),
            ("out.level", 0.0),
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
    /// Aftertouch gain, 1.0 when nothing is pressed.
    xgain: f32,
    /// Pitch bend ratio, 1.0 when centred.
    bend: f32,
    /// Tone filter multiplier from timbre, 1.0 at neutral.
    tone_r: f32,
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
            xgain: 1.0,
            bend: 1.0,
            tone_r: 1.0,
        }
    }

    #[inline]
    fn tick(&mut self, sr_inv: f32, p: &Stage) -> f32 {
        let freq =
            (self.f0 + self.f_span * self.pitch_env + self.f_snap * self.snap_env) * self.bend;
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
        let out = y * self.gain * self.fade.tick() * self.xgain;
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

/// Acoustic kick modes relative to the fundamental: the batter head's
/// lowest mode, the resonant head a little sharp of it (the slow beating in
/// the tail), higher head modes pulled together by air loading, and two
/// shell modes that give the knock.
const AK_RATIO: [f32; 9] = [1.0, 1.04, 1.47, 1.98, 2.44, 2.9, 3.42, 4.6, 7.1];
const AK_GAIN: [f32; 9] = [1.0, 0.3, 0.4, 0.26, 0.19, 0.13, 0.09, 0.06, 0.04];
/// Decay of each mode as a multiple of the Decay control. The upper head
/// modes die much sooner than the fundamental, so the hit has a short
/// resonant knock rather than a long falling sine.
const AK_DECAY: [f32; 9] = [1.0, 1.2, 0.4, 0.3, 0.24, 0.19, 0.15, 0.2, 0.12];
/// How strongly strike position moves each mode (off-centre strokes favour
/// the non-axisymmetric modes).
const AK_POS: [f32; 9] = [-0.15, -0.15, 0.9, 0.6, 0.9, 0.7, 0.9, 0.0, 0.0];
/// Head modes that follow the pitch glide; the rest are shell modes.
const AK_HEAD: usize = 7;
/// Beater slap modes relative to the Click Tone, and their decay (s).
const AK_SLAP: [f32; 3] = [0.55, 0.83, 1.21];
const AK_SLAP_T60: [f32; 3] = [0.035, 0.024, 0.016];

/// The acoustic kick: a beater pulse driving head and shell modes, a slap
/// of the beater on the head, and the same drive, tone, clip and DC stages
/// as the electronic kick.
#[derive(Clone, Copy)]
struct AcKickVoice {
    active: bool,
    modes: [Mode; 9],
    base: [f32; 9],
    weight: [f32; 9],
    pulse: Pulse,
    slap: [Mode; 3],
    slap_w: [f32; 3],
    slap_pulse: Pulse,
    noise: Noise,
    band: Svf,
    hiss: Decay,
    hiss_atk: f32,
    hiss_atk_inc: f32,
    click_gain: f32,
    span: f32,
    pitch_env: f32,
    pitch_coef: f32,
    snap: f32,
    snap_env: f32,
    snap_coef: f32,
    lp: OnePole,
    dc: OnePole,
    bright: f32,
    push: f32,
    gain: f32,
    fade: Fade,
    xgain: f32,
    bend: f32,
    tone_r: f32,
}

impl AcKickVoice {
    fn new() -> AcKickVoice {
        AcKickVoice {
            active: false,
            modes: [Mode::new(); 9],
            base: [50.0; 9],
            weight: [0.0; 9],
            pulse: Pulse::new(),
            slap: [Mode::new(); 3],
            slap_w: [0.0; 3],
            slap_pulse: Pulse::new(),
            noise: Noise::new(11),
            band: Svf::new(),
            hiss: Decay::new(),
            hiss_atk: 0.0,
            hiss_atk_inc: 1.0,
            click_gain: 0.0,
            span: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            snap: 0.0,
            snap_env: 0.0,
            snap_coef: 0.0,
            lp: OnePole::new(),
            dc: OnePole::new(),
            bright: 1.0,
            push: 1.0,
            gain: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            bend: 1.0,
            tone_r: 1.0,
        }
    }

    /// Moves the head modes with the tension glide; control rate.
    fn retune(&mut self, sr_inv: f32) {
        let m = (1.0 + self.span * self.pitch_env + self.snap * self.snap_env) * self.bend;
        for k in 0..AK_HEAD {
            self.modes[k].tune(self.base[k] * m, sr_inv);
        }
        for m in self.modes.iter_mut().chain(self.slap.iter_mut()) {
            m.flush();
        }
    }

    #[inline]
    fn tick(&mut self, p: &Stage) -> f32 {
        self.pitch_env *= self.pitch_coef;
        self.snap_env *= self.snap_coef;
        let x = self.pulse.tick();
        let mut body = 0.0;
        for (m, &w) in self.modes.iter_mut().zip(self.weight.iter()) {
            body += m.tick(x * w);
        }
        let body = lerp(
            body,
            shape(body * self.push, p.drive_g, p.drive_norm),
            p.drive_mix,
        );
        let xs = self.slap_pulse.tick();
        let mut slap = 0.0;
        for (m, &w) in self.slap.iter_mut().zip(self.slap_w.iter()) {
            slap += m.tick(xs * w);
        }
        // The beater noise fades in from zero so the hit never starts on a step.
        let atk = self.hiss_atk;
        self.hiss_atk = (atk + self.hiss_atk_inc).min(1.0);
        let hiss = self.band.tick(self.noise.white()).band_unity * self.hiss.tick() * atk;
        let y = self.lp.low(body + (slap + 0.35 * hiss) * self.click_gain);
        let y = lerp(y, soft_clip(y, p.clip_g, p.clip_norm), p.clip_mix);
        let y = self.dc.high(y);
        let out = y * self.gain * self.fade.tick() * self.xgain;
        if self.fade.is_done()
            || (!self.pulse.busy()
                && self.modes[0].amp() < 1.0e-4
                && self.modes[1].amp() < 1.0e-4
                && self.modes[2].amp() < 1.0e-4
                && self.slap[0].amp() < 1.0e-4
                && self.hiss.is_below(1.0e-4))
        {
            self.active = false;
        }
        out
    }
}

impl PoolVoice for AcKickVoice {
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

/// The circuit kick: a sine with a two-stage pitch envelope like the
/// electronic kick, but with a hold-then-decay amplitude (the punch of a
/// 909 or a modern kick), velocity as accent (more pitch, more drive), a
/// trigger tick shaped by a short pulse, and anti-aliased drive and clip.
#[derive(Clone, Copy)]
struct CkKickVoice {
    active: bool,
    phase: f32,
    f0: f32,
    f_span: f32,
    f_snap: f32,
    pitch_env: f32,
    pitch_coef: f32,
    snap_env: f32,
    snap_coef: f32,
    env: f32,
    atk_inc: f32,
    rising: bool,
    /// Per-sample approach of the pitch envelopes back to the top after a
    /// retrigger in a sounding voice (0 when not gliding up).
    pitch_rise: f32,
    hold: u32,
    coef: f32,
    tick_pulse: Pulse,
    tick_hp: OnePole,
    tick_gain: f32,
    click_atk: f32,
    click_atk_inc: f32,
    band: Svf,
    noise: Noise,
    nz_env: f32,
    nz_coef: f32,
    nz_gain: f32,
    drive: Adaa,
    clip: Adaa,
    lp: OnePole,
    dc: OnePole,
    bright: f32,
    push: f32,
    gain: f32,
    fade: Fade,
    xgain: f32,
    bend: f32,
    tone_r: f32,
}

impl CkKickVoice {
    fn new() -> CkKickVoice {
        CkKickVoice {
            active: false,
            phase: 0.0,
            f0: 50.0,
            f_span: 0.0,
            f_snap: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            snap_env: 0.0,
            snap_coef: 0.0,
            env: 0.0,
            atk_inc: 1.0,
            rising: false,
            pitch_rise: 0.0,
            hold: 0,
            coef: 0.0,
            tick_pulse: Pulse::new(),
            tick_hp: OnePole::new(),
            tick_gain: 0.0,
            click_atk: 0.0,
            click_atk_inc: 1.0,
            band: Svf::new(),
            noise: Noise::new(51),
            nz_env: 0.0,
            nz_coef: 0.0,
            nz_gain: 0.0,
            drive: Adaa::new(),
            clip: Adaa::new(),
            lp: OnePole::new(),
            dc: OnePole::new(),
            bright: 1.0,
            push: 1.0,
            gain: 1.0,
            fade: Fade::new(),
            xgain: 1.0,
            bend: 1.0,
            tone_r: 1.0,
        }
    }

    #[inline]
    fn tick(&mut self, sr_inv: f32, p: &Stage) -> f32 {
        let freq =
            (self.f0 + self.f_span * self.pitch_env + self.f_snap * self.snap_env) * self.bend;
        if self.pitch_rise > 0.0 {
            // A retrigger glides the pitch back up instead of jumping, so
            // the waveform never kinks.
            self.pitch_env += (1.0 - self.pitch_env) * self.pitch_rise;
            self.snap_env += (1.0 - self.snap_env) * self.pitch_rise;
            if self.pitch_env > 0.995 {
                self.pitch_rise = 0.0;
            }
        } else {
            self.pitch_env *= self.pitch_coef;
            self.snap_env *= self.snap_coef;
        }
        if self.rising {
            self.env += self.atk_inc;
            if self.env >= 1.0 {
                self.env = 1.0;
                self.rising = false;
            }
        } else if self.hold > 0 {
            self.hold -= 1;
        } else {
            self.env *= self.coef;
        }
        let sine = fast_sin(self.phase) * self.env;
        self.phase += (freq * sr_inv).min(0.45);
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let body = lerp(
            sine,
            self.drive.tick(sine * self.push * p.drive_g) * p.drive_norm,
            p.drive_mix,
        );
        let atk = self.click_atk;
        self.click_atk = (atk + self.click_atk_inc).min(1.0);
        let tick = self.tick_hp.high(self.tick_pulse.tick()) * self.tick_gain;
        let nz = self.band.tick(self.noise.white()).band_unity * self.nz_env * self.nz_gain;
        self.nz_env *= self.nz_coef;
        let y = self.lp.low(body + (tick + nz) * atk);
        let y = lerp(y, self.clip.tick(y * p.clip_g) * p.clip_norm, p.clip_mix);
        let y = self.dc.high(y);
        let out = y * self.gain * self.fade.tick() * self.xgain;
        if self.fade.is_done()
            || (!self.rising
                && self.hold == 0
                && self.env < 1.0e-4
                && self.nz_env < 1.0e-4
                && !self.tick_pulse.busy())
        {
            self.active = false;
        }
        out
    }
}

impl PoolVoice for CkKickVoice {
    fn is_active(&self) -> bool {
        self.active
    }
    fn level(&self) -> f32 {
        self.env
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
    expr: Expression,
    sr: f32,
    pool: Pool<KickVoice>,
    ac: Pool<AcKickVoice>,
    ck: Pool<CkKickVoice>,
    acoustic: bool,
    circuit: bool,
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
            expr: Expression::new(),
            sr: 48_000.0,
            pool: Pool::new(4, 1, StealPolicy::Oldest, 48_000.0, KickVoice::new),
            ac: Pool::new(4, 1, StealPolicy::Oldest, 48_000.0, AcKickVoice::new),
            ck: Pool::new(4, 1, StealPolicy::Oldest, 48_000.0, CkKickVoice::new),
            acoustic: false,
            circuit: false,
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
        self.drive.tick(k);
        self.clip.tick(k);
        self.tone.tick(k);
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
        });
    }

    /// Starts a circuit hit.
    fn ck_note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let sr = self.sr;
        self.pool.release_all(|v| v.fade_out(sr));
        self.ac.release_all(|v| v.fade_out(sr));
        // Like the circuit, a retrigger carries on in the same voice: the
        // phase runs on and the envelope rises again from where it is, so a
        // fast run never stacks two hits on top of each other.
        let (slot, warm) = analog_slot(&mut self.ck, key, id, true, |v| v.fade.is_fading());
        self.hits = self.hits.wrapping_add(1);
        let f0 = (self.pitch * st_ratio(key_offset(key))).clamp(15.0, 0.4 * sr);
        // Accent: harder hits sweep further and push the drive harder.
        let sweep_ratio = st_ratio(self.sweep * lerp(0.75, 1.05, velocity));
        let snap_ratio = st_ratio(self.snap * lerp(0.55, 1.05, velocity));
        let f_span = (f0 * (sweep_ratio - 1.0)).min(0.4 * sr - f0).max(0.0);
        let f_snap = (f0 * sweep_ratio * (snap_ratio - 1.0))
            .min(0.4 * sr - f0 - f_span)
            .max(0.0);
        let x = self.expr.voice(id, key);
        let tone_hz = self.tone_hz;
        let hits = self.hits;
        let v = &mut self.ck.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        v.bend = x.ratio;
        v.tone_r = x.tone_ratio(TIMBRE_OCTAVES);
        if !warm {
            v.phase = 0.0;
            v.env = 0.0;
            v.drive.reset();
            v.clip.reset();
            v.tick_hp.reset();
            v.band.reset();
            v.lp.reset();
            v.dc.reset();
        }
        v.f0 = f0;
        v.f_span = f_span;
        v.f_snap = f_snap;
        if warm {
            v.pitch_rise = 1.0 - (-1.0 / (0.000_7 * sr)).exp();
        } else {
            v.pitch_env = 1.0;
            v.snap_env = 1.0;
            v.pitch_rise = 0.0;
        }
        v.pitch_coef = (-1.0 / (self.sweep_time * sr)).exp();
        v.snap_coef = (-1.0 / (0.003 * sr)).exp();
        v.rising = true;
        v.atk_inc = 1.0 / (0.000_15 * sr).max(1.0);
        // Hold the peak for a slice of the decay: the body hits at full
        // level before it starts to fall.
        v.hold = ((self.decay * 0.08).clamp(0.003, 0.04) * lerp(0.75, 1.0, velocity) * sr) as u32;
        v.coef = peel_dsp::envelope::t60_to_coef(self.decay * 0.86, sr);
        let hard = (self.click_tone / 3_200.0).sqrt();
        v.tick_pulse.start((0.000_09 / hard).max(2.0 / sr), sr);
        v.tick_hp.set_cutoff(900.0, sr);
        v.tick_gain = self.click * 0.7 * velocity.powf(1.5) * x.tone_ratio(1.0);
        v.click_atk = 0.0;
        v.click_atk_inc = 1.0 / (0.000_2 * sr);
        v.band.set_q((self.click_tone * 1.2).min(0.4 * sr), 0.6, sr);
        v.noise.reseed(hits.wrapping_add(300));
        v.nz_env = 1.0;
        v.nz_coef = (-1.0 / (0.001_2 * sr)).exp();
        v.nz_gain = self.click * 0.35 * velocity * x.tone_ratio(1.0);
        v.bright = lerp(0.3, 1.0, velocity.sqrt());
        v.push = lerp(0.6, 1.15, velocity);
        v.lp.set_cutoff(tone_hz * v.bright * v.tone_r, sr);
        v.dc.set_cutoff(18.0, sr);
        // The held peak carries more energy than a plain decay; this keeps
        // the circuit presets level with the electronic ones.
        v.gain = 0.8 * vel_gain(velocity, self.vel_sens);
        v.fade.reset();
    }

    /// Starts an acoustic hit.
    fn ac_note_on(&mut self, key: u8, velocity: f32, id: u32) {
        let sr = self.sr;
        // The kick is one drum: a new stroke damps whatever the other models
        // still have ringing.
        self.pool.release_all(|v| v.fade_out(sr));
        self.ck.release_all(|v| v.fade_out(sr));
        let slot = self.ac.begin(key, id);
        self.hits = self.hits.wrapping_add(1);
        let mut rng = HitRng::new(self.hits, 0x4b1c);
        let f0 = (self.pitch * st_ratio(key_offset(key))).clamp(15.0, 0.1 * sr);
        // Harder beaters (higher Click Tone) and harder strokes shorten the
        // contact, which reaches further up the head's modes.
        let hardness = (3_200.0 / self.click_tone).sqrt();
        let contact = 0.001_1 * lerp(1.45, 0.7, velocity) * hardness * (1.0 + 0.08 * rng.next());
        // Strike position: a little off centre on average, never the same.
        let pos = 0.25 * rng.next();
        let x = self.expr.voice(id, key);
        let tone_hz = self.tone_hz;
        let v = &mut self.ac.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        v.bend = x.ratio;
        v.tone_r = x.tone_ratio(TIMBRE_OCTAVES);
        v.span = st_ratio(self.sweep * velocity.powf(1.5) * (1.0 + 0.1 * rng.next())) - 1.0;
        v.pitch_env = 1.0;
        v.pitch_coef = (-1.0 / (self.sweep_time * sr)).exp();
        v.snap = st_ratio(self.snap * velocity * velocity) - 1.0;
        v.snap_env = 1.0;
        v.snap_coef = (-1.0 / (0.004 * sr)).exp();
        for k in 0..9 {
            v.base[k] = f0 * AK_RATIO[k];
            let t60 = (self.decay * AK_DECAY[k]).max(0.02);
            v.modes[k].start(v.base[k], t60, sr);
            v.weight[k] = AK_GAIN[k] * (1.0 + AK_POS[k] * pos).max(0.0) * (1.0 + 0.06 * rng.next());
        }
        v.retune(1.0 / sr);
        v.pulse.start(contact, sr);
        let slap_hz = self.click_tone * (1.0 + 0.05 * rng.next());
        for k in 0..3 {
            v.slap[k].start((slap_hz * AK_SLAP[k]).min(0.4 * sr), AK_SLAP_T60[k], sr);
            v.slap_w[k] = 1.0 + 0.3 * rng.next();
        }
        v.slap_pulse
            .start(0.000_12 * hardness * lerp(1.3, 0.8, velocity), sr);
        v.noise.reseed(self.hits.wrapping_add(77));
        v.band.set_q((self.click_tone * 1.4).min(0.4 * sr), 0.7, sr);
        v.band.reset();
        v.hiss.set_t60(0.006, sr);
        v.hiss.trigger(1.0, 0.000_25, sr);
        v.hiss_atk = 0.0;
        v.hiss_atk_inc = 1.0 / (0.000_3 * sr);
        v.click_gain =
            self.click * 0.45 * velocity.powf(1.5) * (1.0 + 0.12 * rng.next()) * x.tone_ratio(1.0);
        v.bright = lerp(0.35, 1.0, velocity.sqrt());
        v.push = lerp(0.7, 1.0, velocity);
        v.lp.set_cutoff(tone_hz * v.bright * v.tone_r, sr);
        v.lp.reset();
        v.dc.set_cutoff(18.0, sr);
        v.dc.reset();
        v.gain = vel_gain(velocity, self.vel_sens) * (1.0 + 0.03 * rng.next());
        v.fade.reset();
    }
}

impl Synth for Kick {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = sample_rate;
        self.ramp_k = ramp_coef(sample_rate);
        self.pool = Pool::new(4, 1, StealPolicy::Oldest, sample_rate, KickVoice::new);
        self.ac = Pool::new(4, 1, StealPolicy::Oldest, sample_rate, AcKickVoice::new);
        self.ck = Pool::new(4, 1, StealPolicy::Oldest, sample_rate, CkKickVoice::new);
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
            ACOUSTIC => self.acoustic = v >= 0.5,
            CIRCUIT => self.circuit = v >= 0.5,
            _ => {}
        }
        if immediate {
            self.derive();
        }
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: u32) {
        // Acoustic wins if both new switches are on.
        if self.acoustic {
            self.ac_note_on(key, velocity, id);
            return;
        }
        if self.circuit {
            self.ck_note_on(key, velocity, id);
            return;
        }
        let fade_sr = self.sr;
        self.ac.release_all(|v| v.fade_out(fade_sr));
        self.ck.release_all(|v| v.fade_out(fade_sr));
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
        let x = self.expr.voice(id, key);
        let v = &mut self.pool.voices[slot];
        v.active = true;
        v.xgain = x.gain();
        v.bend = x.ratio;
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
        // Timbre also sets how hard the beater clicks.
        v.click_gain = self.click * 0.9 * lerp(0.35, 1.0, velocity) * x.tone_ratio(1.0);
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
        v.tone_r = x.tone_ratio(TIMBRE_OCTAVES);
        v.lp.set_cutoff(self.tone_hz * v.bright * v.tone_r, sr);
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
                out += v.tick(sr_inv, &self.stage);
            }
            for v in self.ac.voices.iter_mut().filter(|v| v.active) {
                out += v.tick(&self.stage);
            }
            for v in self.ck.voices.iter_mut().filter(|v| v.active) {
                out += v.tick(sr_inv, &self.stage);
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
