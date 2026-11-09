//! Tempo-synced glitch, stutter and gate.
//!
//! Two sections that both follow the song position and tempo:
//!
//! * **Time** records the input into a six second history and drives a
//!   playhead from a pattern. The playhead can repeat the slice just before
//!   a grid line (stutter, 1/4 down to 1/64), play half speed, play a beat
//!   backwards, run a tape stop (speed ramps to zero) or scatter
//!   pseudo-random repeats and reversals. Every jump of the playhead is
//!   eased in over 3 ms, so slice boundaries do not click.
//! * **Volume** is a 16-step gate across one cycle (half a bar, one bar or
//!   two bars, in 4/4). Steps hold levels, `gate.depth` scales how far a
//!   closed step ducks, and `gate.smooth` sets the attack and release time.
//!
//! The time section feeds the gate; `mix` blends the result with the dry
//! input. With both patterns Off and `mix` at 1 the audio passes through
//! bit for bit. While the transport is stopped the pattern clock free-runs
//! at the current tempo, so the effect can be monitored live.
//!
//! Slices are cut on the beat grid. A repeat takes the audio from the slice
//! length *before* the slot started, so nothing needs look-ahead and the
//! effect adds no latency.

use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, sanitize};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

pub const ID_TIME_PATTERN: ParamId = 0;
pub const ID_TIME_MIX: ParamId = 1;
pub const ID_GATE_PATTERN: ParamId = 2;
pub const ID_GATE_SMOOTH: ParamId = 3;
pub const ID_GATE_DEPTH: ParamId = 4;
pub const ID_CYCLE: ParamId = 5;
pub const ID_MIX: ParamId = 6;

pub const TIME_OFF: u32 = 0;
pub const TIME_STUTTER_4: u32 = 1;
pub const TIME_STUTTER_8: u32 = 2;
pub const TIME_STUTTER_16: u32 = 3;
pub const TIME_STUTTER_32: u32 = 4;
pub const TIME_STUTTER_64: u32 = 5;
pub const TIME_BUILD: u32 = 6;
pub const TIME_HALF_SPEED: u32 = 7;
pub const TIME_REVERSE: u32 = 8;
pub const TIME_TAPE_STOP: u32 = 9;
pub const TIME_SCATTER: u32 = 10;

const TIME_PATTERNS: &[&str] = &[
    "Off",
    "Stutter 1/4",
    "Stutter 1/8",
    "Stutter 1/16",
    "Stutter 1/32",
    "Stutter 1/64",
    "Stutter Build",
    "Half Speed",
    "Reverse Beat",
    "Tape Stop",
    "Scatter",
];

pub const GATE_OFF: u32 = 0;
pub const GATE_TRANCE: u32 = 1;
pub const GATE_OFFBEAT: u32 = 2;
pub const GATE_HALF: u32 = 3;
pub const GATE_CHOP: u32 = 4;
pub const GATE_DUCK: u32 = 5;
pub const GATE_RAMP_UP: u32 = 6;
pub const GATE_RAMP_DOWN: u32 = 7;

const GATE_PATTERNS: &[&str] = &[
    "Off",
    "Trance 1/16",
    "Offbeat Pump",
    "Half Gate",
    "Chop 3-3-2",
    "Sidechain Duck",
    "Ramp Up",
    "Ramp Down",
];

const CYCLES: &[&str] = &["1/2 Bar", "1 Bar", "2 Bars"];
/// Cycle lengths in quarter notes (4/4).
const CYCLE_BEATS: [f64; 3] = [2.0, 4.0, 8.0];

/// Step levels (0 closed, 1 open) of each gate pattern, 16 steps per cycle.
/// Index with `pattern - 1`.
pub const GATE_STEPS: [[f32; 16]; 7] = [
    // Trance 1/16
    [
        1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0,
    ],
    // Offbeat Pump: closed on the first half of every beat
    [
        0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0,
    ],
    // Half Gate
    [
        1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ],
    // Chop 3-3-2, twice
    [
        1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0,
    ],
    // Sidechain Duck: drop on every beat, then recover
    [
        0.0, 0.2, 0.55, 0.85, 0.0, 0.2, 0.55, 0.85, 0.0, 0.2, 0.55, 0.85, 0.0, 0.2, 0.55, 0.85,
    ],
    // Ramp Up
    [
        0.0, 0.07, 0.13, 0.2, 0.27, 0.33, 0.4, 0.47, 0.53, 0.6, 0.67, 0.73, 0.8, 0.87, 0.93, 1.0,
    ],
    // Ramp Down
    [
        1.0, 0.93, 0.87, 0.8, 0.73, 0.67, 0.6, 0.53, 0.47, 0.4, 0.33, 0.27, 0.2, 0.13, 0.07, 0.0,
    ],
];

