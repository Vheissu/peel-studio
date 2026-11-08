//! Delay lines with fractional reads, plus the small modulation helpers that
//! go with them (LFO, time glide).
//!
//! A [`DelayLine`] is allocated once for a maximum delay and never grows.
//! Reads clamp to the valid range, so no parameter value can index out of
//! bounds. Realtime rules: only [`DelayLine::new`] allocates.

/// Fractional-read quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interp {
    /// Two-point linear. Cheap, low-passes slightly.
    Linear,
    /// Four-point, third-order Hermite. Good default for modulated delays.
    Hermite,
}

/// Circular buffer with integer and fractional reads.
///
/// Use it as read-then-push inside a feedback loop:
///
/// ```
/// use peel_dsp::delay::DelayLine;
/// let mut d = DelayLine::new(100);
/// let out = d.read(10); // x[n - 10]
/// d.push(0.5);          // now write x[n]
/// # let _ = out;
/// ```
///
/// `read(d)` returns the sample pushed `d` pushes ago, so `read(1)` is the
/// most recent push and an impulse pushed at time `t` comes back at `t + d`.
#[derive(Clone, Debug)]
pub struct DelayLine {
    buf: Vec<f32>,
    mask: usize,
    write: usize,
    max_delay: usize,
}

impl DelayLine {
    /// Allocates a line that can delay by up to `max_delay` samples.
    pub fn new(max_delay: usize) -> DelayLine {
        let cap = (max_delay + 4).next_power_of_two().max(8);
        DelayLine {
            buf: vec![0.0; cap],
            mask: cap - 1,
            write: 0,
            max_delay,
        }
    }

    /// Largest delay in samples this line can produce.
    pub fn max_delay(&self) -> usize {
        self.max_delay
    }

    /// Silences the buffer without allocating.
    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
    }

    /// Appends one sample.
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.buf[self.write] = x;
        self.write = (self.write + 1) & self.mask;
    }

    /// Sample pushed `d` pushes ago (`d` is clamped to `1..=max_delay`).
    #[inline]
    pub fn read(&self, d: usize) -> f32 {
        let d = d.clamp(1, self.max_delay);
        self.buf[self.write.wrapping_sub(d) & self.mask]
    }

    /// Linear interpolation at a fractional delay (clamped to `1..=max`).
    #[inline]
    pub fn read_linear(&self, d: f32) -> f32 {
        let d = if d.is_finite() { d } else { 1.0 };
        let d = d.clamp(1.0, (self.max_delay - 1) as f32);
        let i = d as usize;
        let f = d - i as f32;
        let a = self.buf[self.write.wrapping_sub(i) & self.mask];
        let b = self.buf[self.write.wrapping_sub(i + 1) & self.mask];
        a + (b - a) * f
    }

    /// Cubic Hermite interpolation at a fractional delay (clamped to
    /// `2..=max_delay - 1`).
    #[inline]
    pub fn read_hermite(&self, d: f32) -> f32 {
        let d = if d.is_finite() { d } else { 2.0 };
        let d = d.clamp(2.0, (self.max_delay.max(4) - 1) as f32);
        let i = d as usize;
        let f = d - i as f32;
        // Time-ordered points p0 (oldest) .. p3 (newest); the target lies
        // between p1 and p2, `t` of the way from p1 toward p2.
        let p3 = self.buf[self.write.wrapping_sub(i - 1) & self.mask];
        let p2 = self.buf[self.write.wrapping_sub(i) & self.mask];
        let p1 = self.buf[self.write.wrapping_sub(i + 1) & self.mask];
        let p0 = self.buf[self.write.wrapping_sub(i + 2) & self.mask];
        let t = 1.0 - f;
        let c1 = 0.5 * (p2 - p0);
        let c2 = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
        let c3 = 0.5 * (p3 - p0) + 1.5 * (p1 - p2);
        ((c3 * t + c2) * t + c1) * t + p1
    }

    /// Fractional read with a chosen interpolator.
    #[inline]
    pub fn read_interp(&self, d: f32, interp: Interp) -> f32 {
        match interp {
            Interp::Linear => self.read_linear(d),
            Interp::Hermite => self.read_hermite(d),
        }
    }
}

/// First-order all-pass interpolator for a delay tap.
///
/// Flat magnitude and very accurate at low frequencies, which makes it a
/// good fit for slowly changing delays (reverb, chorus). It has memory, so a
/// separate instance is needed per tap, and rapid delay changes cause
/// transients; use [`Interp::Hermite`] for fast modulation.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllpassTap {
    y1: f32,
}

impl AllpassTap {
    pub fn new() -> AllpassTap {
        AllpassTap::default()
    }

    pub fn reset(&mut self) {
        self.y1 = 0.0;
    }

