//! Resonant multimode filter with drive and an LFO.
//!
//! The core is a trapezoidal-integration state-variable filter (Zavalishin's
//! TPT form), which stays stable and accurate while the cutoff is modulated
//! at audio rate. Low-pass, high-pass, band-pass and notch outputs are
//! weighted sums of the same states, so switching mode crossfades instead of
//! clicking. The 24 dB/oct slope runs two identical stages in series.
//!
//! * **Drive** applies a `tanh` saturator before the filter; loudness is
//!   partly compensated so the knob changes character more than level.
//! * **LFO** moves the cutoff by up to four octaves either way. The rate is
//!   free in Hz or locked to the host tempo; when the transport is playing a
//!   synced LFO follows the song position, so loops stay in phase.
//! * The output passes through a soft limiter that is transparent below
//!   0 dBFS, which tames resonant peaks.

use peel_dsp::delay::fast_sin_turns;
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain};

use crate::common::{
    Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all, snap_beats, soft_limit,
};

pub const ID_MODE: ParamId = 0;
pub const ID_CUTOFF: ParamId = 1;
pub const ID_RESONANCE: ParamId = 2;
pub const ID_SLOPE: ParamId = 3;
pub const ID_DRIVE: ParamId = 4;
pub const ID_LFO_RATE: ParamId = 5;
pub const ID_LFO_SYNC: ParamId = 6;
pub const ID_LFO_DIVISION: ParamId = 7;
pub const ID_LFO_DEPTH: ParamId = 8;
pub const ID_LFO_SHAPE: ParamId = 9;
pub const ID_MIX: ParamId = 10;
pub const ID_OUTPUT: ParamId = 11;

pub const MODE_LOW_PASS: u32 = 0;
pub const MODE_HIGH_PASS: u32 = 1;
pub const MODE_BAND_PASS: u32 = 2;
pub const MODE_NOTCH: u32 = 3;

const MODES: &[&str] = &["Low Pass", "High Pass", "Band Pass", "Notch"];
const SLOPES: &[&str] = &["12 dB/oct", "24 dB/oct"];
const SHAPES: &[&str] = &["Sine", "Triangle", "Saw", "Square"];

/// Samples between filter coefficient updates. Persistent across blocks, so
/// results do not depend on block size.
const CONTROL_PERIOD: u32 = 8;

static PARAMS: [ParamDescriptor; 12] = [
    ParamDescriptor::choice(ID_MODE, "mode", "Mode", "Filter", MODES, 0),
    ParamDescriptor::log(
        ID_CUTOFF,
        "cutoff",
        "Cutoff",
        "Filter",
        20.0,
        20_000.0,
        1_200.0,
        Unit::Hertz,
    ),
    ParamDescriptor::linear(
        ID_RESONANCE,
        "resonance",
        "Resonance",
        "Filter",
        0.0,
        1.0,
        0.25,
        Unit::Percent,
    ),
    ParamDescriptor::choice(ID_SLOPE, "slope", "Slope", "Filter", SLOPES, 0),
    ParamDescriptor::linear(
        ID_DRIVE,
        "drive",
        "Drive",
        "Filter",
        0.0,
        24.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::log(
        ID_LFO_RATE,
        "lfo.rate",
        "Rate",
        "LFO",
        0.05,
        20.0,
        1.0,
        Unit::Hertz,
    ),
    ParamDescriptor::toggle(ID_LFO_SYNC, "lfo.sync", "Sync", "LFO", false),
    ParamDescriptor::log(
        ID_LFO_DIVISION,
        "lfo.division",
        "Division",
        "LFO",
        0.125,
        8.0,
        1.0,
        Unit::Beats,
    ),
    ParamDescriptor::linear(
        ID_LFO_DEPTH,
        "lfo.depth",
        "Depth",
        "LFO",
        0.0,
        4.0,
        0.0,
        Unit::Octaves,
    ),
    ParamDescriptor::choice(ID_LFO_SHAPE, "lfo.shape", "Shape", "LFO", SHAPES, 0),
    ParamDescriptor::linear(ID_MIX, "mix", "Mix", "Output", 0.0, 1.0, 1.0, Unit::Percent),
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
    type_id: "peel.fx.filter",
    name: "Filter",
    description: "Resonant multimode filter with drive and tempo-syncable LFO",
    category: "Filter",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Soft Low Pass",
        params: &[],
    },
    Preset {
        name: "Sweeping Low Pass",
        params: &[
            ("cutoff", 900.0),
            ("resonance", 0.45),
            ("lfo.sync", 1.0),
            ("lfo.division", 4.0),
            ("lfo.depth", 1.6),
            ("lfo.shape", 1.0),
        ],
    },
    Preset {
        name: "Telephone",
        params: &[
            ("mode", 2.0),
            ("cutoff", 1_500.0),
            ("resonance", 0.3),
            ("slope", 1.0),
            ("drive", 6.0),
        ],
    },
    Preset {
        name: "Acid Squelch",
        params: &[
            ("cutoff", 500.0),
            ("resonance", 0.85),
            ("slope", 1.0),
            ("drive", 9.0),
            ("lfo.sync", 1.0),
            ("lfo.division", 0.5),
            ("lfo.depth", 1.2),
            ("lfo.shape", 2.0),
            ("output", -3.0),
        ],
    },
    Preset {
        name: "Rumble Cut",
        params: &[
            ("mode", 1.0),
            ("cutoff", 60.0),
            ("resonance", 0.1),
            ("slope", 1.0),
        ],
    },
    Preset {
        name: "Gate Wobble",
        params: &[
            ("cutoff", 700.0),
            ("resonance", 0.6),
            ("lfo.sync", 1.0),
            ("lfo.division", 0.5),
            ("lfo.depth", 2.2),
            ("lfo.shape", 3.0),
        ],
    },
];

