//! Flanger: a short modulated delay mixed with the dry signal.
//!
//! The delay (0.1 to 10 ms) sweeps exponentially around the *Delay* control
//! under a sine LFO, free in Hz or locked to the host tempo. The right
//! channel's LFO is offset by *Spread*. Feedback runs from -95% to +95% (a
//! soft limiter bounds the loop).
//!
//! *Through zero* delays the dry signal by the centre delay and sweeps the
//! wet delay linearly around it, so the two cross each other every cycle and
//! the comb inverts through a full cancellation. It adds up to 10 ms of
//! latency to the dry path while engaged; the change is crossfaded.

use peel_dsp::delay::{DelayLine, Lfo, fast_sin_turns};
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain, sanitize};

use crate::common::{
    Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, lfo_follow_tempo, refresh_all,
    soft_limit,
};

pub const ID_SYNC: ParamId = 0;
pub const ID_RATE: ParamId = 1;
pub const ID_BEATS: ParamId = 2;
pub const ID_DELAY: ParamId = 3;
pub const ID_DEPTH: ParamId = 4;
pub const ID_FEEDBACK: ParamId = 5;
pub const ID_SPREAD: ParamId = 6;
pub const ID_THROUGH_ZERO: ParamId = 7;
pub const ID_MIX: ParamId = 8;
pub const ID_OUTPUT: ParamId = 9;

/// Sweep range at full depth, in octaves either side of the centre delay.
const SWEEP_OCTAVES: f32 = 2.5;
const MIN_DELAY_MS: f32 = 0.1;
const MAX_DELAY_MS: f32 = 10.0;

