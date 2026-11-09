//! Phaser: a chain of up to twelve first-order all-pass stages.
//!
//! All stages share one corner frequency that an LFO sweeps around *Center*
//! (up to two octaves either way at full depth). Mixing the chain's output
//! with the dry signal cancels where the stages have turned the phase by
//! 180 degrees, giving one notch per pair of stages. Feedback from the chain
//! output to its input (positive or negative) sharpens the notches.
//!
//! The stage count is smoothed: every stage always runs and the output tap
//! slides between neighbouring stages, so changing it never clicks. The
//! right channel's LFO is offset by *Spread*; the LFO is free in Hz or
//! locked to the host tempo.

use peel_dsp::delay::{Lfo, fast_sin_turns};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain, sanitize};

use crate::common::{
    Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, lfo_follow_tempo, refresh_all,
    soft_limit,
};

pub const ID_STAGES: ParamId = 0;
pub const ID_SYNC: ParamId = 1;
pub const ID_RATE: ParamId = 2;
pub const ID_BEATS: ParamId = 3;
pub const ID_CENTER: ParamId = 4;
pub const ID_DEPTH: ParamId = 5;
pub const ID_FEEDBACK: ParamId = 6;
pub const ID_SPREAD: ParamId = 7;
pub const ID_MIX: ParamId = 8;
pub const ID_OUTPUT: ParamId = 9;

const MAX_STAGES: usize = 12;
/// Sweep either side of the centre at full depth, in octaves.
const SWEEP_OCTAVES: f32 = 2.0;

/// Stage counts 2 to 12; the choice index is `stages - 2`.
const STAGES: &[&str] = &["2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12"];

