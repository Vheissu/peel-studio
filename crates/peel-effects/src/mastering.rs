//! A stereo mastering chain built from Peel's native EQ, compressor and limiter.
//!
//! Input trim feeds broad tone controls, linked Glue compression, mid/side
//! width and the lookahead limiter. Simple and advanced editors use these
//! same parameters; changing editor mode never rewrites the sound. The
//! limiter reports its fixed latency for the engine's delay compensation.

use peel_dsp::util::{Smoothed, db_to_gain};
use peel_dsp::{
    Device, DeviceDescriptor, DeviceKind, ParamDescriptor, ParamId, Preset, ProcessContext, Unit,
};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};
use crate::{compressor, eq, limiter};

// Stable identities: append new parameters without renumbering these.
pub const ID_INPUT: ParamId = 0;
pub const ID_LOW: ParamId = 1;
pub const ID_MID: ParamId = 2;
pub const ID_HIGH: ParamId = 3;
pub const ID_THRESHOLD: ParamId = 4;
pub const ID_RATIO: ParamId = 5;
pub const ID_ATTACK: ParamId = 6;
pub const ID_RELEASE: ParamId = 7;
pub const ID_MAKEUP: ParamId = 8;
pub const ID_WIDTH: ParamId = 9;
pub const ID_CEILING: ParamId = 10;
pub const ID_LIMITER_RELEASE: ParamId = 11;
pub const ID_TRUE_PEAK: ParamId = 12;