/// One trapezoidal state-variable filter stage for one channel.
#[derive(Clone, Copy, Default)]
struct Svf {
    ic1: f64,
    ic2: f64,
}

/// Coefficients shared by every stage and channel.
#[derive(Clone, Copy)]
struct SvfCoeffs {
    k: f64,
    a1: f64,
    a2: f64,
    a3: f64,
}

impl SvfCoeffs {
    fn new(sample_rate: f32, cutoff: f32, q: f32) -> SvfCoeffs {
        let fc = f64::from(cutoff).clamp(20.0, f64::from(sample_rate) * 0.45);
        let g = (std::f64::consts::PI * fc / f64::from(sample_rate)).tan();
        let k = 1.0 / f64::from(q.max(0.2));
        let a1 = 1.0 / (1.0 + g * (g + k));
        SvfCoeffs {
            k,
            a1,
            a2: g * a1,
            a3: g * g * a1,
        }
    }
}

impl Svf {
    /// Returns `(low, band, high)` for one input sample.
    #[inline]
    fn tick(&mut self, c: &SvfCoeffs, x: f64) -> (f64, f64, f64) {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        if self.ic1.abs() < 1e-30 {
            self.ic1 = 0.0;
        }
        if self.ic2.abs() < 1e-30 {
            self.ic2 = 0.0;
        }
        (v2, v1, x - c.k * v1 - v2)
    }

    fn reset(&mut self) {
        *self = Svf::default();
    }
}

/// Filter effect. See the module docs.
pub struct Filter {
    store: ParamStore,
    sample_rate: f32,
    stages: [[Svf; 2]; 2],
    coeffs: SvfCoeffs,
    /// Output weights for low, band and high, crossfaded on mode change.
    w_low: Smoothed,
    w_band: Smoothed,
    w_high: Smoothed,
    ln_cutoff: Smoothed,
    resonance: Smoothed,
    drive: Smoothed,
    depth: Smoothed,
    mix: Smoothed,
    output: Smoothed,
    two_stage: bool,
    lfo_phase: f32,
    ctrl: u32,
    silence: SilenceTracker,
}

impl Default for Filter {
    fn default() -> Self {
        Filter::new()
    }
}

fn lfo_value(shape: u32, phase: f32) -> f32 {
    match shape {
        1 => {
            // Triangle, starting at 0 and rising.
            let p = phase - phase.floor();
            if p < 0.25 {
                4.0 * p
            } else if p < 0.75 {
                2.0 - 4.0 * p
            } else {
                4.0 * p - 4.0
            }
        }
        2 => {
            // Rising saw from -1 to 1.
            let p = phase - phase.floor();
            2.0 * p - 1.0
        }
        3 => {
            // Square with ~1 ms-equivalent edges: a steep sine clipped.
            (fast_sin_turns(phase) * 6.0).clamp(-1.0, 1.0)
        }
        _ => fast_sin_turns(phase),
    }
}

