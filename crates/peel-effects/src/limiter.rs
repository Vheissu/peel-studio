//! Lookahead brickwall limiter.
//!
//! A look-ahead limiter with an input gain stage. The lookahead is fixed at
//! 4 ms and reported through `latency()`, so the host can compensate; it
//! does not change with any parameter. Output sample peaks never exceed the
//! ceiling. With *True Peak* on, the detector reads a 4x oversampled view of
//! the signal, so inter-sample overs are caught to within a few hundredths of
//! a dB for ordinary material (a 12-point interpolator, so extreme content
//! right at the Nyquist frequency can stay a few tenths of a dB over). The
//! gain holds steady across the cycles of bass notes and recovers in two
//! stages, which keeps 3 to 6 dB of reduction free of bass distortion and
//! pumping; see [`Core`] for the details.

use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::dynamics::time_coeff;
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain, gain_to_db};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};

pub const ID_INPUT: ParamId = 0;
pub const ID_CEILING: ParamId = 1;
pub const ID_RELEASE: ParamId = 2;
pub const ID_TRUE_PEAK: ParamId = 3;

/// Lookahead in seconds.
const LOOKAHEAD_SECONDS: f32 = 0.004;

static PARAMS: [ParamDescriptor; 4] = [
    ParamDescriptor::linear(
        ID_INPUT,
        "input",
        "Input",
        "Level",
        -12.0,
        24.0,
        0.0,
        Unit::Decibels,
    ),
    ParamDescriptor::linear(
        ID_CEILING,
        "ceiling",
        "Ceiling",
        "Level",
        -12.0,
        0.0,
        -1.0,
        Unit::Decibels,
    ),
    ParamDescriptor::log(
        ID_RELEASE,
        "release",
        "Release",
        "Timing",
        1.0,
        1_000.0,
        100.0,
        Unit::Milliseconds,
    ),
    ParamDescriptor::toggle(ID_TRUE_PEAK, "true_peak", "True Peak", "Detector", true),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.limiter",
    name: "Limiter",
    description: "Lookahead brickwall limiter with true-peak detection",
    category: "Dynamics",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 5] = [
    Preset {
        name: "Transparent",
        params: &[],
    },
    Preset {
        name: "Master Safety",
        params: &[("ceiling", -0.3), ("release", 250.0)],
    },
    Preset {
        name: "Loud Master",
        params: &[("input", 6.0), ("ceiling", -0.5), ("release", 60.0)],
    },
    Preset {
        name: "Drum Bus Catch",
        params: &[("input", 3.0), ("ceiling", -2.0), ("release", 30.0)],
    },
    Preset {
        name: "Soft Peaks",
        params: &[("input", 2.0), ("ceiling", -1.0), ("release", 400.0)],
    },
];

/// Samples of detector delay: the true-peak interpolator needs six samples
/// of look-ahead on its newest side.
const DETECT_DELAY: usize = 6;
/// Interpolator taps (a 12-point windowed sinc).
const TAPS: usize = 12;

/// Fractional-delay interpolation weights for a quarter, half and
/// three-quarter sample offset: a Kaiser-windowed sinc, normalized to unit
/// gain at DC. Together with the sample itself these give a 4x oversampled
/// view of the signal for true-peak detection.
fn interpolator() -> [[f32; TAPS]; 3] {
    fn bessel_i0(x: f64) -> f64 {
        let (mut sum, mut term) = (1.0f64, 1.0f64);
        for k in 1..30 {
            term *= (x / (2.0 * f64::from(k))).powi(2);
            sum += term;
        }
        sum
    }
    let beta = 7.0f64;
    let mut out = [[0.0f32; TAPS]; 3];
    for (p, row) in out.iter_mut().enumerate() {
        let f = 0.25 * (p + 1) as f64;
        let mut w = [0.0f64; TAPS];
        for (j, v) in w.iter_mut().enumerate() {
            // Tap j reads the sample at offset (j - 5) from the centre.
            let t = (j as f64 - 5.0) - f;
            let sinc = if t.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t)
            };
            let r = t / 6.0;
            let win = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta);
            *v = sinc * win;
        }
        let sum: f64 = w.iter().sum();
        for (o, v) in row.iter_mut().zip(w) {
            *o = (v / sum) as f32;
        }
    }
    out
}

