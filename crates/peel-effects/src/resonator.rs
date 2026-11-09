//! Four tuned feedback strings, excited by incoming audio.
use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};
use peel_dsp::biquad::{DcBlocker, OnePoleLowpass};
use peel_dsp::delay::{AllpassTap, DelayLine};
use peel_dsp::param::{ParamDescriptor, Unit};
use peel_dsp::util::{Smoothed, db_to_gain, key_to_hz, pan_gains};
use peel_dsp::{DeviceDescriptor, DeviceKind, ParamId, Preset, ProcessContext};
pub const ID_ROOT: ParamId = 1;
pub const ID_OCTAVE: ParamId = 2;
pub const ID_CHORD: ParamId = 3;
pub const ID_DECAY: ParamId = 4;
pub const ID_DAMPING: ParamId = 5;
pub const ID_SPREAD: ParamId = 6;
pub const ID_OUTPUT: ParamId = 7;
pub const ID_MIX: ParamId = 8;
const ROOTS: &[&str] = &[
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const CHORDS: &[&str] = &[
    "Unison", "Octaves", "Fifths", "Major", "Minor", "Sus4", "Major 7", "Minor 7",
];
const INTERVALS: [[i8; 4]; 8] = [
    [0, 0, 0, 12],
    [0, 12, 24, -12],
    [0, 7, 12, 19],
    [0, 4, 7, 12],
    [0, 3, 7, 12],
    [0, 5, 7, 12],
    [0, 4, 7, 11],
    [0, 3, 7, 10],
];
static PARAMS: [ParamDescriptor; 8] = [
    ParamDescriptor::choice(1, "root", "Root", "Tuning", ROOTS, 0),
    ParamDescriptor::choice(
        2,
        "octave",
        "Octave",
        "Tuning",
        &["1", "2", "3", "4", "5"],
        2,
    ),
    ParamDescriptor::choice(3, "chord", "Chord", "Tuning", CHORDS, 2),
    ParamDescriptor::log(
        4,
        "decay",
        "Decay",
        "Resonance",
        0.05,
        10.0,
        1.5,
        Unit::Seconds,
    ),
    ParamDescriptor::linear(
        5,
        "damping",
        "Damping",
        "Resonance",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        6,
        "spread",
        "Spread",
        "Resonance",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        7,
        "output",
        "Output",
        "Output",
        -24.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(8, "mix", "Mix", "Output", 0.0, 1.0, 0.35, Unit::Percent),
];
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.resonator",
    name: "Resonator",
    description: "Four tuned string resonances that ring with the input",
    category: "Creative",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};
pub static PRESETS: [Preset; 7] = [
    Preset {
        name: "Fifths",
        params: &[],
    },
    Preset {
        name: "Drone Octaves",
        params: &[
            ("chord", 1.0),
            ("decay", 4.0),
            ("damping", 0.3),
            ("mix", 0.4),
        ],
    },
    Preset {
        name: "Major Glow",
        params: &[
            ("chord", 3.0),
            ("decay", 2.5),
            ("damping", 0.5),
            ("mix", 0.3),
        ],
    },
    Preset {
        name: "Minor Haze",
        params: &[
            ("chord", 4.0),
            ("octave", 1.0),
            ("decay", 3.0),
            ("damping", 0.55),
            ("spread", 0.8),
            ("mix", 0.35),
        ],
    },
    Preset {
        name: "Metal Pipe",
        params: &[
            ("chord", 0.0),
            ("octave", 3.0),
            ("decay", 6.0),
            ("damping", 0.05),
            ("mix", 0.5),
        ],
    },
    Preset {
        name: "Short Body",
        params: &[
            ("chord", 1.0),
            ("octave", 1.0),
            ("decay", 0.15),
            ("damping", 0.6),
            ("mix", 0.25),
        ],
    },
    Preset {
        name: "Sus Shimmer",
        params: &[
            ("chord", 5.0),
            ("octave", 3.0),
            ("decay", 3.0),
            ("damping", 0.2),
            ("spread", 1.0),
            ("mix", 0.3),
        ],
    },
];
/// MIDI pitches in descriptor order; octave choice 2 means C3, MIDI 48.
/// Missing or malformed values use their descriptor defaults.
pub fn notes(params: &[f32]) -> [u8; 4] {
    let value = |i: usize| {
        params
            .get(i)
            .copied()
            .filter(|v| v.is_finite())
            .map_or(PARAMS[i].default, |v| PARAMS[i].clamp(v))
    };
    let root = 24 + 12 * value(1).round() as i16 + value(0).round() as i16;
    INTERVALS[value(2).round() as usize].map(|n| (root + n as i16).clamp(0, 127) as u8)
}
struct StringLoop {
    line: DelayLine,
    tap: AllpassTap,
    lp: OnePoleLowpass,
    delay: Smoothed,
    feedback: f32,
    period: f32,
}
impl StringLoop {
    fn new(sr: f32) -> Self {
        let mut delay = Smoothed::new(sr / 130.81);
        delay.set_ramp(sr, 0.04);
        Self {
            // The lowest exposed chord voice is C0 (16.35 Hz); allow
            // the full MIDI floor so every note in the readout can sound.
            line: DelayLine::new((sr / 8.0).ceil() as usize + 8),
            tap: AllpassTap::new(),
            lp: OnePoleLowpass::new(sr, 8_000.0),
            delay,
            feedback: 0.9,
            period: sr / 130.81,
        }
    }
    fn clear(&mut self) {
        self.line.clear();
        self.tap.reset();
        self.lp.reset();
    }
}
pub struct Resonator {
    store: ParamStore,
    sr: f32,
    p: [Smoothed; 8],
    strings: [StringLoop; 4],
    dc: [DcBlocker; 2],
    control: u8,
    output: f32,
    silence: SilenceTracker,
}
impl Default for Resonator {
    fn default() -> Self {
        Self::new()
    }
}
impl Resonator {
    pub fn new() -> Self {
        let mut s = Self {
            store: ParamStore::new(&DESCRIPTOR),
            sr: 48_000.0,
            p: std::array::from_fn(|i| Smoothed::new(PARAMS[i].default)),
            strings: std::array::from_fn(|_| StringLoop::new(48_000.0)),
            dc: std::array::from_fn(|_| DcBlocker::new(48_000.0, 10.0)),
            control: 0,
            output: 1.0,
            silence: SilenceTracker::default(),
        };
        s.setup(48_000.0, 512);
        s
    }
    fn tuning(&mut self, immediate: bool) {
        let pitches = notes(self.store.values());
        let unison = self.store.get(ID_CHORD) < 0.5;
        for (i, s) in self.strings.iter_mut().enumerate() {
            let detune = if unison {
                [0.0, 0.04, -0.04, 0.0][i]
            } else {
                0.0
            };
            let freq = key_to_hz(pitches[i] as f32 + detune).clamp(8.0, 0.2 * self.sr);
            s.period = self.sr / freq;
            s.delay.set_target(s.period);
            if immediate {
                s.delay.set_immediate(s.delay.target());
            }
        }
    }
    fn coefficients(&mut self, p: &[f32; 8]) {
        self.output = db_to_gain(p[6]);
        for s in &mut self.strings {
            let freq = self.sr / s.period;
            s.feedback = (-6.907_755 / (freq * p[3])).exp().min(0.999_99);
            // A higher cutoff for high notes keeps damping comparable across octaves.
            let cutoff = (freq * (4.0 + 100.0 * (1.0 - p[4]).powi(2))).min(self.sr * 0.45);
            s.lp.set_cutoff(self.sr, cutoff);
            // Subtract the loop filter's phase delay so Damping changes tone,
            // rather than flattening every string's tuning.
            let w = std::f32::consts::TAU * freq / self.sr;
            let pole = (-std::f32::consts::TAU * cutoff / self.sr).exp();
            let phase_delay = (pole * w.sin()).atan2(1.0 - pole * w.cos()) / w;
            let delay = (s.period - phase_delay).max(2.0);
            if (delay - s.delay.target()).abs() > 1e-4 {
                s.delay.set_target(delay);
            }
        }
    }
}
impl Fx for Resonator {
    fn store(&self) -> &ParamStore {
        &self.store
    }
    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }
    fn setup(&mut self, sr: f32, _max: usize) {
        self.sr = sr.max(8_000.0);
        self.strings = std::array::from_fn(|_| StringLoop::new(self.sr));
        self.dc = std::array::from_fn(|_| DcBlocker::new(self.sr, 10.0));
        for p in &mut self.p {
            p.set_ramp(self.sr, RAMP_SECONDS);
        }
        refresh_all(self);
        for p in &mut self.p {
            p.set_immediate(p.target());
        }
        self.tuning(true);
        self.coefficients(&self.p.each_ref().map(Smoothed::value));
        for string in &mut self.strings {
            string.delay.set_immediate(string.delay.target());
        }
        self.clear();
    }
    fn param_changed(&mut self, id: ParamId, v: f32) {
        if let Some(p) = self.p.get_mut(id.wrapping_sub(1) as usize) {
            p.set_target(v);
        }
        if matches!(id, ID_ROOT | ID_OCTAVE | ID_CHORD) {
            self.tuning(false);
        }
    }
    fn clear(&mut self) {
        for s in &mut self.strings {
            s.clear();
        }
        for dc in &mut self.dc {
            dc.reset();
        }
        self.control = 0;
        self.silence.reset();
    }
    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let p = self.p.each_mut().map(Smoothed::next);
            if self.control == 0 {
                self.coefficients(&p);
            }
            self.control = (self.control + 1) % 8;
            let dry = [*l, *r];
            let mono = 0.5 * (self.dc[0].process(*l) + self.dc[1].process(*r));
            let mut wet = [0.0; 2];
            for (i, s) in self.strings.iter_mut().enumerate() {
                let delayed = s.tap.read(&s.line, s.delay.next());
                let y = s.lp.process(delayed);
                s.line
                    .push((mono * (1.0 - s.feedback) + y * s.feedback).tanh());
                let pan = if i % 2 == 0 { -p[5] } else { p[5] };
                let (a, b) = pan_gains(pan);
                wet[0] += y * a * 0.5;
                wet[1] += y * b * 0.5;
            }
            *l = (dry[0] + (wet[0] - dry[0]) * p[7]) * self.output;
            *r = (dry[1] + (wet[1] - dry[1]) * p[7]) * self.output;
        }
    }
    fn tail(&self) -> Option<f32> {
        Some(self.store.get(ID_DECAY) * 1.5 + 0.5)
    }
    fn silent(&self) -> bool {
        self.silence
            .quiet_for((self.tail().unwrap_or(16.0) * self.sr) as u32)
    }
}
impl_device!(Resonator, DESCRIPTOR);