static PARAMS: [ParamDescriptor; 13] = [
    ParamDescriptor::linear(
        ID_INPUT,
        "input",
        "Input",
        "Input",
        -12.0,
        18.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(ID_LOW, "low", "Low", "Tone", -6.0, 6.0, 0.0, Unit::Decibels),
    ParamDescriptor::linear(
        ID_MID,
        "mid",
        "Body",
        "Tone",
        -6.0,
        6.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_HIGH,
        "high",
        "Air",
        "Tone",
        -6.0,
        6.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_THRESHOLD,
        "threshold",
        "Threshold",
        "Compression",
        -36.0,
        0.0,
        -18.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_RATIO,
        "ratio",
        "Ratio",
        "Compression",
        1.0,
        6.0,
        1.5,
        Unit::Ratio,
    ),
    ParamDescriptor::log(
        ID_ATTACK,
        "attack",
        "Attack",
        "Compression",
        0.1,
        100.0,
        30.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::log(
        ID_RELEASE,
        "release",
        "Release",
        "Compression",
        10.0,
        1000.0,
        250.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::linear(
        ID_MAKEUP,
        "makeup",
        "Makeup",
        "Compression",
        0.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_WIDTH,
        "width",
        "Width",
        "Stereo",
        0.0,
        2.0,
        1.0,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_CEILING,
        "ceiling",
        "Ceiling",
        "Limiter",
        -12.0,
        0.0,
        -1.0,
        Unit::Decibels,
    ),
    ParamDescriptor::log(
        ID_LIMITER_RELEASE,
        "limiter_release",
        "Release",
        "Limiter",
        10.0,
        1000.0,
        200.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::toggle(ID_TRUE_PEAK, "true_peak", "True Peak", "Limiter", true),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.mastering",
    name: "Mastering",
    description: "Broad tone EQ, linked bus compression, stereo width and true-peak limiting",
    category: "Mastering",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Clean",
        params: &[],
    },
    Preset {
        name: "Balanced",
        params: &[
            ("input", 1.5),
            ("low", 0.5),
            ("mid", -0.75),
            ("high", 0.5),
            ("makeup", 1.0),
        ],
    },
    Preset {
        name: "Warm",
        params: &[
            ("low", 1.5),
            ("mid", -1.0),
            ("high", -0.5),
            ("threshold", -20.0),
            ("ratio", 1.6),
            ("makeup", 1.0),
        ],
    },
    Preset {
        name: "Punch",
        params: &[
            ("input", 1.5),
            ("low", 0.75),
            ("threshold", -14.0),
            ("ratio", 2.0),
            ("attack", 40.0),
            ("release", 150.0),
            ("makeup", 1.0),
            ("limiter_release", 150.0),
        ],
    },
    Preset {
        name: "Loud",
        params: &[
            ("input", 4.0),
            ("mid", -0.5),
            ("high", 0.75),
            ("threshold", -20.0),
            ("ratio", 2.2),
            ("attack", 20.0),
            ("release", 180.0),
            ("makeup", 2.0),
            ("width", 1.05),
            ("limiter_release", 120.0),
        ],
    },
];

/// Stereo mastering processor. All buffers are allocated in `prepare`.
pub struct Mastering {
    store: ParamStore,
    sample_rate: f32,
    input: Smoothed,
    width: Smoothed,
    tone: eq::Eq,
    compression: compressor::Compressor,
    limiter: limiter::Limiter,
    silence: SilenceTracker,
}

impl Default for Mastering {
    fn default() -> Self {
        Self::new()
    }
}

impl Mastering {
    pub fn new() -> Self {
        let mut effect = Self {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            input: Smoothed::new(1.0),
            width: Smoothed::new(1.0),
            tone: eq::Eq::new(),
            compression: compressor::Compressor::new(),
            limiter: limiter::Limiter::new(),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut effect, 48_000.0, 512);
        effect
    }

    /// Sum of compressor and limiter gain reduction, in positive dB.
    pub fn gain_reduction_db(&self) -> f32 {
        self.compression.gain_reduction_db() + self.limiter.gain_reduction_db()
    }
}

impl Fx for Mastering {
    fn store(&self) -> &ParamStore {
        &self.store
    }
    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, max_block: usize) {
        self.sample_rate = if sample_rate.is_finite() {
            sample_rate.max(8_000.0)
        } else {
            48_000.0
        };
        self.input.set_ramp(self.sample_rate, RAMP_SECONDS);
        self.width.set_ramp(self.sample_rate, RAMP_SECONDS);
        self.tone.set_param(eq::band_id(1, 1), 120.0);
        self.tone.set_param(eq::band_id(2, 1), 500.0);
        self.tone.set_param(eq::band_id(2, 3), 0.7);
        self.tone.set_param(eq::band_id(3, 0), 0.0);
        self.tone.set_param(eq::band_id(4, 1), 8_000.0);
        self.compression.set_param(compressor::ID_DETECTOR, 2.0);
        self.compression.set_param(compressor::ID_KNEE, 12.0);
        self.compression.set_param(compressor::ID_SC_HPF, 80.0);
        self.compression.set_param(compressor::ID_LINK, 1.0);
        self.compression.set_param(compressor::ID_AUTO_MAKEUP, 0.0);
        refresh_all(self);
        self.input.set_immediate(self.input.target());
        self.width.set_immediate(self.width.target());
        self.tone.prepare(self.sample_rate, max_block);
        self.compression.prepare(self.sample_rate, max_block);
        self.limiter.prepare(self.sample_rate, max_block);
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_INPUT => self.input.set_target(db_to_gain(value)),
            ID_LOW => self.tone.set_param(eq::band_id(1, 2), value),
            ID_MID => self.tone.set_param(eq::band_id(2, 2), value),
            ID_HIGH => self.tone.set_param(eq::band_id(4, 2), value),
            ID_THRESHOLD => self.compression.set_param(compressor::ID_THRESHOLD, value),
            ID_RATIO => self.compression.set_param(compressor::ID_RATIO, value),
            ID_ATTACK => self.compression.set_param(compressor::ID_ATTACK, value),
            ID_RELEASE => self.compression.set_param(compressor::ID_RELEASE, value),
            ID_MAKEUP => self.compression.set_param(compressor::ID_MAKEUP, value),
            ID_WIDTH => self.width.set_target(value),
            ID_CEILING => self.limiter.set_param(limiter::ID_CEILING, value),
            ID_LIMITER_RELEASE => self.limiter.set_param(limiter::ID_RELEASE, value),
            ID_TRUE_PEAK => self.limiter.set_param(limiter::ID_TRUE_PEAK, value),
            _ => {}
        }
    }

    fn clear(&mut self) {
        self.tone.reset();
        self.compression.reset();
        self.limiter.reset();
        self.silence.reset();
    }

    fn render(&mut self, ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let gain = self.input.next();
            *l *= gain;
            *r *= gain;
        }
        Fx::render(&mut self.tone, ctx, left, right);
        Fx::render(&mut self.compression, ctx, left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mid = (*l + *r) * 0.5;
            let side = (*l - *r) * 0.5 * self.width.next();
            *l = mid + side;
            *r = mid - side;
        }
        Fx::render(&mut self.limiter, ctx, left, right);
    }

    fn reduction(&self) -> Option<f32> {
        Some(self.gain_reduction_db())
    }
    fn latency_samples(&self) -> u32 {
        self.limiter.latency()
    }
    fn tail(&self) -> Option<f32> {
        Some(0.5 + self.limiter.latency() as f32 / self.sample_rate)
    }
    fn silent(&self) -> bool {
        self.silence
            .quiet_for((self.sample_rate * 0.5) as u32 + self.limiter.latency() + 8)
    }
}

impl_device!(Mastering, DESCRIPTOR);
