//! Algorithmic reverb built on a sixteen-line feedback delay network.
//!
//! Signal path: stereo pre-delay, two chains of four Schroeder all-pass
//! diffusers (different lengths for left and right), a sixteen-line FDN
//! with a fast Hadamard feedback matrix, in-loop absorption filters and
//! slowly drifting modulated delay lines, a stereo output tap matrix, low
//! and high cuts on the wet signal, mid/side width, and a linear dry/wet mix.
//!
//! The design follows Jot's FDN reverberator (per-line gain from the
//! requested T60 plus a lossy low-pass for frequency-dependent decay), with
//! the line count, length spread and modulation chosen for modal and echo
//! density: sixteen unrelated lengths between about 30 and 130 ms give around
//! one mode per hertz at medium size, and the Hadamard mix multiplies echoes
//! sixteen-fold per trip, so the tail turns into smooth noise within about
//! a hundred milliseconds instead of ringing.
//!
//! * **Size** scales all line lengths from 0.25x to 2x (an exponential map),
//!   so small values are rooms and large values are halls. Changes glide
//!   rather than click.
//! * **Decay** is the time the tail takes to fall by 60 dB, set per line so
//!   it is independent of size.
//! * **Damping** lowers the in-loop low-pass cutoff from 20 kHz to about
//!   1.2 kHz, so highs die faster than lows like in real rooms.
//! * **Diffusion** is the all-pass gain: 0 keeps early echoes distinct,
//!   100% smears them into a smooth wash.
//! * **Early** sets the level of six discrete early reflections per side,
//!   read from the pre-delay line with times that scale with size. They fill
//!   the gap before the FDN's shortest line speaks.
//! * **Modulation** drifts the line lengths by a few samples with two
//!   unrelated slow sine rates per line (a quasi-random walk with no
//!   audible period), which breaks up metallic ringing and gives the tail a
//!   gentle chorus without a pitch wobble.

use peel_dsp::biquad::{OnePoleHighpass, OnePoleLowpass};
use peel_dsp::delay::{DelayLine, TimeGlide, fast_sin_turns};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::reverb::Allpass;
use peel_dsp::util::{Smoothed, sanitize};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

pub const ID_SIZE: ParamId = 0;
pub const ID_DECAY: ParamId = 1;
pub const ID_PREDELAY: ParamId = 2;
pub const ID_DAMPING: ParamId = 3;
pub const ID_DIFFUSION: ParamId = 4;
pub const ID_MODULATION: ParamId = 5;
pub const ID_WIDTH: ParamId = 6;
pub const ID_LOW_CUT: ParamId = 7;
pub const ID_HIGH_CUT: ParamId = 8;
pub const ID_MIX: ParamId = 9;
pub const ID_EARLY: ParamId = 10;