/// Sliding-window minimum with a window that can change at run time (up to
/// the capacity chosen at construction).
#[derive(Clone, Debug)]
struct SlidingMin {
    vals: Vec<f32>,
    pos: Vec<u64>,
    head: usize,
    len: usize,
    window: u64,
    t: u64,
}

impl SlidingMin {
    fn new(max_window: usize) -> SlidingMin {
        let cap = max_window.max(1) + 2;
        SlidingMin {
            vals: vec![1.0; cap],
            pos: vec![0; cap],
            head: 0,
            len: 0,
            window: max_window.max(1) as u64,
            t: 0,
        }
    }

    fn set_window(&mut self, window: usize) {
        self.window = (window.max(1) as u64).min(self.vals.len() as u64 - 2);
    }

    fn reset(&mut self) {
        self.head = 0;
        self.len = 0;
        self.t = 0;
    }

    #[inline]
    fn push(&mut self, x: f32) -> f32 {
        let cap = self.vals.len();
        while self.len > 0 {
            let back = (self.head + self.len - 1) % cap;
            if self.vals[back] >= x {
                self.len -= 1;
            } else {
                break;
            }
        }
        let slot = (self.head + self.len) % cap;
        self.vals[slot] = x;
        self.pos[slot] = self.t;
        self.len += 1;
        while self.len > 1 && self.pos[self.head] + self.window <= self.t {
            self.head = (self.head + 1) % cap;
            self.len -= 1;
        }
        self.t += 1;
        self.vals[self.head]
    }
}

/// Running average over a fixed window with an `f64` accumulator.
#[derive(Clone, Debug)]
struct MovingAverage {
    ring: Vec<f32>,
    pos: usize,
    sum: f64,
}

impl MovingAverage {
    fn new(window: usize) -> MovingAverage {
        let n = window.max(1);
        MovingAverage {
            ring: vec![1.0; n],
            pos: 0,
            sum: n as f64,
        }
    }

    fn reset(&mut self) {
        self.ring.fill(1.0);
        self.pos = 0;
        self.sum = self.ring.len() as f64;
    }

    #[inline]
    fn push(&mut self, x: f32) -> f32 {
        let old = self.ring[self.pos];
        self.ring[self.pos] = x;
        self.pos += 1;
        if self.pos == self.ring.len() {
            self.pos = 0;
            // Re-sum once per revolution so rounding cannot accumulate.
            self.sum = self.ring.iter().map(|&v| f64::from(v)).sum();
        } else {
            self.sum += f64::from(x) - f64::from(old);
        }
        (self.sum / self.ring.len() as f64) as f32
    }
}

/// Longest hold in seconds (see [`Core::set_release`]).
const MAX_HOLD: f32 = 0.012;

/// Stereo-linked look-ahead limiter with true-peak detection.
///
/// Per sample: the input goes into a delay of `lookahead` samples while the
/// detector (a 4x oversampled peak reader, six samples behind the input)
/// computes the gain each peak needs. That gain goes through a sliding
/// minimum covering the look-ahead plus a short *hold*, so the reduction is
/// in place before a peak arrives and does not let go between the cycles of a
/// bass note, which is what makes a plain release distort low frequencies.
/// A two-stage exponential release (a fast and a slow recovery averaged)
/// lets an isolated peak recover quickly while sustained reduction eases off
/// slowly, and a moving average the length of the look-ahead window turns the
/// steps into linear ramps. The windows are sized so the delayed sample's own
/// peak is always inside them: sample peaks never exceed the ceiling (a final
/// clamp removes rounding residue) and true peaks stay within a few hundredths
/// of a dB for ordinary material.
#[derive(Clone, Debug)]
struct Core {
    lookahead: usize,
    ring_l: Vec<f32>,
    ring_r: Vec<f32>,
    mask: usize,
    write: usize,
    weights: [[f32; TAPS]; 3],
    min: SlidingMin,
    avg: MovingAverage,
    window: usize,
    ceiling: f32,
    fast_coeff: f32,
    slow_coeff: f32,
    fast: f32,
    slow: f32,
    true_peak: bool,
    gain: f32,
}