    /// Reads `d` samples back (clamped to `2..=max_delay - 2`).
    #[inline]
    pub fn read(&mut self, line: &DelayLine, d: f32) -> f32 {
        let d = if d.is_finite() { d } else { 2.0 };
        let d = d.clamp(2.0, (line.max_delay.max(5) - 2) as f32);
        // Split into an integer part and a fraction in [0.5, 1.5) where the
        // all-pass is most accurate.
        let i = (d - 0.5) as usize;
        let frac = d - i as f32;
        let eta = (1.0 - frac) / (1.0 + frac);
        let x0 = line.read(i);
        let x1 = line.read(i + 1);
        let y = eta * (x0 - self.y1) + x1;
        self.y1 = if y.abs() > 1e-30 && y.is_finite() {
            y
        } else {
            0.0
        };
        y
    }
}

/// Parabolic sine approximation. `phase` is in turns (any finite value).
/// Maximum error is about 0.1%, which is inaudible for modulation.
#[inline]
pub fn fast_sin_turns(phase: f32) -> f32 {
    let p = phase - phase.floor();
    let mut x = p * 2.0;
    if x >= 1.0 {
        x -= 2.0;
    }
    let y = 4.0 * x * (1.0 - x.abs());
    0.225 * (y * y.abs() - y) + y
}

/// Phase-accumulating low-frequency oscillator.
#[derive(Clone, Copy, Debug)]
pub struct Lfo {
    /// Phase in turns, `0..1`.
    pub phase: f32,
    inc: f32,
}

impl Lfo {
    pub fn new(phase: f32) -> Lfo {
        Lfo {
            phase: phase - phase.floor(),
            inc: 0.0,
        }
    }

    /// Sets the rate in Hz.
    pub fn set_rate(&mut self, hz: f32, sample_rate: f32) {
        self.inc = (hz / sample_rate.max(1.0)).clamp(0.0, 0.5);
    }

    /// Sets the increment in turns per sample directly.
    pub fn set_increment(&mut self, turns_per_sample: f32) {
        self.inc = turns_per_sample.clamp(0.0, 0.5);
    }

    #[inline]
    fn advance(&mut self) {
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
    }

    /// Returns the current phase (turns) and advances by one sample. Use it
    /// to read several waveforms or offsets from one oscillator.
    #[inline]
    pub fn next_phase(&mut self) -> f32 {
        let p = self.phase;
        self.advance();
        p
    }

    /// Next sine value in `-1..=1`.
    #[inline]
    pub fn next_sine(&mut self) -> f32 {
        let v = fast_sin_turns(self.phase);
        self.advance();
        v
    }

    /// Next triangle value in `-1..=1` (starts at 0, rising).
    #[inline]
    pub fn next_triangle(&mut self) -> f32 {
        let p = self.phase;
        let v = if p < 0.25 {
            4.0 * p
        } else if p < 0.75 {
            2.0 - 4.0 * p
        } else {
            4.0 * p - 4.0
        };
        self.advance();
        v
    }

    /// Value at an explicit phase offset (turns), without advancing.
    #[inline]
    pub fn peek_sine(&self, offset: f32) -> f32 {
        fast_sin_turns(self.phase + offset)
    }
}

/// Smooths a delay time so changes bend pitch gently instead of jumping.
///
/// The time approaches its target exponentially with time constant `tau`,
/// but never faster than `max_rate` samples per sample. A rate of 0.4 bounds
/// the pitch excursion of the read head to `0.6x..1.4x` however large the
/// jump is, like a tape delay.
#[derive(Clone, Copy, Debug)]
pub struct TimeGlide {
    current: f64,
    target: f64,
    coeff: f64,
    max_rate: f64,
}

impl TimeGlide {
    pub fn new(initial: f32) -> TimeGlide {
        TimeGlide {
            current: f64::from(initial),
            target: f64::from(initial),
            coeff: 0.001,
            max_rate: 0.4,
        }
    }

    pub fn configure(&mut self, sample_rate: f32, tau_seconds: f32, max_rate: f32) {
        let tau = f64::from(tau_seconds.max(1e-4)) * f64::from(sample_rate.max(1.0));
        self.coeff = 1.0 - (-1.0 / tau).exp();
        self.max_rate = f64::from(max_rate.clamp(0.001, 4.0));
    }

    pub fn set_target(&mut self, samples: f32) {
        self.target = f64::from(samples);
    }

    pub fn set_immediate(&mut self, samples: f32) {
        self.current = f64::from(samples);
        self.target = self.current;
    }

    pub fn target(&self) -> f32 {
        self.target as f32
    }

    pub fn current(&self) -> f32 {
        self.current as f32
    }

    pub fn is_gliding(&self) -> bool {
        self.current != self.target
    }