static PARAMS: [ParamDescriptor; 11] = [
    ParamDescriptor::linear(
        ID_SIZE,
        "size",
        "Size",
        "Space",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ID_DECAY,
        "decay",
        "Decay",
        "Space",
        0.2,
        30.0,
        2.5,
        Unit::Seconds,
    ),
    ParamDescriptor::power(
        ID_PREDELAY,
        "predelay",
        "Pre-delay",
        "Space",
        0.0,
        250.0,
        12.0,
        Unit::Milliseconds,
        2.0,
    ),
    ParamDescriptor::linear(
        ID_DAMPING,
        "damping",
        "Damping",
        "Tone",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::log(
        ID_LOW_CUT,
        "lowcut",
        "Low Cut",
        "Tone",
        20.0,
        1_000.0,
        20.0,
        Unit::Hertz,
    ),
    ParamDescriptor::log(
        ID_HIGH_CUT,
        "highcut",
        "High Cut",
        "Tone",
        1_000.0,
        20_000.0,
        20_000.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_DIFFUSION,
        "diffusion",
        "Diffusion",
        "Character",
        0.0,
        1.0,
        0.75,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_EARLY,
        "early",
        "Early",
        "Character",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_MODULATION,
        "modulation",
        "Modulation",
        "Character",
        0.0,
        1.0,
        0.3,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_WIDTH,
        "width",
        "Width",
        "Output",
        0.0,
        1.5,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_MIX,
        "mix",
        "Mix",
        "Output",
        0.0,
        1.0,
        0.25,
        Unit::Percent,
    ),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.reverb",
    name: "Reverb",
    description: "Lush algorithmic reverb with modulated feedback delay network",
    category: "Reverb",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 16] = [
    Preset {
        name: "Room",
        params: &[],
    },
    Preset {
        name: "Small Plate",
        params: &[
            ("size", 0.3),
            ("decay", 1.6),
            ("predelay", 4.0),
            ("damping", 0.25),
            ("diffusion", 0.95),
            ("modulation", 0.45),
            ("lowcut", 120.0),
            ("mix", 0.22),
        ],
    },
    Preset {
        name: "Warm Hall",
        params: &[
            ("size", 0.72),
            ("decay", 3.8),
            ("predelay", 24.0),
            ("damping", 0.6),
            ("diffusion", 0.8),
            ("modulation", 0.3),
            ("lowcut", 80.0),
            ("mix", 0.26),
        ],
    },
    Preset {
        name: "Big Cathedral",
        params: &[
            ("size", 0.95),
            ("decay", 9.0),
            ("predelay", 38.0),
            ("damping", 0.55),
            ("diffusion", 0.85),
            ("modulation", 0.35),
            ("lowcut", 140.0),
            ("highcut", 9_000.0),
            ("mix", 0.3),
        ],
    },
    Preset {
        name: "Ambient Wash",
        params: &[
            ("size", 0.85),
            ("decay", 14.0),
            ("predelay", 60.0),
            ("damping", 0.4),
            ("diffusion", 1.0),
            ("modulation", 0.7),
            ("lowcut", 200.0),
            ("highcut", 12_000.0),
            ("width", 1.3),
            ("mix", 0.4),
        ],
    },
    Preset {
        name: "Tight Ambience",
        params: &[
            ("size", 0.12),
            ("decay", 0.7),
            ("predelay", 0.0),
            ("damping", 0.5),
            ("diffusion", 0.6),
            ("modulation", 0.15),
            ("mix", 0.18),
        ],
    },
    Preset {
        name: "Big Hall",
        params: &[
            ("size", 0.85),
            ("decay", 4.5),
            ("predelay", 28.0),
            ("damping", 0.5),
            ("diffusion", 0.9),
            ("modulation", 0.35),
            ("lowcut", 110.0),
            ("highcut", 14_000.0),
            ("mix", 0.28),
        ],
    },
    Preset {
        name: "Plate",
        params: &[
            ("size", 0.4),
            ("decay", 2.2),
            ("predelay", 0.0),
            ("damping", 0.3),
            ("diffusion", 1.0),
            ("early", 0.0),
            ("modulation", 0.4),
            ("lowcut", 150.0),
            ("mix", 0.25),
        ],
    },
    Preset {
        name: "Vocal Plate",
        params: &[
            ("size", 0.45),
            ("decay", 1.8),
            ("predelay", 30.0),
            ("damping", 0.4),
            ("diffusion", 1.0),
            ("early", 0.1),
            ("modulation", 0.35),
            ("lowcut", 220.0),
            ("highcut", 11_000.0),
            ("mix", 0.2),
        ],
    },
    Preset {
        name: "Clean Wash",
        params: &[
            ("size", 0.9),
            ("decay", 8.0),
            ("predelay", 50.0),
            ("damping", 0.55),
            ("diffusion", 1.0),
            ("early", 0.0),
            ("modulation", 0.2),
            ("lowcut", 250.0),
            ("highcut", 9_000.0),
            ("width", 1.2),
            ("mix", 0.35),
        ],
    },
    Preset {
        name: "Huge Space",
        params: &[
            ("size", 1.0),
            ("decay", 12.0),
            ("predelay", 40.0),
            ("damping", 0.45),
            ("diffusion", 1.0),
            ("early", 0.25),
            ("modulation", 0.5),
            ("lowcut", 250.0),
            ("highcut", 10_000.0),
            ("width", 1.3),
            ("mix", 0.35),
        ],
    },
    Preset {
        name: "Drum Room",
        params: &[
            ("size", 0.22),
            ("decay", 0.5),
            ("predelay", 0.0),
            ("damping", 0.5),
            ("diffusion", 0.8),
            ("early", 0.6),
            ("modulation", 0.1),
            ("lowcut", 100.0),
            ("mix", 0.2),
        ],
    },
    Preset {
        name: "Neon Hall",
        params: &[
            ("size", 0.8),
            ("decay", 3.8),
            ("predelay", 28.0),
            ("damping", 0.35),
            ("lowcut", 220.0),
            ("highcut", 9_000.0),
            ("diffusion", 0.85),
            ("early", 0.35),
            ("modulation", 0.6),
            ("width", 1.2),
            ("mix", 0.3),
        ],
    },
    Preset {
        name: "Snare Plate",
        params: &[
            ("size", 0.55),
            ("decay", 1.6),
            ("predelay", 10.0),
            ("damping", 0.25),
            ("lowcut", 250.0),
            ("highcut", 11_000.0),
            ("diffusion", 0.9),
            ("early", 0.7),
            ("modulation", 0.25),
            ("width", 1.1),
            ("mix", 0.45),
        ],
    },
    Preset {
        name: "Dark Space",
        params: &[
            ("size", 0.65),
            ("decay", 2.8),
            ("predelay", 35.0),
            ("damping", 0.6),
            ("lowcut", 300.0),
            ("highcut", 6_500.0),
            ("diffusion", 0.8),
            ("early", 0.3),
            ("modulation", 0.4),
            ("width", 1.1),
            ("mix", 0.28),
        ],
    },
    Preset {
        name: "Dusty Room",
        params: &[
            ("size", 0.25),
            ("decay", 0.7),
            ("predelay", 6.0),
            ("damping", 0.8),
            ("lowcut", 180.0),
            ("highcut", 3_800.0),
            ("diffusion", 0.7),
            ("early", 0.6),
            ("modulation", 0.15),
            ("width", 0.9),
            ("mix", 0.2),
        ],
    },
];

/// Lines in the feedback delay network.
const N: usize = 16;

/// Base line lengths in milliseconds at size scale 1.0. Roughly geometric
/// from 30 to 130 ms with a small irregular nudge so no two lines share a
/// common factor, then shuffled so the short and long lines alternate
/// across the stereo taps.
const LINE_MS: [f32; N] = [
    30.9, 68.0, 42.0, 93.1, 49.1, 119.9, 33.2, 71.6, 55.4, 124.7, 35.3, 77.9, 56.8, 108.6, 42.9,
    91.7,
];
/// Left and right input diffuser lengths in milliseconds.
const DIFFUSER_MS: [[f32; 4]; 2] = [[4.77, 7.31, 11.13, 16.07], [5.21, 8.17, 12.39, 17.61]];

/// Early reflection tap times in milliseconds (before size scaling) and
/// their gains, left then right. Irregular on purpose.
const ER_MS: [[f32; 6]; 2] = [
    [7.3, 12.9, 19.7, 27.1, 36.3, 46.9],
    [9.1, 15.3, 22.3, 30.7, 40.1, 52.3],
];
const ER_GAIN: [[f32; 6]; 2] = [
    [0.9, -0.75, 0.6, 0.5, -0.4, 0.3],
    [0.85, 0.7, -0.6, 0.45, -0.4, -0.3],
];

/// FDN injection signs. Even lines take the left diffuser, odd lines the
/// right one. The pattern is not a Hadamard row, so the first mixing pass
/// spreads the input over all lines instead of concentrating it.
const INJECT: [f32; N] = [
    1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0,
];
/// Feedback sign flips applied before the Hadamard matrix. A diagonal sign
/// matrix keeps the loop orthogonal but breaks the matrix's regularity.
const FLIP: [f32; N] = [
    1.0, -1.0, -1.0, 1.0, 1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0, 1.0,
];

/// Row `r` of the sixteen-point Sylvester Hadamard matrix.
const fn hadamard_row(r: usize) -> [f32; N] {
    let mut out = [0.0; N];
    let mut c = 0;
    while c < N {
        out[c] = if (r & c).count_ones() & 1 == 0 {
            1.0
        } else {
            -1.0
        };
        c += 1;
    }
    out
}
/// Output taps: two orthogonal Hadamard rows decorrelate left and right.
const TAP_L: [f32; N] = hadamard_row(5);
const TAP_R: [f32; N] = hadamard_row(10);

/// 1/sqrt(16) keeps the tap sum at unity RMS for uncorrelated lines; the extra
/// 3 dB puts a sustained wet signal at a useful level next to the dry one.
const TAP_NORM: f32 = 0.25 * 1.41;
/// Per-line injection gain, calibrated so a sustained signal comes out at the
/// same level as it did with the former eight-line network.
const INJECT_GAIN: f32 = 0.53;

/// In-place orthonormal 16-point Walsh-Hadamard transform.
#[inline]
fn hadamard16(x: &mut [f32; N]) {
    let mut h = 8;
    while h >= 1 {
        let mut base = 0;
        while base < N {
            for i in base..base + h {
                let (a, b) = (x[i], x[i + h]);
                x[i] = a + b;
                x[i + h] = a - b;
            }
            base += 2 * h;
        }
        h /= 2;
    }
    for v in x.iter_mut() {
        *v *= 0.25;
    }
}

/// Samples between recomputations of modulation, gliding lengths and
/// per-line decay gains. Delay offsets are interpolated linearly in between.
const CONTROL: usize = 8;

/// Slow modulation rates in hertz, two per line. Distinct and unrelated,
/// so the drift never lines up into an audible pattern.
const MOD_HZ: [[f32; 2]; N] = [
    [0.13, 0.31],
    [0.17, 0.43],
    [0.21, 0.57],
    [0.11, 0.71],
    [0.19, 0.83],
    [0.23, 0.97],
    [0.15, 1.13],
    [0.27, 1.27],
    [0.12, 0.37],
    [0.16, 0.49],
    [0.22, 0.61],
    [0.14, 0.77],
    [0.18, 0.89],
    [0.24, 1.03],
    [0.26, 1.19],
    [0.29, 0.53],
];

/// Sixteen-line feedback delay network with absorption filters and drifting
/// delay lengths.
struct Fdn {
    lines: [DelayLine; N],
    sample_rate: f32,
    max_len: f32,
    len_target: [f32; N],
    len_cur: [f32; N],
    len_coeff: f32,
    /// Current delay including modulation, and its per-sample step.
    delay: [f32; N],
    step: [f32; N],
    t60: f32,
    gains: [f32; N],
    damp_a: f32,
    damp_state: [f32; N],
    mod_depth: f32,
    phase: [[f32; 2]; N],
    inc: [[f32; 2]; N],
    ctrl: usize,
}

impl Fdn {
    fn new(sample_rate: f32, max_len: usize) -> Fdn {
        let lines = std::array::from_fn(|_| DelayLine::new(max_len + 64));
        let mut phase = [[0.0; 2]; N];
        let mut inc = [[0.0; 2]; N];
        for i in 0..N {
            for k in 0..2 {
                // Spread the start phases so the drift is uncorrelated from
                // the first sample.
                phase[i][k] = ((i * 7 + k * 5) % N) as f32 / N as f32;
                inc[i][k] = MOD_HZ[i][k] / sample_rate.max(1.0);
            }
        }
        let default_len = LINE_MS.map(|ms| ms * 0.001 * sample_rate * 0.7);
        let mut f = Fdn {
            lines,
            sample_rate,
            max_len: max_len as f32,
            len_target: default_len,
            len_cur: default_len,
            len_coeff: 1.0 - (-(CONTROL as f32) / (0.15 * sample_rate.max(1.0))).exp(),
            delay: default_len,
            step: [0.0; N],
            t60: 2.0,
            gains: [0.9; N],
            damp_a: 0.0,
            damp_state: [0.0; N],
            mod_depth: 0.0,
            phase,
            inc,
            ctrl: 0,
        };
        f.update_gains();
        f
    }

    /// Sets the base delay of each line in samples. With `immediate` the
    /// lengths jump; otherwise they glide (about 150 ms time constant).
    fn set_lengths(&mut self, lengths: [f32; N], immediate: bool) {
        for (i, len) in lengths.iter().enumerate() {
            self.len_target[i] = len.clamp(8.0, self.max_len);
            if immediate {
                self.len_cur[i] = self.len_target[i];
                self.delay[i] = self.len_target[i];
            }
        }
        if immediate {
            self.update_gains();
            self.step = [0.0; N];
        }
    }

    fn set_t60(&mut self, seconds: f32) {
        self.t60 = seconds.clamp(0.01, 600.0);
    }

    /// In-loop damping low-pass. At or above 0.45 of the sample rate the
    /// filter is bypassed.
    fn set_damping_hz(&mut self, hz: f32) {
        if hz >= self.sample_rate * 0.45 {
            self.damp_a = 0.0;
        } else {
            self.damp_a = (-std::f32::consts::TAU * hz.max(20.0) / self.sample_rate).exp();
        }
    }

    /// Peak delay deviation of the modulation in samples.
    fn set_mod_depth(&mut self, samples: f32) {
        self.mod_depth = samples.clamp(0.0, 32.0);
    }

    fn len_target(&self) -> [f32; N] {
        self.len_target
    }

    fn clear(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        self.damp_state = [0.0; N];
        self.ctrl = 0;
    }

    fn update_gains(&mut self) {
        for i in 0..N {
            self.gains[i] = 10f32.powf(-3.0 * self.len_cur[i] / (self.t60 * self.sample_rate));
        }
    }

    /// Runs every `CONTROL` samples: glides lengths, refreshes the decay
    /// gains and sets up the linear delay ramps for the next stretch.
    fn control(&mut self) {
        for i in 0..N {
            self.len_cur[i] += (self.len_target[i] - self.len_cur[i]) * self.len_coeff;
        }
        self.update_gains();
        let advance = CONTROL as f32;
        for i in 0..N {
            let m = if self.mod_depth > 0.0 {
                let p0 = self.phase[i][0] + self.inc[i][0] * advance;
                let p1 = self.phase[i][1] + self.inc[i][1] * advance;
                self.phase[i][0] = p0 - p0.floor();
                self.phase[i][1] = p1 - p1.floor();
                self.mod_depth
                    * (0.62 * fast_sin_turns(self.phase[i][0])
                        + 0.38 * fast_sin_turns(self.phase[i][1]))
            } else {
                0.0
            };
            let next = (self.len_cur[i] + m).clamp(4.0, self.max_len + 32.0);
            self.step[i] = (next - self.delay[i]) / advance;
        }
    }

    /// Advances one sample. Returns the damped line outputs.
    #[inline]
    fn process(&mut self, input: &[f32; N]) -> [f32; N] {
        if self.ctrl == 0 {
            self.control();
            self.ctrl = CONTROL;
        }
        self.ctrl -= 1;

        let mut fed = [0.0f32; N];
        let mut outs = [0.0f32; N];
        let a = self.damp_a;
        for i in 0..N {
            self.delay[i] += self.step[i];
            let tap = self.lines[i].read_hermite(self.delay[i]);
            let damped = if a > 0.0 {
                sanitize((1.0 - a) * tap + a * self.damp_state[i])
            } else {
                tap
            };
            self.damp_state[i] = damped;
            outs[i] = damped;
            fed[i] = damped * self.gains[i] * FLIP[i];
        }
        hadamard16(&mut fed);
        for i in 0..N {
            self.lines[i].push(sanitize(input[i] + fed[i]));
        }
        outs
    }
}

/// Reverb effect. See the module docs.
pub struct Reverb {
    store: ParamStore,
    sample_rate: f32,
    pre: [DelayLine; 2],
    pre_glide: TimeGlide,
    diffusers: [[Allpass; 4]; 2],
    fdn: Fdn,
    low_cut: [OnePoleHighpass; 2],
    high_cut: [OnePoleLowpass; 2],
    low_cut_on: bool,
    high_cut_on: bool,
    width: Smoothed,
    mix: Smoothed,
    early: Smoothed,
    er_scale: Smoothed,
    silence: SilenceTracker,
}

impl Default for Reverb {
    fn default() -> Self {
        Reverb::new()
    }
}

fn make_diffusers(sample_rate: f32) -> [[Allpass; 4]; 2] {
    std::array::from_fn(|ch| {
        std::array::from_fn(|k| {
            Allpass::new(
                (DIFFUSER_MS[ch][k] * 0.001 * sample_rate).round().max(2.0) as usize,
                0.5,
            )
        })
    })
}

impl Reverb {
    pub fn new() -> Reverb {
        let mut r = Reverb {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            pre: [DelayLine::new(16), DelayLine::new(16)],
            pre_glide: TimeGlide::new(2.0),
            diffusers: make_diffusers(48_000.0),
            fdn: Fdn::new(48_000.0, 64),
            low_cut: [OnePoleHighpass::default(); 2],
            high_cut: [OnePoleLowpass::default(); 2],
            low_cut_on: false,
            high_cut_on: false,
            width: Smoothed::new(1.0),
            mix: Smoothed::new(0.25),
            early: Smoothed::new(0.5),
            er_scale: Smoothed::new(0.7),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut r, 48_000.0, 512);
        r
    }

    fn update_size(&mut self) {
        let size = self.store.get(ID_SIZE);
        let scale = 0.25 * 8f32.powf(size);
        self.er_scale.set_target(scale);
        let lens = LINE_MS.map(|ms| ms * 0.001 * self.sample_rate * scale);
        self.fdn.set_lengths(lens, false);
    }

    fn pre_samples(&self) -> f32 {
        (self.store.get(ID_PREDELAY) * 0.001 * self.sample_rate).max(2.0)
    }
}

impl Fx for Reverb {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        let pre_max = (0.26 * sr + 0.12 * sr) as usize + 8;
        self.pre = [DelayLine::new(pre_max), DelayLine::new(pre_max)];
        self.pre_glide.configure(sr, 0.08, 0.4);
        self.diffusers = make_diffusers(sr);
        // Longest line at the largest size, plus modulation headroom.
        let max_line = (130.0 * 0.001 * sr * 2.0) as usize + 64;
        self.fdn = Fdn::new(sr, max_line);
        self.width.set_ramp(sr, RAMP_SECONDS);
        self.mix.set_ramp(sr, RAMP_SECONDS);
        self.early.set_ramp(sr, RAMP_SECONDS);
        self.er_scale.set_ramp(sr, 0.1);
        refresh_all(self);
        let lens = self.fdn.len_target();
        self.fdn.set_lengths(lens, true);
        let p = self.pre_samples();
        self.pre_glide.set_immediate(p);
        for s in [
            &mut self.width,
            &mut self.mix,
            &mut self.early,
            &mut self.er_scale,
        ] {
            let t = s.target();
            s.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        let sr = self.sample_rate;
        match id {
            ID_SIZE => self.update_size(),
            ID_DECAY => self.fdn.set_t60(value),
            ID_PREDELAY => {
                let p = self.pre_samples();
                self.pre_glide.set_target(p);
            }
            ID_DAMPING => {
                let hz = 20_000.0 * (1_200.0f32 / 20_000.0).powf(value);
                self.fdn.set_damping_hz(hz);
            }
            ID_DIFFUSION => {
                let g = 0.75 * value;
                for chain in &mut self.diffusers {
                    for a in chain {
                        a.set_gain(g);
                    }
                }
            }
            ID_MODULATION => self.fdn.set_mod_depth(value * 0.35e-3 * sr),
            ID_WIDTH => self.width.set_target(value),
            ID_LOW_CUT => {
                self.low_cut_on = value > 20.5;
                for f in &mut self.low_cut {
                    f.set_cutoff(sr, value);
                }
            }
            ID_HIGH_CUT => {
                self.high_cut_on = value < 19_500.0;
                for f in &mut self.high_cut {
                    f.set_cutoff(sr, value);
                }
            }
            ID_MIX => self.mix.set_target(value),
            ID_EARLY => self.early.set_target(value * 0.7),
            _ => {}
        }
    }

    fn clear(&mut self) {
        for l in &mut self.pre {
            l.clear();
        }
        for chain in &mut self.diffusers {
            for a in chain {
                a.clear();
            }
        }
        self.fdn.clear();
        for f in &mut self.low_cut {
            f.reset();
        }
        for f in &mut self.high_cut {
            f.reset();
        }
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (in_l, in_r) = (*l, *r);
            let pd = self.pre_glide.next();
            let pl = self.pre[0].read_hermite(pd);
            let pr = self.pre[1].read_hermite(pd);
            let early = self.early.next();
            let er_scale = self.er_scale.next() * 0.001 * self.sample_rate;
            let (mut er_l, mut er_r) = (0.0f32, 0.0f32);
            if early > 0.0 {
                for k in 0..6 {
                    er_l += ER_GAIN[0][k] * self.pre[0].read_linear(pd + ER_MS[0][k] * er_scale);
                    er_r += ER_GAIN[1][k] * self.pre[1].read_linear(pd + ER_MS[1][k] * er_scale);
                }
                er_l *= early * 0.5;
                er_r *= early * 0.5;
            }
            self.pre[0].push(in_l);
            self.pre[1].push(in_r);

            let mut a = pl;
            let mut b = pr;
            for k in 0..4 {
                a = self.diffusers[0][k].process(a);
                b = self.diffusers[1][k].process(b);
            }

            let mut feed = [0.0f32; N];
            for (i, f) in feed.iter_mut().enumerate() {
                *f = INJECT_GAIN * INJECT[i] * if i % 2 == 0 { a } else { b };
            }
            let taps = self.fdn.process(&feed);
            let mut wl = 0.0;
            let mut wr = 0.0;
            for i in 0..N {
                wl += TAP_L[i] * taps[i];
                wr += TAP_R[i] * taps[i];
            }
            wl = wl * TAP_NORM + er_l;
            wr = wr * TAP_NORM + er_r;

            if self.low_cut_on {
                wl = self.low_cut[0].process(wl);
                wr = self.low_cut[1].process(wr);
            }
            if self.high_cut_on {
                wl = self.high_cut[0].process(wl);
                wr = self.high_cut[1].process(wr);
            }

            let width = self.width.next();
            let mid = (wl + wr) * 0.5;
            let side = (wl - wr) * 0.5 * width;
            let (wl, wr) = (mid + side, mid - side);

            let mix = self.mix.next();
            *l = in_l * (1.0 - mix) + wl * mix;
            *r = in_r * (1.0 - mix) + wr * mix;
        }
    }

    fn tail(&self) -> Option<f32> {
        Some(self.store.get(ID_DECAY) + self.store.get(ID_PREDELAY) * 0.001)
    }

    fn silent(&self) -> bool {
        // Twice the 60 dB time puts the tail at -120 dB.
        let secs = 2.0 * self.store.get(ID_DECAY) + self.store.get(ID_PREDELAY) * 0.001 + 0.1;
        self.silence.quiet_for((secs * self.sample_rate) as u32)
    }
}