impl Core {
    /// `lookahead` is in samples and is also the latency.
    fn new(lookahead: usize, sample_rate: f32) -> Core {
        let lookahead = lookahead.max(DETECT_DELAY + 16);
        let cap = (lookahead + 32).next_power_of_two();
        let window = lookahead - DETECT_DELAY - 1;
        let max_hold = (MAX_HOLD * sample_rate) as usize + 1;
        let mut c = Core {
            lookahead,
            ring_l: vec![0.0; cap],
            ring_r: vec![0.0; cap],
            mask: cap - 1,
            write: 0,
            weights: interpolator(),
            min: SlidingMin::new(window + max_hold),
            avg: MovingAverage::new(window),
            window,
            ceiling: 1.0,
            fast_coeff: 0.999,
            slow_coeff: 0.9999,
            fast: 1.0,
            slow: 1.0,
            true_peak: true,
            gain: 1.0,
        };
        c.set_release(0.1, sample_rate);
        c
    }

    fn lookahead(&self) -> usize {
        self.lookahead
    }

    fn set_ceiling_db(&mut self, db: f32) {
        self.ceiling = db_to_gain(db.clamp(-60.0, 6.0)).max(1e-6);
    }

    /// Release in seconds. The fast stage recovers with half of it and the
    /// slow stage with three times it. The hold, which keeps the gain steady
    /// across the cycles of low notes, is a third of the release between
    /// 2 ms and 12 ms.
    fn set_release(&mut self, seconds: f32, sample_rate: f32) {
        let r = seconds.max(1e-4);
        self.fast_coeff = time_coeff(0.5 * r, sample_rate);
        self.slow_coeff = time_coeff(3.0 * r, sample_rate);
        let hold = (r / 3.0).clamp(0.002, MAX_HOLD);
        self.min
            .set_window(self.window + (hold * sample_rate) as usize);
    }

    fn set_true_peak(&mut self, on: bool) {
        self.true_peak = on;
    }

    fn reset(&mut self) {
        self.ring_l.fill(0.0);
        self.ring_r.fill(0.0);
        self.write = 0;
        self.min.reset();
        self.avg.reset();
        self.fast = 1.0;
        self.slow = 1.0;
        self.gain = 1.0;
    }

    /// Gain applied to the most recent sample, as a positive reduction in dB.
    fn gain_reduction_db(&self) -> f32 {
        -gain_to_db(self.gain)
    }

    /// Sample `back` frames before the newest one.
    #[inline]
    fn at(&self, back: usize, right: bool) -> f32 {
        let i = self.write.wrapping_sub(1 + back) & self.mask;
        if right {
            self.ring_r[i]
        } else {
            self.ring_l[i]
        }
    }

    /// Largest absolute value among the sample `DETECT_DELAY` frames back and
    /// the three interpolated points that follow it.
    #[inline]
    fn detect(&self) -> f32 {
        let mut peak = self.at(DETECT_DELAY, false).abs();
        peak = peak.max(self.at(DETECT_DELAY, true).abs());
        if self.true_peak {
            for right in [false, true] {
                let mut acc = [0.0f32; 3];
                for j in 0..TAPS {
                    // Tap j reads the sample j - 5 after the centre.
                    let x = self.at(DETECT_DELAY + 5 - j, right);
                    for (a, w) in acc.iter_mut().zip(&self.weights) {
                        *a += w[j] * x;
                    }
                }
                for a in acc {
                    peak = peak.max(a.abs());
                }
            }
        }
        peak
    }

    /// Processes one stereo frame, returning the limited frame delayed by
    /// [`Core::lookahead`] samples.
    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let l = if l.is_finite() { l } else { 0.0 };
        let r = if r.is_finite() { r } else { 0.0 };
        self.ring_l[self.write] = l;
        self.ring_r[self.write] = r;
        self.write = (self.write + 1) & self.mask;

        let peak = self.detect();
        let target = if peak > self.ceiling {
            self.ceiling / peak
        } else {
            1.0
        };
        let m = self.min.push(target);
        self.fast = if m < self.fast {
            m
        } else {
            m + self.fast_coeff * (self.fast - m)
        };
        self.slow = if m < self.slow {
            m
        } else {
            m + self.slow_coeff * (self.slow - m)
        };
        let released = 0.5 * (self.fast + self.slow);
        let g = self.avg.push(released.min(1.0)).min(1.0);
        self.gain = g;