impl Filter {
    pub fn new() -> Filter {
        let mut f = Filter {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            stages: [[Svf::default(); 2]; 2],
            coeffs: SvfCoeffs::new(48_000.0, 1_000.0, 0.7),
            w_low: Smoothed::new(1.0),
            w_band: Smoothed::new(0.0),
            w_high: Smoothed::new(0.0),
            ln_cutoff: Smoothed::new(1_000f32.ln()),
            resonance: Smoothed::new(0.25),
            drive: Smoothed::new(1.0),
            depth: Smoothed::new(0.0),
            mix: Smoothed::new(1.0),
            output: Smoothed::new(1.0),
            two_stage: false,
            lfo_phase: 0.0,
            ctrl: 0,
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut f, 48_000.0, 512);
        f
    }

    fn smoothers(&mut self) -> [&mut Smoothed; 9] {
        [
            &mut self.w_low,
            &mut self.w_band,
            &mut self.w_high,
            &mut self.ln_cutoff,
            &mut self.resonance,
            &mut self.drive,
            &mut self.depth,
            &mut self.mix,
            &mut self.output,
        ]
    }

    /// Q for a resonance setting; the two-stage slope uses a gentler curve
    /// so the combined peak stays musical.
    fn q_for(&self, res: f32) -> f32 {
        let exponent = if self.two_stage { 0.55 } else { 1.0 };
        std::f32::consts::FRAC_1_SQRT_2 * 35f32.powf(res * exponent)
    }

    fn lfo_increment(&self, ctx: &ProcessContext) -> f32 {
        if self.store.get(ID_LFO_SYNC) >= 0.5 {
            let beats = f64::from(snap_beats(self.store.get(ID_LFO_DIVISION)));
            let cycles_per_second = ctx.tempo_bpm / 60.0 / beats;
            (cycles_per_second / f64::from(self.sample_rate)) as f32
        } else {
            self.store.get(ID_LFO_RATE) / self.sample_rate
        }
    }

    /// Nudges the free-running phase toward the song position when synced
    /// and playing; big jumps (loop, seek) snap.
    fn follow_transport(&mut self, ctx: &ProcessContext) {
        if self.store.get(ID_LFO_SYNC) < 0.5 || !ctx.playing {
            return;
        }
        let beats = f64::from(snap_beats(self.store.get(ID_LFO_DIVISION)));
        let target = (ctx.beat_position / beats).fract() as f32;
        let mut diff = target - self.lfo_phase;
        diff -= diff.round();
        if diff.abs() > 0.02 {
            self.lfo_phase = target;
        } else {
            self.lfo_phase += diff * 0.25;
        }
        self.lfo_phase -= self.lfo_phase.floor();
    }
}

impl Fx for Filter {
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
            s.set_ramp(sr, RAMP_SECONDS);
        }
        refresh_all(self);
        for s in self.smoothers() {
            let t = s.target();
            s.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_MODE => {
                let (lo, bp, hi) = match value as u32 {
                    MODE_HIGH_PASS => (0.0, 0.0, 1.0),
                    MODE_BAND_PASS => (0.0, 1.0, 0.0),
                    MODE_NOTCH => (1.0, 0.0, 1.0),
                    _ => (1.0, 0.0, 0.0),
                };
                self.w_low.set_target(lo);
                self.w_band.set_target(bp);
                self.w_high.set_target(hi);
            }
            ID_CUTOFF => self.ln_cutoff.set_target(value.ln()),
            ID_RESONANCE => self.resonance.set_target(value),
            ID_SLOPE => self.two_stage = value >= 0.5,
            ID_DRIVE => self.drive.set_target(db_to_gain(value)),
            ID_LFO_DEPTH => self.depth.set_target(value),
            ID_MIX => self.mix.set_target(value),
            ID_OUTPUT => self.output.set_target(db_to_gain(value)),
            _ => {}
        }
    }

    fn clear(&mut self) {
        for ch in &mut self.stages {
            for s in ch {
                s.reset();
            }
        }
        self.ctrl = 0;
        self.silence.reset();
    }

    fn render(&mut self, ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        self.follow_transport(ctx);
        let inc = self.lfo_increment(ctx).clamp(0.0, 0.45);
        let shape = self.store.get(ID_LFO_SHAPE) as u32;
        let sr = self.sample_rate;
        let max_hz = sr * 0.45;

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (w_lo, w_bp, w_hi) = (
                f64::from(self.w_low.next()),
                f64::from(self.w_band.next()),
                f64::from(self.w_high.next()),
            );
            let ln_fc = self.ln_cutoff.next();
            let res = self.resonance.next();
            let depth = self.depth.next();
            let drive = self.drive.next();
            let lfo = lfo_value(shape, self.lfo_phase);
            self.lfo_phase += inc;
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }

            if self.ctrl == 0 {
                let fc = (ln_fc + depth * lfo * std::f32::consts::LN_2)
                    .exp()
                    .clamp(20.0, max_hz);
                self.coeffs = SvfCoeffs::new(sr, fc, self.q_for(res));
                self.ctrl = CONTROL_PERIOD;
            }
            self.ctrl -= 1;

            let comp = drive.powf(-0.4);
            let band_scale = self.coeffs.k; // 0 dB peak band-pass
            let mut out = [0.0f32; 2];
            for (ch, input) in [*l, *r].into_iter().enumerate() {
                let x = if drive > 1.0001 {
                    f64::from((input * drive).tanh() * comp)
                } else {
                    f64::from(input)
                };
                let (lo, bp, hi) = self.stages[ch][0].tick(&self.coeffs, x);
                let mut y = w_lo * lo + w_bp * bp * band_scale + w_hi * hi;
                if self.two_stage {
                    let (lo2, bp2, hi2) = self.stages[ch][1].tick(&self.coeffs, y);
                    y = w_lo * lo2 + w_bp * bp2 * band_scale + w_hi * hi2;
                }
                out[ch] = y as f32;
            }

            let mix = self.mix.next();
            let gain = self.output.next();
            *l = (*l + (soft_limit(out[0]) - *l) * mix) * gain;
            *r = (*r + (soft_limit(out[1]) - *r) * mix) * gain;
        }
    }

    fn tail(&self) -> Option<f32> {
        // Ring time of a resonant section: about 7 * Q / (pi * fc).
        let fc = self.store.get(ID_CUTOFF).max(20.0);
        let q = self.q_for(self.store.get(ID_RESONANCE));
        Some((7.0 * q / (std::f32::consts::PI * fc)).clamp(0.05, 5.0))
    }

    fn silent(&self) -> bool {
        let secs = self.tail().unwrap_or(5.0);
        self.silence.quiet_for((secs * self.sample_rate) as u32)
    }
}

