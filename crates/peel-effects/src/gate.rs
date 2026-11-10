//! `peel.fx.gate`: a noise gate with hold, hysteresis and a sidechain.
//!
//! The gate opens when the detector rises above Threshold and closes once
//! it has stayed below Threshold minus Hysteresis for the Hold time. Closed,
//! the signal is turned down by Range, so a gate can duck as well as mute.
//! The detector listens to the track itself or, when a sidechain is routed,
//! to that key: a reverb return gated by its dry snare is the classic gated
//! reverb. Both channels share one gain, so the image never shifts.
//!
//! The detector is a peak follower that falls over a few milliseconds, so a
//! steady tone reads as its peak level rather than dipping at every zero
//! crossing.

use peel_dsp::biquad::{Coeffs, FilterKind, StereoBiquad};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::db_to_gain;

use crate::common::{Fx, ParamStore, SilenceTracker, impl_device, refresh_all};

pub const ID_THRESHOLD: ParamId = 1;
pub const ID_RANGE: ParamId = 2;
pub const ID_HYSTERESIS: ParamId = 3;
pub const ID_ATTACK: ParamId = 4;
pub const ID_HOLD: ParamId = 5;
pub const ID_RELEASE: ParamId = 6;
pub const ID_SC_HPF: ParamId = 7;

static PARAMS: [ParamDescriptor; 7] = [
    ParamDescriptor::linear(
        ID_THRESHOLD,
        "threshold",
        "Threshold",
        "Gate",
        -80.0,
        0.0,
        -30.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_RANGE,
        "range",
        "Range",
        "Gate",
        -80.0,
        0.0,
        -80.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_HYSTERESIS,
        "hysteresis",
        "Hysteresis",
        "Gate",
        0.0,
        12.0,
        3.0,
        Unit::Decibels,
    ),
    ParamDescriptor::log(
        ID_ATTACK,
        "attack",
        "Attack",
        "Timing",
        0.05,
        50.0,
        0.5,
        Unit::Milliseconds,
    ),
    ParamDescriptor::power(
        ID_HOLD,
        "hold",
        "Hold",
        "Timing",
        0.0,
        1_000.0,
        40.0,
        Unit::Milliseconds,
        2.0,
    ),
    ParamDescriptor::log(
        ID_RELEASE,
        "release",
        "Release",
        "Timing",
        5.0,
        2_000.0,
        120.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::log(
        ID_SC_HPF,
        "sidechain.hpf",
        "Sidechain HP",
        "Detector",
        20.0,
        2_000.0,
        20.0,
        Unit::Hertz,
    ),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.gate",
    name: "Gate",
    description: "Noise gate with hold, range and a sidechain key",
    category: "Dynamics",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Default",
        params: &[],
    },
    Preset {
        name: "Tight Drums",
        params: &[
            ("threshold", -24.0),
            ("attack", 0.1),
            ("hold", 25.0),
            ("release", 60.0),
        ],
    },
    Preset {
        name: "Gated Reverb",
        params: &[
            ("threshold", -30.0),
            ("hysteresis", 6.0),
            ("attack", 0.2),
            ("hold", 180.0),
            ("release", 35.0),
        ],
    },
    Preset {
        name: "Noise Floor",
        params: &[
            ("threshold", -55.0),
            ("range", -20.0),
            ("attack", 2.0),
            ("hold", 80.0),
            ("release", 300.0),
        ],
    },
    Preset {
        name: "Chop",
        params: &[
            ("threshold", -20.0),
            ("hysteresis", 1.0),
            ("attack", 0.1),
            ("hold", 0.0),
            ("release", 20.0),
        ],
    },
];

/// Time constant of the detector's fall, in seconds: long enough to span a
/// cycle of a 40 Hz tone without letting go.
const DETECTOR_FALL: f32 = 0.02;

/// Gate effect. See the module docs.
pub struct Gate {
    store: ParamStore,
    sample_rate: f32,
    open_level: f32,
    close_level: f32,
    floor: f32,
    attack: f32,
    release: f32,
    hold_samples: u32,
    hold_left: u32,
    open: bool,
    gain: f32,
    /// Detector level: instant rise, short fall.
    level: f32,
    level_fall: f32,
    sidechain: StereoBiquad,
    silence: SilenceTracker,
}