static PARAMS: [ParamDescriptor; 10] = [
    ParamDescriptor::toggle(ID_SYNC, "sync", "Sync", "Rate", false),
    ParamDescriptor::log(
        ID_RATE,
        "rate",
        "Rate",
        "Rate",
        0.02,
        10.0,
        0.4,
        Unit::Hertz,
    ),
    ParamDescriptor::log(
        ID_BEATS,
        "beats",
        "Note",
        "Rate",
        0.125,
        8.0,
        4.0,
        Unit::Beats,
    ),
    ParamDescriptor::choice(ID_STAGES, "stages", "Stages", "Phaser", STAGES, 2),
    ParamDescriptor::log(
        ID_CENTER,
        "center",
        "Center",
        "Phaser",
        100.0,
        5_000.0,
        800.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_DEPTH,
        "depth",
        "Depth",
        "Phaser",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_FEEDBACK,
        "feedback",
        "Feedback",
        "Phaser",
        -0.9,
        0.9,
        0.4,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_SPREAD,
        "spread",
        "Spread",
        "Stereo",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    ParamDescriptor::linear(ID_MIX, "mix", "Mix", "Output", 0.0, 1.0, 0.5, Unit::Percent),
    ParamDescriptor::linear(
        ID_OUTPUT,
        "output",
        "Output",
        "Output",
        -24.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.phaser",
    name: "Phaser",
    description: "Two to twelve stage all-pass phaser with feedback and tempo sync",
    category: "Modulation",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Classic",
        params: &[],
    },
    Preset {
        name: "Slow Swirl",
        params: &[
            ("stages", 4.0),
            ("rate", 0.12),
            ("center", 600.0),
            ("depth", 0.9),
            ("feedback", 0.5),
        ],
    },
    Preset {
        name: "Deep 12",
        params: &[
            ("stages", 10.0),
            ("rate", 0.3),
            ("center", 700.0),
            ("depth", 1.0),
            ("feedback", 0.6),
        ],
    },
    Preset {
        name: "Synced",
        params: &[
            ("sync", 1.0),
            ("beats", 2.0),
            ("stages", 6.0),
            ("depth", 0.85),
            ("feedback", 0.45),
        ],
    },
    Preset {
        name: "Subtle",
        params: &[
            ("stages", 2.0),
            ("rate", 0.2),
            ("depth", 0.4),
            ("feedback", 0.1),
            ("mix", 0.3),
        ],
    },
];

/// Phaser effect. See the module docs.
pub struct Phaser {
    store: ParamStore,
    sample_rate: f32,
    lfo: Lfo,
    sync: bool,
    rate_hz: f32,
    beats: f32,
    /// All-pass state per channel and stage.
    state: [[f32; MAX_STAGES]; 2],
    /// Last chain output per channel, for feedback.
    last: [f32; 2],
    stages: Smoothed,
    center_log2: Smoothed,
    depth: Smoothed,
    feedback: Smoothed,
    spread: Smoothed,
    mix: Smoothed,
    output: Smoothed,
    silence: SilenceTracker,
}

impl Default for Phaser {
    fn default() -> Self {
        Phaser::new()
    }
}

impl Phaser {
    pub fn new() -> Phaser {
        let mut p = Phaser {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            lfo: Lfo::new(0.0),
            sync: false,
            rate_hz: 0.4,
            beats: 4.0,
            state: [[0.0; MAX_STAGES]; 2],
            last: [0.0; 2],
            stages: Smoothed::new(4.0),
            center_log2: Smoothed::new(10.0),
            depth: Smoothed::new(0.0),
            feedback: Smoothed::new(0.0),
            spread: Smoothed::new(0.0),
            mix: Smoothed::new(0.5),
            output: Smoothed::new(1.0),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut p, 48_000.0, 512);
        p
    }

    fn smoothers(&mut self) -> [&mut Smoothed; 7] {
        [
            &mut self.stages,
            &mut self.center_log2,
            &mut self.depth,
            &mut self.feedback,
            &mut self.spread,
            &mut self.mix,
            &mut self.output,
        ]
    }
}

impl Fx for Phaser {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        for s in self.smoothers() {
            s.set_ramp(sr, RAMP_SECONDS * 2.0);
        }
        self.stages.set_ramp(sr, 0.08);
        refresh_all(self);
        for s in self.smoothers() {
            let t = s.target();
            s.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_STAGES => self.stages.set_target(value.round() + 2.0),
            ID_SYNC => self.sync = value >= 0.5,
            ID_RATE => self.rate_hz = value,
            ID_BEATS => self.beats = value,
            ID_CENTER => self.center_log2.set_target(value.log2()),
            ID_DEPTH => self.depth.set_target(value),
            ID_FEEDBACK => self.feedback.set_target(value),
            ID_SPREAD => self.spread.set_target(value),
            ID_MIX => self.mix.set_target(value),
            ID_OUTPUT => self.output.set_target(db_to_gain(value)),
            _ => {}
        }
    }

    fn clear(&mut self) {
        self.state = [[0.0; MAX_STAGES]; 2];
        self.last = [0.0; 2];
        self.silence.reset();
    }

    fn render(&mut self, ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        let sr = self.sample_rate;
        if self.sync {
            lfo_follow_tempo(&mut self.lfo, ctx, self.beats);
        } else {
            self.lfo.set_rate(self.rate_hz, sr);
        }
        let max_hz = 0.45 * sr;
        let w = std::f32::consts::PI / sr;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let stages = self.stages.next().clamp(2.0, MAX_STAGES as f32);
            let center = self.center_log2.next();
            let depth = self.depth.next();
            let fb = self.feedback.next();
            let spread = self.spread.next();
            let mix = self.mix.next();
            let out = self.output.next();
            let phase = self.lfo.next_phase();

            // Output tap: slide between stage `k` and `k + 1`.
            let k = stages.floor() as usize;
            let frac = stages - k as f32;
            let k1 = (k + 1).min(MAX_STAGES);

            let ins = [*l, *r];
            let mut res = [0.0f32; 2];
            for ch in 0..2 {
                let offset = if ch == 1 { spread * 0.5 } else { 0.0 };
                let m = fast_sin_turns(phase + offset);
                let hz = (center + depth * SWEEP_OCTAVES * m)
                    .exp2()
                    .clamp(15.0, max_hz);
                let t = (w * hz).tan();
                let a = (t - 1.0) / (t + 1.0);

                let mut x = soft_limit(ins[ch] + fb * self.last[ch]);
                let (mut tap_k, mut tap_k1) = (x, x);
                for (s, z) in self.state[ch].iter_mut().enumerate() {
                    let y = a * x + *z;
                    *z = sanitize(x - a * y);
                    x = y;
                    if s + 1 == k {
                        tap_k = y;
                    }
                    if s + 1 == k1 {
                        tap_k1 = y;
                    }
                }
                let wet = sanitize(tap_k + (tap_k1 - tap_k) * frac);
                self.last[ch] = wet;
                res[ch] = wet;
            }
            *l = (ins[0] + (res[0] - ins[0]) * mix) * out;
            *r = (ins[1] + (res[1] - ins[1]) * mix) * out;
        }
    }

    fn tail(&self) -> Option<f32> {
        let fb = self.store.get(ID_FEEDBACK).abs();
        let repeats = if fb < 1e-3 {
            1.0
        } else {
            (1e-3f32.ln() / fb.ln()).ceil().clamp(1.0, 100.0)
        };
        Some((0.2 * (repeats + 1.0)).min(20.0))
    }

    fn silent(&self) -> bool {
        let secs = self.tail().unwrap_or(2.0);
        self.silence.quiet_for((secs * self.sample_rate) as u32)
    }
}