impl_device!(Filter, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;

    fn sine_gain(f: &mut Filter, freq: f64) -> f32 {
        let ctx = ProcessContext::new(SR);
        let n = 24_000;
        let mut energy = 0.0f64;
        let mut count = 0usize;
        for block in 0..(n / 256) {
            let mut l: Vec<f32> = (0..256)
                .map(|i| {
                    let idx = (block * 256 + i) as f64;
                    (std::f64::consts::TAU * (idx * freq / f64::from(SR)).fract()).sin() as f32
                        * 0.1
                })
                .collect();
            let mut r = l.clone();
            f.process(&ctx, &[], &mut l, &mut r);
            if block > n / 256 / 2 {
                energy += l.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>();
                count += 256;
            }
        }
        // RMS relative to the 0.1-amplitude input's RMS.
        (20.0 * ((2.0 * energy / count as f64).sqrt() / 0.1).log10()) as f32
    }

    fn configured(mode: u32, cutoff: f32, res: f32, slope: u32) -> Filter {
        let mut f = Filter::new();
        f.set_param(ID_MODE, mode as f32);
        f.set_param(ID_CUTOFF, cutoff);
        f.set_param(ID_RESONANCE, res);
        f.set_param(ID_SLOPE, slope as f32);
        f.prepare(SR, 256);
        f
    }

    #[test]
    fn low_pass_shape() {
        let mut f = configured(MODE_LOW_PASS, 2_000.0, 0.0, 0);
        assert!(sine_gain(&mut f, 200.0).abs() < 0.3);
        let at_fc = sine_gain(&mut f, 2_000.0);
        assert!((at_fc + 3.0).abs() < 0.6, "{at_fc}");
        // 12 dB/oct: about -24 dB two octaves up.
        let up = sine_gain(&mut f, 8_000.0);
        assert!((-30.0..-18.0).contains(&up), "{up}");
        let mut f = configured(MODE_LOW_PASS, 2_000.0, 0.0, 1);
        let up24 = sine_gain(&mut f, 8_000.0);
        assert!(up24 < up - 15.0, "{up24} vs {up}");
    }

    #[test]
    fn high_pass_band_pass_and_notch() {
        let mut f = configured(MODE_HIGH_PASS, 2_000.0, 0.0, 0);
        assert!(sine_gain(&mut f, 200.0) < -15.0);
        assert!(sine_gain(&mut f, 12_000.0).abs() < 0.5);
        let mut f = configured(MODE_BAND_PASS, 2_000.0, 0.5, 0);
        let centre = sine_gain(&mut f, 2_000.0);
        assert!(centre.abs() < 0.6, "{centre}");
        assert!(sine_gain(&mut f, 100.0) < -15.0);
        assert!(sine_gain(&mut f, 15_000.0) < -10.0);
        let mut f = configured(MODE_NOTCH, 2_000.0, 0.5, 0);
        assert!(sine_gain(&mut f, 2_000.0) < -30.0);
        assert!(sine_gain(&mut f, 200.0).abs() < 1.0);
    }

    #[test]
    fn resonance_boosts_the_cutoff() {
        let mut flat = configured(MODE_LOW_PASS, 1_000.0, 0.0, 0);
        let mut res = configured(MODE_LOW_PASS, 1_000.0, 0.8, 0);
        let a = sine_gain(&mut flat, 1_000.0);
        let b = sine_gain(&mut res, 1_000.0);
        assert!(b > a + 12.0, "{a} vs {b}");
    }

    #[test]
    fn lfo_moves_the_cutoff() {
        let ctx = ProcessContext::new(SR);
        let run = |depth: f32| {
            let mut f = configured(MODE_LOW_PASS, 1_000.0, 0.2, 0);
            f.set_param(ID_LFO_DEPTH, depth);
            f.set_param(ID_LFO_RATE, 2.0);
            f.prepare(SR, 256);
            // Feed a 3 kHz tone; the level swings as the cutoff sweeps.
            let mut lo = f32::MAX;
            let mut hi = 0.0f32;
            for block in 0..400 {
                let mut l: Vec<f32> = (0..256)
                    .map(|i| {
                        let idx = (block * 256 + i) as f64;
                        (std::f64::consts::TAU * (idx * 3_000.0 / f64::from(SR)).fract()).sin()
                            as f32
                            * 0.3
                    })
                    .collect();
                let mut r = l.clone();
                f.process(&ctx, &[], &mut l, &mut r);
                if block > 20 {
                    let p = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
            hi / lo.max(1e-6)
        };
        assert!(run(0.0) < 1.1);
        assert!(run(2.0) > 4.0);
    }

    #[test]
    fn tempo_synced_lfo_follows_beat_position() {
        let mut f = Filter::new();
        f.set_param(ID_LFO_SYNC, 1.0);
        f.set_param(ID_LFO_DIVISION, 1.0);
        f.set_param(ID_LFO_DEPTH, 1.0);
        f.prepare(SR, 256);
        let mut ctx = ProcessContext::new(SR);
        ctx.playing = true;
        ctx.beat_position = 0.25; // quarter of a cycle into a one-beat LFO
        let (mut l, mut r) = (vec![0.0; 16], vec![0.0; 16]);
        f.process(&ctx, &[], &mut l, &mut r);
        assert!(
            (f.lfo_phase - 0.25).abs() < 0.01 + 16.0 * 2.0 / SR,
            "{}",
            f.lfo_phase
        );
        // Jumping the transport snaps the phase.
        ctx.beat_position = 7.75;
        f.process(&ctx, &[], &mut l, &mut r);
        assert!(
            (f.lfo_phase - 0.75).abs() < 0.01 + 16.0 * 2.0 / SR,
            "{}",
            f.lfo_phase
        );
    }

    #[test]
    fn extreme_settings_stay_bounded() {
        let ctx = ProcessContext::new(SR);
        let mut f = configured(MODE_LOW_PASS, 20.0, 1.0, 1);
        f.set_param(ID_DRIVE, 24.0);
        f.set_param(ID_LFO_DEPTH, 4.0);
        f.set_param(ID_LFO_RATE, 20.0);
        let mut seed = 5u32;
        for _ in 0..400 {
            let mut l: Vec<f32> = (0..256)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
                })
                .collect();
            let mut r = l.clone();
            f.process(&ctx, &[], &mut l, &mut r);
            assert!(l.iter().all(|v| v.is_finite() && v.abs() <= 2.5));
        }
    }

    #[test]
    fn mode_switch_is_click_free() {
        let ctx = ProcessContext::new(SR);
        let mut f = configured(MODE_LOW_PASS, 1_000.0, 0.3, 0);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for block in 0..60 {
            let mut l: Vec<f32> = (0..256)
                .map(|i| 0.4 * ((block * 256 + i) as f32 * 0.02).sin())
                .collect();
            let mut r = l.clone();
            if block == 30 {
                f.set_param(ID_MODE, MODE_HIGH_PASS as f32);
            }
            f.process(&ctx, &[], &mut l, &mut r);
            for v in l {
                worst = worst.max((v - prev).abs());
                prev = v;
            }
        }
        assert!(worst < 0.1, "{worst}");
    }
}