static PARAMS: [ParamDescriptor; 10] = [
    ParamDescriptor::toggle(ID_SYNC, "sync", "Sync", "Rate", false),
    ParamDescriptor::log(
        ID_RATE,
        "rate",
        "Rate",
        "Rate",
        0.02,
        10.0,
        0.25,
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
    ParamDescriptor::log(
        ID_DELAY,
        "delay",
        "Delay",
        "Sweep",
        MIN_DELAY_MS,
        MAX_DELAY_MS,
        1.5,
        Unit::Milliseconds,
    ),
    ParamDescriptor::linear(
        ID_DEPTH,
        "depth",
        "Depth",
        "Sweep",
        0.0,
        1.0,
        0.7,
        Unit::Percent,
    ),
    ParamDescriptor::linear(
        ID_FEEDBACK,
        "feedback",
        "Feedback",
        "Sweep",
        -0.95,
        0.95,
        0.6,
        Unit::Percent,
    ),
    ParamDescriptor::toggle(
        ID_THROUGH_ZERO,
        "through_zero",
        "Through Zero",
        "Sweep",
        false,
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
    type_id: "peel.fx.flanger",
    name: "Flanger",
    description: "Modulated short delay with feedback, tempo sync and through-zero",
    category: "Modulation",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Classic Jet",
        params: &[],
    },
    Preset {
        name: "Metallic",
        params: &[
            ("rate", 0.5),
            ("delay", 0.6),
            ("depth", 0.3),
            ("feedback", -0.85),
        ],
    },
    Preset {
        name: "Slow Sweep",
        params: &[
            ("rate", 0.05),
            ("delay", 3.0),
            ("depth", 1.0),
            ("feedback", 0.5),
            ("through_zero", 1.0),
        ],
    },
    Preset {
        name: "Synced 1 Bar",
        params: &[
            ("sync", 1.0),
            ("beats", 4.0),
            ("delay", 2.5),
            ("depth", 0.8),
            ("feedback", 0.5),
        ],
    },
    Preset {
        name: "Subtle",
        params: &[
            ("rate", 0.15),
            ("delay", 4.0),
            ("depth", 0.3),
            ("feedback", 0.2),
            ("mix", 0.3),
        ],
    },
];

/// Flanger effect. See the module docs.
pub struct Flanger {
    store: ParamStore,
    sample_rate: f32,
    lines: [DelayLine; 2],
    dry_lines: [DelayLine; 2],
    lfo: Lfo,
    sync: bool,
    rate_hz: f32,
    beats: f32,
    delay: Smoothed,
    depth: Smoothed,
    feedback: Smoothed,
    spread: Smoothed,
    through_zero: Smoothed,
    mix: Smoothed,
    output: Smoothed,
    silence: SilenceTracker,
}

impl Default for Flanger {
    fn default() -> Self {
        Flanger::new()
    }
}

impl Flanger {
    pub fn new() -> Flanger {
        let mut f = Flanger {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            lines: [DelayLine::new(16), DelayLine::new(16)],
            dry_lines: [DelayLine::new(16), DelayLine::new(16)],
            lfo: Lfo::new(0.0),
            sync: false,
            rate_hz: 0.25,
            beats: 4.0,
            delay: Smoothed::new(0.0),
            depth: Smoothed::new(0.0),
            feedback: Smoothed::new(0.0),
            spread: Smoothed::new(0.0),
            through_zero: Smoothed::new(0.0),
            mix: Smoothed::new(0.5),
            output: Smoothed::new(1.0),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut f, 48_000.0, 512);
        f
    }

    fn smoothers(&mut self) -> [&mut Smoothed; 7] {
        [
            &mut self.delay,
            &mut self.depth,
            &mut self.feedback,
            &mut self.spread,
            &mut self.through_zero,
            &mut self.mix,
            &mut self.output,
        ]
    }
}

impl Fx for Flanger {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        let max = (2.0 * MAX_DELAY_MS * 0.001 * sr) as usize + 8;
        self.lines = [DelayLine::new(max), DelayLine::new(max)];
        self.dry_lines = [DelayLine::new(max), DelayLine::new(max)];
        for s in self.smoothers() {
            s.set_ramp(sr, RAMP_SECONDS * 2.0);
        }
        refresh_all(self);
        for s in self.smoothers() {
            let t = s.target();
            s.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        let sr = self.sample_rate;
        match id {
            ID_SYNC => self.sync = value >= 0.5,
            ID_RATE => self.rate_hz = value,
            ID_BEATS => self.beats = value,
            ID_DELAY => self.delay.set_target(value * 0.001 * sr),
            ID_DEPTH => self.depth.set_target(value),
            ID_FEEDBACK => self.feedback.set_target(value),
            ID_SPREAD => self.spread.set_target(value),
            ID_THROUGH_ZERO => self.through_zero.set_target(value),
            ID_MIX => self.mix.set_target(value),
            ID_OUTPUT => self.output.set_target(db_to_gain(value)),
            _ => {}
        }
    }

    fn clear(&mut self) {
        for l in self.lines.iter_mut().chain(self.dry_lines.iter_mut()) {
            l.clear();
        }
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
        let min_d = MIN_DELAY_MS * 0.001 * sr;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let manual = self.delay.next();
            let depth = self.depth.next();
            let fb = self.feedback.next();
            let spread = self.spread.next();
            let tz = self.through_zero.next();
            let mix = self.mix.next();
            let out = self.output.next();
            let phase = self.lfo.next_phase();
            let max_d = MAX_DELAY_MS * 0.001 * sr * (1.0 + tz);

            let ins = [*l, *r];
            let mut wet = [0.0f32; 2];
            let mut dry = ins;
            for ch in 0..2 {
                let offset = if ch == 1 { spread * 0.5 } else { 0.0 };
                let m = fast_sin_turns(phase + offset);
                let d_exp = manual * (depth * SWEEP_OCTAVES * m).exp2();
                let d_lin = manual * (1.0 + depth * m);
                let d = (d_exp + (d_lin - d_exp) * tz).clamp(min_d, max_d);
                wet[ch] = self.lines[ch].read_hermite(d);
                if tz > 1e-4 {
                    let delayed = self.dry_lines[ch].read_hermite(manual.max(2.0));
                    dry[ch] = ins[ch] + (delayed - ins[ch]) * tz;
                }
            }
            for ch in 0..2 {
                self.lines[ch].push(sanitize(soft_limit(ins[ch] + fb * wet[ch])));
                self.dry_lines[ch].push(ins[ch]);
            }
            *l = (dry[0] + (wet[0] - dry[0]) * mix) * out;
            *r = (dry[1] + (wet[1] - dry[1]) * mix) * out;
        }
    }

    fn tail(&self) -> Option<f32> {
        let fb = self.store.get(ID_FEEDBACK).abs();
        let one = 2.0 * MAX_DELAY_MS * 0.001;
        let repeats = if fb < 1e-3 {
            1.0
        } else {
            (1e-3f32.ln() / fb.ln()).ceil().clamp(1.0, 200.0)
        };
        Some(one * (repeats + 1.0))
    }

    fn silent(&self) -> bool {
        let secs = self.tail().unwrap_or(2.0);
        self.silence.quiet_for((secs * self.sample_rate) as u32)
    }
}