impl_device!(Phaser, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;
    use peel_dsp::device::Device;

    #[test]
    fn every_preset_is_finite() {
        presets_are_finite("peel.fx.phaser");
    }

    #[test]
    fn mix_zero_passes_audio_unchanged() {
        let mut d = Phaser::new();
        d.set_param(ID_MIX, 0.0);
        d.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        for b in 0..40 {
            let src: Vec<f32> = (0..256).map(|i| sine(500.0, b * 256 + i, 0.5)).collect();
            let (mut l, mut r) = (src.clone(), src.clone());
            d.process(&ctx, &[], &mut l, &mut r);
            for (a, b) in src.iter().zip(&l) {
                assert!((a - b).abs() < 1e-6);
            }
        }
    }

    /// Frequency (Hz) of the lowest notch with the LFO locked to `beat`.
    fn notch_at(beat: f64, stages: f32) -> f64 {
        let mut found: Vec<(f64, f64)> = Vec::new();
        for step in 0..60 {
            let freq = 40.0 * (12_000.0f64 / 40.0).powf(f64::from(step) / 59.0);
            let mut d = Phaser::new();
            d.set_param(ID_STAGES, stages);
            d.set_param(ID_SYNC, 1.0);
            d.set_param(ID_BEATS, 4.0);
            d.set_param(ID_DEPTH, 1.0);
            d.set_param(ID_FEEDBACK, 0.0);
            d.set_param(ID_SPREAD, 0.0);
            d.set_param(ID_CENTER, 1_000.0);
            d.prepare(SR, 256);
            let mut ctx = ProcessContext::new(SR);
            ctx.playing = true;
            ctx.beat_position = beat;
            let mut tail = Vec::new();
            for b in 0..48 {
                let mut l: Vec<f32> = (0..256).map(|i| sine(freq, b * 256 + i, 0.5)).collect();
                let mut r = l.clone();
                d.process(&ctx, &[], &mut l, &mut r);
                if b >= 16 {
                    tail.extend(l);
                }
            }
            // Hann window so non-integer cycle counts do not leak.
            let n = tail.len() as f32;
            for (i, v) in tail.iter_mut().enumerate() {
                *v *= 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n).cos();
            }
            let level = tone_level(&tail, freq);
            found.push((freq, level));
        }
        (1..found.len() - 1)
            .find(|&i| {
                found[i].1 < 0.1 && found[i].1 <= found[i - 1].1 && found[i].1 <= found[i + 1].1
            })
            .map_or(0.0, |i| found[i].0)
    }

    #[test]
    fn notches_move_with_the_lfo() {
        // 4 beats per cycle: beat 1 is the LFO peak, beat 3 its trough.
        // Stage choice index 2 is four stages.
        let high = notch_at(1.0, 2.0);
        let low = notch_at(3.0, 2.0);
        assert!(high > 3.0 * low, "notch at peak {high} Hz, trough {low} Hz");
    }

    #[test]
    fn more_stages_make_more_notches() {
        let dips = |stages: f32| {
            let mut d = Phaser::new();
            d.set_param(ID_STAGES, stages);
            d.set_param(ID_DEPTH, 0.0);
            d.set_param(ID_FEEDBACK, 0.0);
            d.prepare(SR, 256);
            let ctx = ProcessContext::new(SR);
            let mut levels = Vec::new();
            for step in 0..120 {
                let freq = 50.0 * (15_000.0f64 / 50.0).powf(f64::from(step) / 119.0);
                let mut tail = Vec::new();
                for b in 0..12 {
                    let mut l: Vec<f32> = (0..256).map(|i| sine(freq, b * 256 + i, 0.5)).collect();
                    let mut r = l.clone();
                    d.process(&ctx, &[], &mut l, &mut r);
                    if b >= 6 {
                        tail.extend(l);
                    }
                }
                levels.push(tone_level(&tail, freq));
            }
            (1..levels.len() - 1)
                .filter(|&i| {
                    levels[i] < 0.1 && levels[i] <= levels[i - 1] && levels[i] <= levels[i + 1]
                })
                .count()
        };
        assert!(dips(12.0) > dips(2.0));
    }

    #[test]
    fn automation_does_not_click() {
        automation_is_click_free("peel.fx.phaser", 0.25);
    }
}