impl_device!(Reverb, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;

    fn impulse_response(rv: &mut Reverb, n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        l[0] = 1.0;
        r[0] = 1.0;
        let ctx = ProcessContext::new(SR);
        for (cl, cr) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            rv.process(&ctx, &[], cl, cr);
        }
        (l, r)
    }

    /// Reverse-integrated energy decay curve in dB, normalized to 0 dB.
    fn edc_db(x: &[f32]) -> Vec<f32> {
        let mut acc = 0.0f64;
        let mut out = vec![0.0f32; x.len()];
        for i in (0..x.len()).rev() {
            acc += f64::from(x[i]) * f64::from(x[i]);
            out[i] = acc as f32;
        }
        let total = out[0].max(1e-30);
        out.iter()
            .map(|v| 10.0 * (v / total).max(1e-30).log10())
            .collect()
    }

    fn t60_from_edc(edc: &[f32]) -> f32 {
        let find = |db: f32| edc.iter().position(|v| *v < db).unwrap_or(edc.len()) as f32;
        let (a, b) = (find(-5.0), find(-25.0));
        // 20 dB of decay measured between -5 and -25 dB, scaled to 60.
        (b - a) / SR * 3.0
    }

    #[test]
    fn decay_matches_the_decay_parameter() {
        for decay in [1.0f32, 2.5, 6.0] {
            let mut rv = Reverb::new();
            rv.set_param(ID_DECAY, decay);
            rv.set_param(ID_DAMPING, 0.0);
            rv.set_param(ID_MIX, 1.0);
            rv.set_param(ID_PREDELAY, 0.0);
            rv.prepare(SR, 512);
            let (l, _) = impulse_response(&mut rv, (SR * (decay * 1.6 + 1.0)) as usize);
            let measured = t60_from_edc(&edc_db(&l));
            assert!(
                (measured / decay - 1.0).abs() < 0.3,
                "decay {decay}: measured {measured}"
            );
        }
    }

    #[test]
    fn predelay_holds_back_the_wet_signal() {
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 1.0);
        rv.set_param(ID_PREDELAY, 50.0);
        rv.prepare(SR, 512);
        let (l, r) = impulse_response(&mut rv, 12_000);
        let first = l
            .iter()
            .zip(&r)
            .position(|(a, b)| a.abs() > 1e-6 || b.abs() > 1e-6)
            .unwrap();
        let pd = (0.05 * SR) as usize;
        assert!(first >= pd - 2, "first sound at {first}, pre-delay {pd}");
        assert!(first < pd + 600, "{first}");
    }

    #[test]
    fn width_zero_is_mono_and_default_is_decorrelated() {
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 1.0);
        rv.set_param(ID_WIDTH, 0.0);
        rv.prepare(SR, 512);
        let (l, r) = impulse_response(&mut rv, 20_000);
        for i in 0..l.len() {
            assert!((l[i] - r[i]).abs() < 1e-5);
        }
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 1.0);
        rv.prepare(SR, 512);
        let (l, r) = impulse_response(&mut rv, 20_000);
        let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..l.len() {
            ll += f64::from(l[i] * l[i]);
            rr += f64::from(r[i] * r[i]);
            lr += f64::from(l[i] * r[i]);
        }
        let corr = lr / (ll * rr).sqrt();
        assert!(corr.abs() < 0.3, "inter-channel correlation {corr}");
        assert!(ll > 0.0 && rr > 0.0);
    }

    #[test]
    fn mix_zero_passes_dry_exactly() {
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 0.0);
        rv.prepare(SR, 512);
        let ctx = ProcessContext::new(SR);
        let mut l: Vec<f32> = (0..512).map(|i| (i as f32 * 0.1).sin()).collect();
        let mut r = l.clone();
        let want = l.clone();
        rv.process(&ctx, &[], &mut l, &mut r);
        for i in 0..512 {
            assert!((l[i] - want[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn size_changes_are_click_free_and_stable() {
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 1.0);
        rv.set_param(ID_DECAY, 8.0);
        rv.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        let mut seed = 11u32;
        for block in 0..400 {
            let mut l: Vec<f32> = (0..256)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.3
                })
                .collect();
            let mut r = l.clone();
            if block % 50 == 0 {
                rv.set_param(ID_SIZE, if (block / 50) % 2 == 0 { 1.0 } else { 0.0 });
            }
            rv.process(&ctx, &[], &mut l, &mut r);
            for v in l.iter().chain(r.iter()) {
                assert!(v.is_finite() && v.abs() < 10.0, "{v}");
            }
        }
    }

    #[test]
    fn reports_tail_and_goes_silent() {
        let mut rv = Reverb::new();
        rv.set_param(ID_DECAY, 0.5);
        rv.set_param(ID_PREDELAY, 0.0);
        rv.prepare(SR, 512);
        let tail = rv.tail_seconds().unwrap();
        assert!((tail - 0.5).abs() < 0.01);
        let (l, _) = impulse_response(&mut rv, 4096);
        assert!(l.iter().any(|v| v.abs() > 1e-4));
        assert!(!rv.is_silent());
        let ctx = ProcessContext::new(SR);
        for _ in 0..((SR * 1.3) as usize / 512 + 2) {
            let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
            rv.process(&ctx, &[], &mut l, &mut r);
        }
        assert!(rv.is_silent());
    }

    /// Normalized echo density (Abel and Huang): share of samples in a
    /// Hann-weighted window that exceed the window's RMS, divided by the
    /// 31.7% a Gaussian noise tail would give. 1.0 means fully diffuse.
    fn echo_density(x: &[f32], start: usize, win: usize) -> f32 {
        let w: Vec<f32> = (0..win)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * (i as f32 + 0.5) / win as f32).cos())
            .collect();
        let wsum: f32 = w.iter().sum();
        let seg = &x[start..start + win];
        let rms = (seg.iter().zip(&w).map(|(v, w)| v * v * w).sum::<f32>() / wsum).sqrt();
        let above: f32 = seg
            .iter()
            .zip(&w)
            .filter(|(v, _)| v.abs() > rms)
            .map(|(_, w)| w)
            .sum();
        above / wsum / 0.3173
    }

    #[test]
    fn echo_density_builds_within_a_few_tens_of_milliseconds() {
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 1.0);
        rv.set_param(ID_PREDELAY, 0.0);
        rv.prepare(SR, 512);
        let (l, _) = impulse_response(&mut rv, 48_000);
        let mut sum = 0.0;
        for k in 0..8 {
            sum += echo_density(&l, 4_800 + k * 1_024, 1_024);
        }
        let mean = sum / 8.0;
        assert!(
            (0.85..1.2).contains(&mean),
            "mean echo density {mean} between 100 and 270 ms"
        );
    }

    #[test]
    fn tail_spectrum_is_smooth_not_ringing() {
        // A diffuse tail has an exponentially distributed power spectrum
        // (standard deviation 5.6 dB around its local mean). Ringing modes
        // push the spread and the worst peaks well past that.
        for decay in [1.0f32, 4.0] {
            let mut rv = Reverb::new();
            rv.set_param(ID_MIX, 1.0);
            rv.set_param(ID_DECAY, decay);
            rv.set_param(ID_DAMPING, 0.3);
            rv.prepare(SR, 512);
            let (l, _) = impulse_response(&mut rv, 24_576);
            let n = 8_192;
            let seg = &l[4_800..4_800 + n];
            // Naive DFT over 200 Hz .. 5 kHz at the FFT bin spacing.
            let k0 = (200.0 / SR * n as f32) as usize;
            let k1 = (5_000.0 / SR * n as f32) as usize;
            let power: Vec<f32> = (k0..k1)
                .map(|k| {
                    let (mut re, mut im) = (0.0f64, 0.0f64);
                    for (i, v) in seg.iter().enumerate() {
                        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
                        let ph = std::f64::consts::TAU * (k * i % n) as f64 / n as f64;
                        re += f64::from(*v) * w * ph.cos();
                        im -= f64::from(*v) * w * ph.sin();
                    }
                    (re * re + im * im) as f32
                })
                .collect();
            let db: Vec<f32> = (16..power.len() - 16)
                .map(|i| {
                    let local = power[i - 16..=i + 16].iter().sum::<f32>() / 33.0;
                    10.0 * (power[i] / local).max(1e-12).log10()
                })
                .collect();
            let mean = db.iter().sum::<f32>() / db.len() as f32;
            let std = (db.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / db.len() as f32).sqrt();
            let worst = db.iter().fold(f32::MIN, |m, v| m.max(*v));
            assert!(std < 7.0, "decay {decay}: spectral spread {std} dB");
            assert!(
                worst < 14.0,
                "decay {decay}: peak {worst} dB over local mean"
            );
        }
    }

    #[test]
    fn damping_shortens_high_frequency_decay_only() {
        let mut rv = Reverb::new();
        rv.set_param(ID_MIX, 1.0);
        rv.set_param(ID_DECAY, 3.0);
        rv.set_param(ID_DAMPING, 0.7);
        rv.set_param(ID_PREDELAY, 0.0);
        rv.prepare(SR, 512);
        let (l, _) = impulse_response(&mut rv, (SR * 6.0) as usize);
        // Split with one-pole filters: below 400 Hz and above 6 kHz.
        let mut lo = OnePoleLowpass::new(SR, 400.0);
        let mut hi = OnePoleHighpass::new(SR, 6_000.0);
        let low: Vec<f32> = l.iter().map(|v| lo.process(*v)).collect();
        let high: Vec<f32> = l.iter().map(|v| hi.process(*v)).collect();
        let (tl, th) = (t60_from_edc(&edc_db(&low)), t60_from_edc(&edc_db(&high)));
        assert!(tl > 2.4 && tl < 3.8, "low band t60 {tl}");
        assert!(th < tl * 0.4, "high band t60 {th} vs low {tl}");
    }
}