impl_device!(Flanger, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;
    use peel_dsp::device::Device;

    fn run_noise(d: &mut dyn Device, blocks: usize) -> (Vec<f32>, Vec<f32>) {
        let ctx = ProcessContext::new(SR);
        let mut rng = Rng(3);
        let (mut src, mut out) = (Vec::new(), Vec::new());
        for _ in 0..blocks {
            let mut l: Vec<f32> = (0..256).map(|_| 0.3 * rng.next()).collect();
            let mut r = l.clone();
            src.extend_from_slice(&l);
            d.process(&ctx, &[], &mut l, &mut r);
            out.extend(l);
        }
        (src, out)
    }

    #[test]
    fn every_preset_is_finite() {
        presets_are_finite("peel.fx.flanger");
    }

    #[test]
    fn mix_zero_passes_audio_unchanged() {
        let mut d = Flanger::new();
        d.set_param(ID_MIX, 0.0);
        d.prepare(SR, 256);
        let (src, out) = run_noise(&mut d, 40);
        for (a, b) in src.iter().zip(&out) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn feedback_polarity_changes_the_comb() {
        // A positive feedback comb boosts a tone at multiples of 1/delay;
        // negative feedback boosts the odd half-multiples instead.
        let level = |fb: f32, freq: f64| {
            let mut d = Flanger::new();
            d.set_param(ID_DEPTH, 0.0);
            d.set_param(ID_DELAY, 1.0);
            d.set_param(ID_FEEDBACK, fb);
            d.prepare(SR, 256);
            let ctx = ProcessContext::new(SR);
            let mut peak = 0.0f32;
            for b in 0..100 {
                let mut l: Vec<f32> = (0..256).map(|i| sine(freq, b * 256 + i, 0.2)).collect();
                let mut r = l.clone();
                d.process(&ctx, &[], &mut l, &mut r);
                if b > 50 {
                    peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
                }
            }
            peak
        };
        // 1 ms delay: peaks at 1 kHz multiples for positive feedback.
        assert!(level(0.8, 1_000.0) > 2.0 * level(0.8, 1_500.0));
        assert!(level(-0.8, 1_500.0) > 2.0 * level(-0.8, 1_000.0));
    }

    #[test]
    fn tempo_sync_follows_the_song_position() {
        let mut d = Flanger::new();
        d.set_param(ID_SYNC, 1.0);
        d.set_param(ID_BEATS, 4.0);
        d.set_param(ID_FEEDBACK, 0.0);
        d.prepare(SR, 256);
        let mut ctx = ProcessContext::new(SR);
        ctx.playing = true;
        // Two positions one half-cycle apart must yield different delays.
        let mut probe = |d: &mut Flanger, beat: f64| {
            ctx.beat_position = beat;
            let mut l = vec![0.0f32; 64];
            let mut r = vec![0.0f32; 64];
            l[0] = 1.0;
            r[0] = 1.0;
            for _ in 0..4 {
                d.process(&ctx, &[], &mut l, &mut r);
                l.fill(0.0);
                r.fill(0.0);
            }
            d.lfo.phase
        };
        let a = probe(&mut d, 1.0);
        let b = probe(&mut d, 3.0);
        assert!((a - 0.25).abs() < 0.01, "{a}");
        assert!((b - 0.75).abs() < 0.01, "{b}");
    }

    #[test]
    fn through_zero_crosses_the_dry_signal() {
        let mut d = Flanger::new();
        d.set_param(ID_THROUGH_ZERO, 1.0);
        d.set_param(ID_RATE, 2.0);
        d.set_param(ID_DEPTH, 1.0);
        d.set_param(ID_FEEDBACK, 0.0);
        d.prepare(SR, 256);
        let (_, out) = run_noise(&mut d, 400);
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 2.0));
    }

    #[test]
    fn automation_does_not_click() {
        automation_is_click_free("peel.fx.flanger", 0.25);
    }
}