        let dl = self.at(self.lookahead, false);
        let dr = self.at(self.lookahead, true);
        let c = self.ceiling;
        ((dl * g).clamp(-c, c), (dr * g).clamp(-c, c))
    }
}

/// Brickwall limiter. Read the current gain reduction with
/// [`Limiter::gain_reduction_db`].
pub struct Limiter {
    store: ParamStore,
    sample_rate: f32,
    core: Core,
    input: Smoothed,
    silence: SilenceTracker,
}

impl Default for Limiter {
    fn default() -> Self {
        Limiter::new()
    }
}

impl Limiter {
    pub fn new() -> Limiter {
        let mut l = Limiter {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            core: Core::new(192, 48_000.0),
            input: Smoothed::new(1.0),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut l, 48_000.0, 512);
        l
    }

    /// Gain reduction in dB (zero or positive) applied to the latest sample.
    pub fn gain_reduction_db(&self) -> f32 {
        self.core.gain_reduction_db()
    }
}

impl Fx for Limiter {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn reduction(&self) -> Option<f32> {
        Some(self.core.gain_reduction_db())
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let look = (self.sample_rate * LOOKAHEAD_SECONDS).round() as usize;
        self.core = Core::new(look, self.sample_rate);
        self.input.set_ramp(self.sample_rate, RAMP_SECONDS);
        refresh_all(self);
        let t = self.input.target();
        self.input.set_immediate(t);
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_INPUT => self.input.set_target(db_to_gain(value)),
            ID_CEILING => self.core.set_ceiling_db(value),
            ID_RELEASE => self.core.set_release(value * 0.001, self.sample_rate),
            ID_TRUE_PEAK => self.core.set_true_peak(value >= 0.5),
            _ => {}
        }
    }

    fn clear(&mut self) {
        self.core.reset();
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = self.input.next();
            let (a, b) = self.core.process(*l * g, *r * g);
            *l = a;
            *r = b;
        }
    }

    fn latency_samples(&self) -> u32 {
        self.core.lookahead() as u32
    }

    fn tail(&self) -> Option<f32> {
        Some(self.core.lookahead() as f32 / self.sample_rate)
    }

    fn silent(&self) -> bool {
        self.silence.quiet_for(self.core.lookahead() as u32 + 8)
    }
}