impl Default for Gate {
    fn default() -> Self {
        Gate::new()
    }
}

impl Gate {
    pub fn new() -> Gate {
        let mut g = Gate {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            open_level: 0.03,
            close_level: 0.02,
            floor: 0.0,
            attack: 1.0,
            release: 1.0,
            hold_samples: 0,
            hold_left: 0,
            open: false,
            gain: 0.0,
            level: 0.0,
            level_fall: 0.999,
            sidechain: StereoBiquad::default(),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut g, 48_000.0, 512);
        g
    }

    /// Gain applied right now, in dB (zero when fully open).
    pub fn gain_db(&self) -> f32 {
        20.0 * self.gain.max(1.0e-4).log10()
    }

    fn update(&mut self) {
        let v = |id| self.store.get(id);
        let threshold = v(ID_THRESHOLD);
        self.open_level = db_to_gain(threshold);
        self.close_level = db_to_gain(threshold - v(ID_HYSTERESIS));
        let range = v(ID_RANGE);
        // The bottom of the range is a full mute.
        self.floor = if range <= -79.5 {
            0.0
        } else {
            db_to_gain(range)
        };
        let sr = self.sample_rate;
        self.attack = 1.0 - (-1.0 / (v(ID_ATTACK) * 0.001 * sr)).exp();
        // The release time is the time to fall 60 dB.
        self.release = 1.0 - (-6.908 / (v(ID_RELEASE) * 0.001 * sr)).exp();
        self.hold_samples = (v(ID_HOLD) * 0.001 * sr) as u32;
        self.level_fall = (-1.0 / (DETECTOR_FALL * sr)).exp();
    }

    fn gate(&mut self, left: &mut [f32], right: &mut [f32], key: Option<(&[f32], &[f32])>) {
        self.silence.feed(left, right);
        for i in 0..left.len().min(right.len()) {
            let (kl, kr) = match key {
                Some((kl, kr)) => (kl[i], kr[i]),
                None => (left[i], right[i]),
            };
            let (kl, kr) = self.sidechain.process(kl, kr);
            self.level = kl.abs().max(kr.abs()).max(self.level * self.level_fall);
            let level = self.level;
            if level >= self.open_level {
                self.open = true;
                self.hold_left = self.hold_samples;
            } else if self.open && level < self.close_level {
                if self.hold_left == 0 {
                    self.open = false;
                } else {
                    self.hold_left -= 1;
                }
            }
            let (target, k) = if self.open {
                (1.0, self.attack)
            } else {
                (self.floor, self.release)
            };
            self.gain += (target - self.gain) * k;
            if (self.gain - target).abs() < 1.0e-6 {
                self.gain = target;
            }
            left[i] *= self.gain;
            right[i] *= self.gain;
        }
    }
}

impl Fx for Gate {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        self.sidechain.set_ramp_time(self.sample_rate, 0.01);
        refresh_all(self);
        let t = self.sidechain.target();
        self.sidechain.set_immediate(t);
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        if id == ID_SC_HPF {
            let c = if value <= 20.5 {
                Coeffs::IDENTITY
            } else {
                Coeffs::design(
                    FilterKind::HighPass,
                    self.sample_rate,
                    value,
                    std::f32::consts::FRAC_1_SQRT_2,
                    0.0,
                )
            };
            self.sidechain.set_target(c);
        }
        self.update();
    }

    fn clear(&mut self) {
        self.open = false;
        self.hold_left = 0;
        self.gain = self.floor;
        self.level = 0.0;
        self.sidechain.reset();
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.gate(left, right, None);
    }

    fn render_keyed(
        &mut self,
        _ctx: &ProcessContext,
        left: &mut [f32],
        right: &mut [f32],
        key: (&[f32], &[f32]),
    ) {
        self.gate(left, right, Some(key));
    }

    fn reduction(&self) -> Option<f32> {
        Some(-self.gain_db())
    }

    fn silent(&self) -> bool {
        self.silence.quiet_for((self.sample_rate * 0.1) as u32)
    }
}

impl_device!(Gate, DESCRIPTOR);