    /// Advances one sample and returns the delay to read.
    #[inline]
    pub fn next(&mut self) -> f32 {
        let diff = self.target - self.current;
        if diff != 0.0 {
            if diff.abs() < 0.002 {
                self.current = self.target;
            } else {
                self.current += (diff * self.coeff).clamp(-self.max_rate, self.max_rate);
            }
        }
        self.current as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TAU;

    #[test]
    fn integer_delay_is_exact() {
        let mut d = DelayLine::new(64);
        let mut out = Vec::new();
        for n in 0..40 {
            out.push(d.read(10));
            d.push(if n == 3 { 1.0 } else { 0.0 });
        }
        let at: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, v)| **v != 0.0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(at, vec![13]);
    }

    #[test]
    fn wraps_and_clamps_without_panicking() {
        let mut d = DelayLine::new(16);
        for n in 0..1000 {
            d.push(n as f32);
        }
        assert_eq!(d.read(1), 999.0);
        assert_eq!(d.read(0), 999.0);
        assert_eq!(d.read(10_000), d.read(16));
        assert!(d.read_hermite(f32::NAN).is_finite());
        assert!(d.read_linear(-5.0).is_finite());
        assert!(d.read_hermite(1e9).is_finite());
        let mut ap = AllpassTap::new();
        assert!(ap.read(&d, f32::INFINITY).is_finite());
    }

    #[test]
    fn fractional_reads_track_a_sine() {
        let mut d = DelayLine::new(512);
        let freq = 0.01f32; // cycles per sample
        let delay = 100.37f32;
        let mut ap = AllpassTap::new();
        let (mut e_lin, mut e_her, mut e_all) = (0.0f32, 0.0f32, 0.0f32);
        let mut count = 0;
        for n in 0..4000 {
            let t = n as f32;
            let lin = d.read_linear(delay);
            let her = d.read_hermite(delay);
            let all = ap.read(&d, delay);
            d.push((TAU * freq * t).sin());
            if n > 1000 {
                let want = (TAU * freq * (t - delay)).sin();
                e_lin = e_lin.max((lin - want).abs());
                e_her = e_her.max((her - want).abs());
                e_all = e_all.max((all - want).abs());
                count += 1;
            }
        }
        assert!(count > 0);
        assert!(e_her < 2e-3, "hermite error {e_her}");
        assert!(e_all < 1e-2, "allpass error {e_all}");
        assert!(e_lin < 5e-3, "linear error {e_lin}");
        assert!(e_her < e_lin);
    }

    #[test]
    fn hermite_matches_integer_delay_at_integers() {
        let mut d = DelayLine::new(64);
        for n in 0..100 {
            d.push((n as f32 * 0.37).sin());
        }
        for k in 2..30usize {
            assert!((d.read_hermite(k as f32) - d.read(k)).abs() < 1e-5);
            assert!((d.read_linear(k as f32) - d.read(k)).abs() < 1e-6);
        }
    }

    #[test]
    fn clear_silences() {
        let mut d = DelayLine::new(32);
        for _ in 0..100 {
            d.push(1.0);
        }
        d.clear();
        for k in 1..=32 {
            assert_eq!(d.read(k), 0.0);
        }
    }

    #[test]
    fn fast_sine_is_accurate() {
        let mut worst = 0.0f32;
        for i in 0..1000 {
            let p = i as f32 / 1000.0;
            worst = worst.max((fast_sin_turns(p) - (TAU * p).sin()).abs());
        }
        assert!(worst < 2e-3, "{worst}");
        assert!((fast_sin_turns(-0.75) - 1.0).abs() < 2e-3);
        assert!((fast_sin_turns(3.25) - 1.0).abs() < 2e-3);
    }

    #[test]
    fn lfo_period_and_range() {
        let mut l = Lfo::new(0.0);
        l.set_rate(10.0, 1000.0);
        let mut max = -2.0f32;
        let mut min = 2.0f32;
        let mut rises = 0;
        let mut prev = l.next_sine();
        for _ in 0..1000 {
            let v = l.next_sine();
            if prev < 0.0 && v >= 0.0 {
                rises += 1;
            }
            prev = v;
            max = max.max(v);
            min = min.min(v);
        }
        assert!((9..=11).contains(&rises), "{rises}");
        assert!(max <= 1.001 && min >= -1.001);
        let mut t = Lfo::new(0.0);
        t.set_rate(10.0, 1000.0);
        let mut m = 0.0f32;
        for _ in 0..200 {
            m = m.max(t.next_triangle().abs());
        }
        assert!((m - 1.0).abs() < 0.05);
    }

    #[test]
    fn glide_is_rate_limited_and_arrives() {
        let mut g = TimeGlide::new(100.0);
        g.configure(48_000.0, 0.05, 0.4);
        g.set_target(20_000.0);
        let mut prev = g.next();
        let mut max_step = 0.0f32;
        for _ in 0..400_000 {
            let v = g.next();
            max_step = max_step.max((v - prev).abs());
            prev = v;
        }
        // f32 position near 20000 has a quantum of about 0.002.
        assert!(max_step <= 0.4 + 0.01, "{max_step}");
        assert_eq!(g.current(), 20_000.0);
        assert!(!g.is_gliding());
    }
}