impl_device!(Limiter, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;

    #[test]
    fn never_exceeds_ceiling_on_hot_program() {
        for ceiling in [-0.3f32, -3.0, -9.0] {
            let mut lim = Limiter::new();
            lim.set_param(ID_INPUT, 18.0);
            lim.set_param(ID_CEILING, ceiling);
            lim.prepare(SR, 256);
            let limit = db_to_gain(ceiling);
            let ctx = ProcessContext::new(SR);
            let mut seed = 3u32;
            let mut worst = 0.0f32;
            for block in 0..400 {
                let mut l: Vec<f32> = (0..256)
                    .map(|i| {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        let n = ((seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0;
                        0.9 * (((block * 256 + i) as f32) * 0.03).sin() + 0.6 * n
                    })
                    .collect();
                let mut r: Vec<f32> = l.iter().map(|v| -v * 0.8).collect();
                lim.process(&ctx, &[], &mut l, &mut r);
                worst = l.iter().chain(r.iter()).fold(worst, |m, v| m.max(v.abs()));
            }
            assert!(
                worst <= limit + 1e-6,
                "ceiling {ceiling}: peak {worst} > {limit}"
            );
            assert!(worst > limit * 0.95);
        }
    }

    #[test]
    fn reports_constant_latency_and_aligns_output() {
        let mut lim = Limiter::new();
        lim.prepare(SR, 256);
        let lat = lim.latency() as usize;
        assert_eq!(lat, (SR * 0.004) as usize);
        let ctx = ProcessContext::new(SR);
        let mut l = vec![0.0; 512];
        l[10] = 0.5;
        let mut r = l.clone();
        lim.process(&ctx, &[], &mut l, &mut r);
        assert!((l[10 + lat] - 0.5).abs() < 1e-6);
        assert!(l[10].abs() < 1e-6);
        // Latency does not depend on parameters.
        lim.set_param(ID_RELEASE, 5.0);
        lim.set_param(ID_TRUE_PEAK, 0.0);
        assert_eq!(lim.latency() as usize, lat);
    }

    #[test]
    fn sample_rate_change_updates_latency() {
        let mut lim = Limiter::new();
        lim.prepare(44_100.0, 256);
        let a = lim.latency();
        lim.prepare(96_000.0, 256);
        assert!(lim.latency() > a);
    }

    #[test]
    fn tail_covers_the_delayed_signal() {
        let mut lim = Limiter::new();
        lim.prepare(SR, 128);
        assert!((lim.tail_seconds().unwrap() - 0.004).abs() < 1e-4);
    }

    fn run_mono(lim: &mut Limiter, x: &[f32]) -> Vec<f32> {
        let ctx = ProcessContext::new(SR);
        let mut l = x.to_vec();
        let mut r = x.to_vec();
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            lim.process(&ctx, &[], cl, cr);
        }
        l
    }

    /// Amplitude of the component at `freq` (single-bin DFT).
    fn tone_amp(x: &[f32], freq: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let ph = std::f64::consts::TAU * f64::from(freq) * i as f64 / f64::from(SR);
            re += f64::from(*v) * ph.cos();
            im += f64::from(*v) * ph.sin();
        }
        (2.0 * (re * re + im * im).sqrt() / x.len() as f64) as f32
    }

    #[test]
    fn true_peak_catches_inter_sample_overs() {
        // A sine at a quarter of the sample rate with its crests exactly
        // between samples: the samples read 0.707 of the real peak.
        let mut lim = Limiter::new();
        lim.set_param(ID_CEILING, -1.0);
        lim.prepare(SR, 256);
        let x: Vec<f32> = (0..48_000)
            .map(|i| {
                1.4 * (std::f32::consts::FRAC_PI_2 * i as f32 + std::f32::consts::FRAC_PI_4).sin()
            })
            .collect();
        let y = run_mono(&mut lim, &x);
        // The sine is periodic over four samples, so its amplitude is the
        // true peak.
        let amp = tone_amp(&y[24_000..], SR / 4.0);
        assert!(
            amp <= db_to_gain(-1.0) * 1.012,
            "true peak {:.2} dBFS over a -1 dBFS ceiling",
            20.0 * amp.log10()
        );
        assert!(amp > db_to_gain(-1.5), "over-limited to {amp}");
    }

    #[test]
    fn six_db_of_reduction_on_bass_adds_no_distortion() {
        for release in [30.0f32, 100.0, 400.0] {
            let mut lim = Limiter::new();
            lim.set_param(ID_CEILING, -1.0);
            lim.set_param(ID_RELEASE, release);
            lim.prepare(SR, 256);
            let x: Vec<f32> = (0..96_000)
                .map(|i| 2.0 * (std::f32::consts::TAU * 50.0 * i as f32 / SR).sin())
                .collect();
            let y = run_mono(&mut lim, &x);
            let seg = &y[48_000..];
            let fund = tone_amp(seg, 50.0);
            let third = tone_amp(seg, 150.0);
            let second = tone_amp(seg, 100.0);
            let thd = 20.0 * ((second * second + third * third).sqrt() / fund).log10();
            assert!(thd < -70.0, "release {release}: harmonics at {thd} dB");
            assert!((20.0 * fund.log10() + 1.0).abs() < 0.1, "level {fund}");
        }
    }

    #[test]
    fn an_isolated_peak_recovers_and_quiet_material_is_untouched() {
        let mut lim = Limiter::new();
        lim.set_param(ID_CEILING, -1.0);
        lim.set_param(ID_RELEASE, 100.0);
        lim.prepare(SR, 256);
        // A quiet 1 kHz tone with one loud click in the middle.
        let mut x: Vec<f32> = (0..96_000)
            .map(|i| 0.1 * (std::f32::consts::TAU * 1_000.0 * i as f32 / SR).sin())
            .collect();
        for v in &mut x[20_000..20_010] {
            *v = 1.5;
        }
        let y = run_mono(&mut lim, &x);
        let lat = lim.latency() as usize;
        let level = |a: usize| tone_amp(&y[a + lat..a + lat + 4_800], 1_000.0);
        assert!((level(5_000) - 0.1).abs() < 1e-3);
        assert!(level(30_000) < 0.098, "no reduction right after the click");
        assert!(
            level(80_000) > 0.0995,
            "gain did not recover: {}",
            level(80_000)
        );
    }
}