/// Seconds of input kept for the playhead.
const HISTORY_SECONDS: f32 = 6.0;
/// Crossfade at every playhead jump.
const XFADE_SECONDS: f32 = 0.003;
/// Guards slot boundaries against rounding in the beat position.
const EPS: f64 = 1e-9;
/// Longest gate attack/release time constant at `gate.smooth` = 1.
const MAX_GATE_TAU: f32 = 0.08;

static PARAMS: [ParamDescriptor; 7] = [
    ParamDescriptor::choice(
        ID_TIME_PATTERN,
        "time.pattern",
        "Pattern",
        "Time",
        TIME_PATTERNS,
        0,
    ),
    ParamDescriptor::linear(
        ID_TIME_MIX,
        "time.mix",
        "Amount",
        "Time",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::choice(
        ID_GATE_PATTERN,
        "gate.pattern",
        "Pattern",
        "Volume",
        GATE_PATTERNS,
        0,
    ),
    ParamDescriptor::linear(
        ID_GATE_SMOOTH,
        "gate.smooth",
        "Smooth",
        "Volume",
        0.0,
        1.0,
        0.2,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_GATE_DEPTH,
        "gate.depth",
        "Depth",
        "Volume",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::choice(ID_CYCLE, "cycle", "Cycle", "Pattern", CYCLES, 1),
    ParamDescriptor::linear(ID_MIX, "mix", "Mix", "Output", 0.0, 1.0, 1.0, Unit::Percent),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.glitch",
    name: "Glitch",
    description: "Tempo-synced stutter, tape stop, reverse and step gate",
    category: "Creative",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 7] = [
    Preset {
        name: "Bypass Ready",
        params: &[],
    },
    Preset {
        name: "Stutter Fill",
        params: &[("time.pattern", 6.0), ("gate.smooth", 0.1)],
    },
    Preset {
        name: "Tape Stop",
        params: &[("time.pattern", 9.0), ("cycle", 2.0)],
    },
    Preset {
        name: "Half Time",
        params: &[("time.pattern", 7.0), ("cycle", 1.0)],
    },
    Preset {
        name: "Trance Gate",
        params: &[
            ("gate.pattern", 1.0),
            ("gate.smooth", 0.15),
            ("gate.depth", 1.0),
        ],
    },
    Preset {
        name: "Pump",
        params: &[
            ("gate.pattern", 5.0),
            ("gate.smooth", 0.45),
            ("gate.depth", 0.8),
        ],
    },
    Preset {
        name: "Scatter",
        params: &[
            ("time.pattern", 10.0),
            ("gate.pattern", 4.0),
            ("gate.smooth", 0.3),
            ("gate.depth", 0.6),
            ("time.mix", 0.85),
        ],
    },
];

/// Stable pseudo-random value from a slot number (splitmix64 finalizer).
fn hash(n: i64) -> u64 {
    let mut z = (n as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// What the playhead does during one slot of the time pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Live,
    /// Loop the slice of this many beats that preceded the slot.
    Repeat(f64),
    /// Play the preceding slice backwards.
    Reverse(f64),
    Half,
    /// Slow to a stop over the slot.
    TapeStop,
}

/// One slot of the time pattern: an identity, an action and a length.
#[derive(Clone, Copy, Debug)]
struct Slot {
    id: i64,
    action: Action,
    beats: f64,
}

/// The time pattern's slot at beat position `b`.
fn slot_at(pattern: u32, b: f64, cycle: f64) -> Slot {
    let idx = |len: f64| (b / len + EPS).floor() as i64;
    match pattern {
        TIME_STUTTER_4..=TIME_STUTTER_64 => {
            let len = [1.0, 0.5, 0.25, 0.125, 0.0625][(pattern - TIME_STUTTER_4) as usize];
            Slot {
                id: idx(1.0),
                action: Action::Repeat(len),
                beats: 1.0,
            }
        }
        TIME_BUILD => {
            // Four stages per cycle, repeat length halving from 1/8 note.
            let beats = cycle / 4.0;
            let id = idx(beats);
            let k = id.rem_euclid(4);
            Slot {
                id,
                action: Action::Repeat(beats / f64::from(1u32 << (k + 1))),
                beats,
            }
        }
        TIME_HALF_SPEED => Slot {
            id: idx(2.0),
            action: Action::Half,
            beats: 2.0,
        },
        TIME_REVERSE => {
            let id = idx(1.0);
            Slot {
                id,
                action: if id.rem_euclid(2) == 1 {
                    Action::Reverse(1.0)
                } else {
                    Action::Live
                },
                beats: 1.0,
            }
        }
        TIME_TAPE_STOP => {
            let beats = cycle / 2.0;
            let id = idx(beats);
            Slot {
                id,
                action: if id.rem_euclid(2) == 1 {
                    Action::TapeStop
                } else {
                    Action::Live
                },
                beats,
            }
        }
        TIME_SCATTER => {
            let id = idx(0.5);
            let action = match hash(id) % 8 {
                0..=2 => Action::Live,
                3 => Action::Repeat(0.25),
                4 => Action::Repeat(0.125),
                5 => Action::Reverse(0.5),
                6 => Action::Half,
                _ => Action::Repeat(0.5),
            };
            Slot {
                id,
                action,
                beats: 0.5,
            }
        }
        _ => Slot {
            id: 0,
            action: Action::Live,
            beats: 1.0,
        },
    }
}

/// Gate step levels for `pattern` (1..=7), or all open.
pub fn gate_steps(pattern: u32) -> [f32; 16] {
    pattern
        .checked_sub(1)
        .and_then(|i| GATE_STEPS.get(i as usize))
        .copied()
        .unwrap_or([1.0; 16])
}

/// Stereo ring of recent input. `written` counts samples since the last
/// reset; sample `n` lives at `n % len`.
struct History {
    l: Vec<f32>,
    r: Vec<f32>,
    written: i64,
}

impl History {
    fn new(len: usize) -> History {
        History {
            l: vec![0.0; len],
            r: vec![0.0; len],
            written: 0,
        }
    }

    fn push(&mut self, l: f32, r: f32) {
        let i = self.written.rem_euclid(self.l.len() as i64) as usize;
        self.l[i] = l;
        self.r[i] = r;
        self.written += 1;
    }

    /// Cubic read at an absolute sample position, clamped to the valid
    /// range of the history.
    #[inline]
    fn read(&self, pos: f64) -> (f32, f32) {
        let cap = self.l.len() as i64;
        let p = pos.clamp((self.written - cap + 4) as f64, (self.written - 1) as f64);
        let i = p.floor();
        let t = (p - i) as f32;
        let i = i as i64;
        let at = |k: i64| (i + k).rem_euclid(cap) as usize;
        if t == 0.0 {
            let j = at(0);
            return (self.l[j], self.r[j]);
        }
        let (a, b, c, d) = (at(-1), at(0), at(1), at(2));
        (
            cubic(self.l[a], self.l[b], self.l[c], self.l[d], t),
            cubic(self.r[a], self.r[b], self.r[c], self.r[d], t),
        )
    }
}

#[inline]
fn cubic(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// A playhead: either the live input or a read position in the history.
#[derive(Clone, Copy, Debug)]
struct Voice {
    live: bool,
    pos: f64,
    /// Samples advanced per sample (negative plays backwards).
    speed: f64,
    /// Speed lost per sample (tape stop).
    decel: f64,
    lo: f64,
    hi: f64,
    looping: bool,
    /// Fade the output out as the speed approaches zero.
    fade_slow: bool,
}

impl Voice {
    const LIVE: Voice = Voice {
        live: true,
        pos: 0.0,
        speed: 1.0,
        decel: 0.0,
        lo: 0.0,
        hi: 0.0,
        looping: false,
        fade_slow: false,
    };

    fn sample(&self, hist: &History, in_l: f32, in_r: f32) -> (f32, f32) {
        if self.live {
            return (in_l, in_r);
        }
        let (l, r) = hist.read(self.pos);
        if self.fade_slow {
            let s = (self.speed / 0.3).clamp(0.0, 1.0) as f32;
            let g = s * s * (3.0 - 2.0 * s);
            (l * g, r * g)
        } else {
            (l, r)
        }
    }

    /// Moves one sample; true when a loop wrapped.
    fn advance(&mut self) -> bool {
        if self.live {
            return false;
        }
        self.pos += self.speed;
        if self.decel != 0.0 {
            self.speed = (self.speed - self.decel).max(0.0);
        }
        if self.looping {
            let n = self.hi - self.lo;
            if self.speed > 0.0 && self.pos >= self.hi {
                self.pos -= n;
                return true;
            }
            if self.speed < 0.0 && self.pos < self.lo {
                self.pos += n;
                return true;
            }
        }
        false
    }
}

/// Glitch effect. See the module docs.
pub struct Glitch {
    store: ParamStore,
    sample_rate: f32,
    hist: History,
    cur: Voice,
    /// Last time-section output, so a jump can start from it.
    last: (f32, f32),
    /// A jump just happened; measure the step on the next sample.
    pending: bool,
    /// Offset removed over the crossfade, and its remaining weight.
    resid: (f32, f32),
    resid_gain: f32,
    resid_step: f32,
    /// Pattern and slot of the voice now playing.
    last_key: (u32, i64),
    gate_gain: f32,
    time_mix: Smoothed,
    mix: Smoothed,
    /// Pattern clock in quarter notes, used while the transport is stopped.
    clock: f64,
    tempo: f64,
    silence: SilenceTracker,
}

impl Default for Glitch {
    fn default() -> Self {
        Glitch::new()
    }
}

impl Glitch {
    pub fn new() -> Glitch {
        let mut g = Glitch {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            hist: History::new(16),
            cur: Voice::LIVE,
            last: (0.0, 0.0),
            pending: false,
            resid: (0.0, 0.0),
            resid_gain: 0.0,
            resid_step: 0.01,
            last_key: (u32::MAX, 0),
            gate_gain: 1.0,
            time_mix: Smoothed::new(1.0),
            mix: Smoothed::new(1.0),
            clock: 0.0,
            tempo: 120.0,
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut g, 48_000.0, 512);
        g
    }

    /// Builds the playhead for a slot that starts now.
    fn voice_for(&self, slot: &Slot, spb: f64) -> Voice {
        let w = self.hist.written as f64;
        // Index of the sample the slot starts on; slices end just before it.
        let e = w - 1.0;
        let cap = self.hist.l.len() as f64;
        let min_len = (f64::from(self.sample_rate) * f64::from(XFADE_SECONDS) * 4.0).max(16.0);
        let slice = |beats: f64| (beats * spb).round().clamp(min_len, cap * 0.5);
        match slot.action {
            Action::Live => Voice::LIVE,
            Action::Repeat(beats) => {
                let n = slice(beats);
                Voice {
                    live: false,
                    pos: e - n,
                    speed: 1.0,
                    lo: e - n,
                    hi: e,
                    looping: true,
                    ..Voice::LIVE
                }
            }
            Action::Reverse(beats) => {
                let n = slice(beats);
                Voice {
                    live: false,
                    pos: e - 1.0,
                    speed: -1.0,
                    lo: e - n,
                    hi: e,
                    looping: true,
                    ..Voice::LIVE
                }
            }
            Action::Half => Voice {
                live: false,
                pos: w - 8.0,
                speed: 0.5,
                ..Voice::LIVE
            },
            Action::TapeStop => Voice {
                live: false,
                pos: w - 8.0,
                speed: 1.0,
                decel: 1.0 / (slot.beats * spb).max(1.0),
                fade_slow: true,
                ..Voice::LIVE
            },
        }
    }
}

impl Fx for Glitch {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        self.hist = History::new((HISTORY_SECONDS * sr) as usize + 64);
        self.resid_step = 1.0 / (XFADE_SECONDS * sr);
        self.time_mix.set_ramp(sr, RAMP_SECONDS);
        self.mix.set_ramp(sr, RAMP_SECONDS);
        refresh_all(self);
        for s in [&mut self.time_mix, &mut self.mix] {
            let t = s.target();
            s.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_TIME_MIX => self.time_mix.set_target(value),
            ID_MIX => self.mix.set_target(value),
            _ => {}
        }
    }

    fn clear(&mut self) {
        self.hist.l.fill(0.0);
        self.hist.r.fill(0.0);
        self.hist.written = 0;
        self.cur = Voice::LIVE;
        self.last = (0.0, 0.0);
        self.pending = false;
        self.resid_gain = 0.0;
        self.last_key = (u32::MAX, 0);
        self.gate_gain = 1.0;
        self.clock = 0.0;
        self.silence.reset();
    }

    fn render(&mut self, ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        let sr = f64::from(self.sample_rate);
        self.tempo = ctx.tempo_bpm;
        let bpf = ctx.tempo_bpm / (60.0 * sr);
        let spb = 60.0 * sr / ctx.tempo_bpm;
        let base = if ctx.playing {
            ctx.beat_position
        } else {
            self.clock
        };

        let time_pattern = self.store.get(ID_TIME_PATTERN) as u32;
        let gate_pattern = self.store.get(ID_GATE_PATTERN) as u32;
        let cycle = CYCLE_BEATS[(self.store.get(ID_CYCLE) as usize).min(2)];
        let depth = self.store.get(ID_GATE_DEPTH);
        let smooth = self.store.get(ID_GATE_SMOOTH);
        let tau = smooth * smooth * MAX_GATE_TAU;
        let sample_rate = self.sample_rate;
        let coeff = |t: f32| {
            if t <= 0.0 {
                1.0
            } else {
                1.0 - (-1.0 / (t * sample_rate)).exp()
            }
        };
        // Opening is the release of a duck; closing is its attack.
        let (open_a, close_a) = (coeff(tau), coeff(tau * 0.3));
        let steps = gate_steps(gate_pattern);

        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let b = base + i as f64 * bpf;
            let (in_l, in_r) = (*l, *r);
            self.hist.push(in_l, in_r);

            // ---- time ----
            let slot = slot_at(time_pattern, b, cycle);
            let key = (time_pattern, slot.id);
            if key != self.last_key {
                self.last_key = key;
                let next = self.voice_for(&slot, spb);
                if !(self.cur.live && next.live) {
                    self.cur = next;
                    self.pending = true;
                }
            }
            let (mut v_l, mut v_r) = self.cur.sample(&self.hist, in_l, in_r);
            // A jump starts from where the output was and eases onto the new
            // playhead over the crossfade, however the jumps overlap.
            if self.pending {
                self.resid = (self.last.0 - v_l, self.last.1 - v_r);
                self.resid_gain = 1.0;
                self.pending = false;
            }
            if self.resid_gain > 0.0 {
                v_l += self.resid.0 * self.resid_gain;
                v_r += self.resid.1 * self.resid_gain;
                self.resid_gain = (self.resid_gain - self.resid_step).max(0.0);
            }
            self.last = (v_l, v_r);
            if self.cur.advance() {
                self.pending = true;
            }

            let tm = self.time_mix.next();
            let t_l = in_l * (1.0 - tm) + v_l * tm;
            let t_r = in_r * (1.0 - tm) + v_r * tm;

            // ---- volume ----
            let target = if gate_pattern == GATE_OFF {
                1.0
            } else {
                let phase = (b / cycle).rem_euclid(1.0);
                let step = ((phase * 16.0 + EPS) as usize).min(15);
                1.0 - depth * (1.0 - steps[step])
            };
            let a = if target < self.gate_gain {
                close_a
            } else {
                open_a
            };
            self.gate_gain += (target - self.gate_gain) * a;
            if (target - self.gate_gain).abs() < 1e-6 {
                self.gate_gain = target;
            }

            let wet_l = t_l * self.gate_gain;
            let wet_r = t_r * self.gate_gain;
            let mix = self.mix.next();
            *l = sanitize(in_l * (1.0 - mix) + wet_l * mix);
            *r = sanitize(in_r * (1.0 - mix) + wet_r * mix);
        }
        self.clock = base + left.len() as f64 * bpf;
    }

    fn tail(&self) -> Option<f32> {
        if self.store.get(ID_TIME_PATTERN) as u32 == TIME_OFF {
            return Some(0.05);
        }
        // Material captured before the input stopped can play for up to a
        // slot (at most four beats) plus the slice lag.
        let beat = (60.0 / self.tempo.max(1.0)) as f32;
        Some((6.0 * beat).clamp(0.05, 30.0))
    }

    fn silent(&self) -> bool {
        let secs = self.tail().unwrap_or(5.0);
        self.silence.quiet_for((secs * self.sample_rate) as u32)
    }
}

impl_device!(Glitch, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;
    /// 120 bpm: one beat is 24 000 samples.
    const BEAT: usize = 24_000;

    fn make(params: &[(ParamId, f32)]) -> Glitch {
        let mut g = Glitch::new();
        for &(id, v) in params {
            g.set_param(id, v);
        }
        g.prepare(SR, 256);
        g
    }

    fn run(g: &mut Glitch, l: &mut [f32], r: &mut [f32], block: usize) {
        let mut c = ProcessContext::new(SR);
        c.playing = true;
        let mut at = 0;
        while at < l.len() {
            let end = (at + block).min(l.len());
            c.beat_position = at as f64 * 2.0 / f64::from(SR);
            g.process(&c, &[], &mut l[at..end], &mut r[at..end]);
            at = end;
        }
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut s = 12_345u32;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1u32 << 24) as f32 * 1.6 - 0.8
            })
            .collect()
    }

    fn max_step(x: &[f32]) -> f32 {
        x.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn off_passes_audio_unchanged() {
        let mut g = make(&[]);
        let src = noise(3 * BEAT);
        let (mut l, mut r) = (src.clone(), src.clone());
        run(&mut g, &mut l, &mut r, 256);
        assert_eq!(l, src);
        assert_eq!(r, src);
    }

    #[test]
    fn gate_follows_the_step_pattern() {
        let mut g = make(&[
            (ID_GATE_PATTERN, GATE_HALF as f32),
            (ID_GATE_SMOOTH, 0.0),
            (ID_GATE_DEPTH, 1.0),
        ]);
        // One bar is 96 000 samples; each step is 6 000.
        let n = 4 * BEAT;
        let mut l = vec![0.5; n];
        let mut r = vec![0.5; n];
        run(&mut g, &mut l, &mut r, 300);
        for step in 0..16 {
            let expect = if step < 8 { 0.5 } else { 0.0 };
            let mid = step * 6_000 + 3_000;
            assert!((l[mid] - expect).abs() < 1e-6, "step {step}: {}", l[mid]);
        }
    }

    #[test]
    fn gate_depth_scales_the_duck() {
        let mut g = make(&[
            (ID_GATE_PATTERN, GATE_HALF as f32),
            (ID_GATE_SMOOTH, 0.0),
            (ID_GATE_DEPTH, 0.5),
        ]);
        let mut l = vec![1.0; 4 * BEAT];
        let mut r = l.clone();
        run(&mut g, &mut l, &mut r, 512);
        assert!((l[2 * BEAT + 100] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn gate_smoothing_ramps_instead_of_stepping() {
        let mut g = make(&[(ID_GATE_PATTERN, GATE_HALF as f32), (ID_GATE_SMOOTH, 0.6)]);
        let mut l = vec![1.0; 4 * BEAT];
        let mut r = l.clone();
        run(&mut g, &mut l, &mut r, 512);
        let edge = 2 * BEAT;
        assert!(l[edge + 5] > 0.5, "closed instantly: {}", l[edge + 5]);
        assert!(l[edge + 3_000] < 0.3);
        assert!(max_step(&l) < 0.01, "step {}", max_step(&l));
    }

    #[test]
    fn stutter_repeats_the_captured_slice() {
        let mut g = make(&[(ID_TIME_PATTERN, TIME_STUTTER_8 as f32)]);
        let src = noise(3 * BEAT);
        let (mut l, mut r) = (src.clone(), src.clone());
        run(&mut g, &mut l, &mut r, 200);
        // Stutter 1/8 is half a beat; the slot starting at beat 1 repeats the
        // half beat before it.
        let len = BEAT / 2;
        let start = BEAT;
        let fade = 200;
        for rep in 0..2 {
            for j in fade..len - 1 {
                let got = l[start + rep * len + j];
                let want = src[start - len + j];
                assert!((got - want).abs() < 1e-5, "rep {rep} +{j}: {got} vs {want}");
            }
        }
    }

    #[test]
    fn reverse_plays_the_previous_beat_backwards() {
        let mut g = make(&[(ID_TIME_PATTERN, TIME_REVERSE as f32)]);
        let src = noise(3 * BEAT);
        let (mut l, mut r) = (src.clone(), src.clone());
        run(&mut g, &mut l, &mut r, 256);
        // Beat 1 is reversed beat 0; beat 2 is live again.
        for j in 300..BEAT - 300 {
            let want = src[BEAT - 1 - j];
            assert!((l[BEAT + j] - want).abs() < 1e-5, "+{j}");
        }
        for j in 300..BEAT {
            assert!((l[2 * BEAT + j] - src[2 * BEAT + j]).abs() < 1e-6);
        }
    }

    #[test]
    fn tape_stop_slows_to_silence() {
        let mut g = make(&[(ID_TIME_PATTERN, TIME_TAPE_STOP as f32), (ID_CYCLE, 1.0)]);
        let n = 8 * BEAT;
        let mut l: Vec<f32> = (0..n).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
        let mut r = l.clone();
        run(&mut g, &mut l, &mut r, 256);
        // Bar is 4 beats; the stop slot is beats 2 to 4.
        let rms = |a: usize, b: usize| {
            (l[a..b].iter().map(|v| v * v).sum::<f32>() / (b - a) as f32).sqrt()
        };
        assert!(rms(0, BEAT) > 0.3);
        assert!(rms(2 * BEAT, 2 * BEAT + 2_000) > 0.3);
        assert!(rms(4 * BEAT - 2_000, 4 * BEAT) < 0.05);
        // Back to live at the next bar.
        assert!(rms(5 * BEAT, 6 * BEAT) > 0.3);
    }

    #[test]
    fn jumps_stay_click_free() {
        let n = 8 * BEAT;
        let base: Vec<f32> = (0..n).map(|i| 0.6 * (i as f32 * 0.02).sin()).collect();
        let max_in = max_step(&base);
        for pattern in 1..TIME_PATTERNS.len() as u32 {
            let mut g = make(&[(ID_TIME_PATTERN, pattern as f32)]);
            let (mut l, mut r) = (base.clone(), base.clone());
            run(&mut g, &mut l, &mut r, 256);
            let max_out = max_step(&l);
            // A crossfade between two sines can add the difference of their
            // slopes but never a hard step.
            assert!(
                max_out < max_in * 3.0 + 0.01,
                "pattern {pattern}: jump {max_out} vs {max_in}"
            );
        }
    }

    #[test]
    fn output_is_independent_of_block_size() {
        let src = noise(4 * BEAT);
        let render = |block: usize| {
            let mut g = make(&[
                (ID_TIME_PATTERN, TIME_SCATTER as f32),
                (ID_GATE_PATTERN, GATE_TRANCE as f32),
            ]);
            let (mut l, mut r) = (src.clone(), src.clone());
            run(&mut g, &mut l, &mut r, block);
            l
        };
        let a = render(64);
        let b = render(1_000);
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5));
    }

    #[test]
    fn every_preset_is_finite() {
        for preset in PRESETS.iter() {
            let mut g = Glitch::new();
            for (key, v) in preset.params {
                let p = DESCRIPTOR.param_by_key(key).unwrap();
                g.set_param(p.id, *v);
            }
            g.prepare(SR, 256);
            let src = noise(6 * BEAT);
            let (mut l, mut r) = (src.clone(), src);
            run(&mut g, &mut l, &mut r, 256);
            assert!(
                l.iter()
                    .chain(r.iter())
                    .all(|v| v.is_finite() && v.abs() < 2.0)
            );
        }
    }

    #[test]
    fn scatter_is_deterministic() {
        let src = noise(4 * BEAT);
        let render = || {
            let mut g = make(&[(ID_TIME_PATTERN, TIME_SCATTER as f32)]);
            let (mut l, mut r) = (src.clone(), src.clone());
            run(&mut g, &mut l, &mut r, 256);
            l
        };
        assert_eq!(render(), render());
    }
}
