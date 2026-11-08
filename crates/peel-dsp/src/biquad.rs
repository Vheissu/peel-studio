//! Biquad filters and simple one-pole filters.
//!
//! Coefficients follow the RBJ "Audio EQ Cookbook". Filters run in transposed
//! direct form II with `f64` state, which keeps low-frequency sections (a 20 Hz
//! high-pass at 48 kHz) clean. Audio in and out is `f32`.
//!
//! Coefficient smoothing: [`SmoothCoeffs`] and [`StereoBiquad`] ramp linearly
//! between coefficient sets. The stability region of a biquad denominator is a
//! convex triangle in `(a1, a2)`, so interpolating between two stable sets can
//! never leave it; automation cannot make the filter blow up.
//!
//! Realtime rules: nothing here allocates.

use std::f64::consts::PI;

/// Filter response family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    LowPass,
    HighPass,
    /// Band-pass with 0 dB gain at the center frequency.
    BandPass,
    Notch,
    /// Peaking bell. `gain_db` sets the center gain.
    Peak,
    LowShelf,
    HighShelf,
    AllPass,
}

/// Normalized biquad coefficients (`a0 == 1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coeffs {
    /// Passes the signal through unchanged.
    pub const IDENTITY: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// Designs an RBJ biquad. `freq` is clamped to `1 Hz..0.49 * sample_rate`
    /// and `q` to at least 0.02. `gain_db` only affects peak and shelf kinds.
    pub fn design(kind: FilterKind, sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Coeffs {
        let sr = f64::from(sample_rate.max(1.0));
        let f = f64::from(freq.max(1.0)).min(sr * 0.49);
        let q = f64::from(q).max(0.02);
        let gain_db = f64::from(gain_db).clamp(-60.0, 60.0);
        let w0 = 2.0 * PI * f / sr;
        let (sinw, cosw) = w0.sin_cos();
        let alpha = sinw / (2.0 * q);
        let a = 10f64.powf(gain_db / 40.0);

        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterKind::LowPass => (
                (1.0 - cosw) * 0.5,
                1.0 - cosw,
                (1.0 - cosw) * 0.5,
                1.0 + alpha,
                -2.0 * cosw,
                1.0 - alpha,
            ),
            FilterKind::HighPass => (
                (1.0 + cosw) * 0.5,
                -(1.0 + cosw),
                (1.0 + cosw) * 0.5,
                1.0 + alpha,
                -2.0 * cosw,
                1.0 - alpha,
            ),
            FilterKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cosw, 1.0 - alpha),
            FilterKind::Notch => (1.0, -2.0 * cosw, 1.0, 1.0 + alpha, -2.0 * cosw, 1.0 - alpha),
            FilterKind::AllPass => (
                1.0 - alpha,
                -2.0 * cosw,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cosw,
                1.0 - alpha,
            ),
            FilterKind::Peak => (
                1.0 + alpha * a,
                -2.0 * cosw,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cosw,
                1.0 - alpha / a,
            ),
            FilterKind::LowShelf => {
                let t = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cosw + t),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cosw),
                    a * ((a + 1.0) - (a - 1.0) * cosw - t),
                    (a + 1.0) + (a - 1.0) * cosw + t,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cosw),
                    (a + 1.0) + (a - 1.0) * cosw - t,
                )
            }
            FilterKind::HighShelf => {
                let t = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cosw + t),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cosw),
                    a * ((a + 1.0) + (a - 1.0) * cosw - t),
                    (a + 1.0) - (a - 1.0) * cosw + t,
                    2.0 * ((a - 1.0) - (a + 1.0) * cosw),
                    (a + 1.0) - (a - 1.0) * cosw - t,
                )
            }
        };
        Coeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// First-order (6 dB/oct) low-pass via the bilinear transform.
    pub fn first_order_lowpass(sample_rate: f32, freq: f32) -> Coeffs {
        let k = Self::prewarp(sample_rate, freq);
        let n = 1.0 / (1.0 + k);
        Coeffs {
            b0: k * n,
            b1: k * n,
            b2: 0.0,
            a1: (k - 1.0) * n,
            a2: 0.0,
        }
    }

    /// First-order (6 dB/oct) high-pass via the bilinear transform.
    pub fn first_order_highpass(sample_rate: f32, freq: f32) -> Coeffs {
        let k = Self::prewarp(sample_rate, freq);
        let n = 1.0 / (1.0 + k);
        Coeffs {
            b0: n,
            b1: -n,
            b2: 0.0,
            a1: (k - 1.0) * n,
            a2: 0.0,
        }
    }

    fn prewarp(sample_rate: f32, freq: f32) -> f64 {
        let sr = f64::from(sample_rate.max(1.0));
        let f = f64::from(freq.max(1.0)).min(sr * 0.49);
        (PI * f / sr).tan()
    }

    /// Complex frequency response `(re, im)` at `freq` Hz.
    fn response(&self, sample_rate: f32, freq: f32) -> (f64, f64) {
        let w = 2.0 * PI * f64::from(freq) / f64::from(sample_rate.max(1.0));
        let (s1, c1) = (-w).sin_cos();
        let (s2, c2) = (-2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = self.b1 * s1 + self.b2 * s2;
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = self.a1 * s1 + self.a2 * s2;
        let den = dr * dr + di * di;
        if den <= 1e-300 {
            return (0.0, 0.0);
        }
        ((nr * dr + ni * di) / den, (ni * dr - nr * di) / den)
    }

    /// Linear magnitude of the response at `freq` Hz.
    pub fn magnitude(&self, sample_rate: f32, freq: f32) -> f32 {
        let (re, im) = self.response(sample_rate, freq);
        (re * re + im * im).sqrt() as f32
    }

    /// Magnitude of the response at `freq` Hz in decibels (floored at -144).
    pub fn magnitude_db(&self, sample_rate: f32, freq: f32) -> f32 {
        let m = self.magnitude(sample_rate, freq);
        if m <= 1e-7 { -144.0 } else { 20.0 * m.log10() }
    }

    /// True when both sets are bit-identical to [`Coeffs::IDENTITY`].
    pub fn is_identity(&self) -> bool {
        *self == Coeffs::IDENTITY
    }
}

/// Number of sections needed for a Butterworth filter of `order` (1..=8):
/// `order / 2` biquads plus one first-order section for odd orders.
pub fn butterworth_section_count(order: u32) -> usize {
    let order = order.clamp(1, 8) as usize;
    order.div_ceil(2)
}

/// Writes the cascade of sections for a Butterworth low-pass or high-pass of
/// `order` (1..=8) into `out` and returns how many sections are used (at most
/// four). Each section is a valid [`Coeffs`]; multiply their responses.
pub fn butterworth(
    highpass: bool,
    order: u32,
    sample_rate: f32,
    freq: f32,
    out: &mut [Coeffs; 4],
) -> usize {
    let order = order.clamp(1, 8);
    let biquads = (order / 2) as usize;
    let kind = if highpass {
        FilterKind::HighPass
    } else {
        FilterKind::LowPass
    };
    for (k, slot) in out.iter_mut().enumerate().take(biquads) {
        // Pole pair k: Q = 1 / (2 sin((2k+1) pi / (2 order))).
        let theta = (2 * k + 1) as f64 * PI / (2.0 * f64::from(order));
        let q = 1.0 / (2.0 * theta.sin());
        *slot = Coeffs::design(kind, sample_rate, freq, q as f32, 0.0);
    }
    let mut used = biquads;
    if order % 2 == 1 {
        out[used] = if highpass {
            Coeffs::first_order_highpass(sample_rate, freq)
        } else {
            Coeffs::first_order_lowpass(sample_rate, freq)
        };
        used += 1;
    }
    used
}

#[inline]
fn flush(x: f64) -> f64 {
    if x.abs() < 1e-30 { 0.0 } else { x }
}

/// Mono biquad, transposed direct form II, `f64` state.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    pub coeffs: Coeffs,
    z1: f64,
    z2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Biquad::new(Coeffs::IDENTITY)
    }
}

impl Biquad {
    pub fn new(coeffs: Coeffs) -> Biquad {
        Biquad {
            coeffs,
            z1: 0.0,
            z2: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let c = &self.coeffs;
        let x = f64::from(x);
        let y = c.b0 * x + self.z1;
        self.z1 = flush(c.b1 * x - c.a1 * y + self.z2);
        self.z2 = flush(c.b2 * x - c.a2 * y);
        if y.is_finite() {
            y as f32
        } else {
            self.reset();
            0.0
        }
    }

    pub fn process_block(&mut self, buf: &mut [f32]) {
        for s in buf {
            *s = self.process(*s);
        }
    }
}

/// Linearly ramps a [`Coeffs`] set toward a target.
#[derive(Clone, Copy, Debug)]
pub struct SmoothCoeffs {
    current: Coeffs,
    target: Coeffs,
    step: [f64; 5],
    remaining: u32,
    ramp: u32,
}

impl SmoothCoeffs {
    pub fn new(coeffs: Coeffs) -> SmoothCoeffs {
        SmoothCoeffs {
            current: coeffs,
            target: coeffs,
            step: [0.0; 5],
            remaining: 0,
            ramp: 64,
        }
    }

    /// Ramp length in samples (at least 1).
    pub fn set_ramp_samples(&mut self, samples: u32) {
        self.ramp = samples.max(1);
    }

    /// Ramp length derived from a duration.
    pub fn set_ramp_time(&mut self, sample_rate: f32, seconds: f32) {
        self.set_ramp_samples((sample_rate * seconds) as u32);
    }

    pub fn set_target(&mut self, target: Coeffs) {
        if target == self.target {
            return;
        }
        self.target = target;
        let n = f64::from(self.ramp);
        let c = &self.current;
        self.step = [
            (target.b0 - c.b0) / n,
            (target.b1 - c.b1) / n,
            (target.b2 - c.b2) / n,
            (target.a1 - c.a1) / n,
            (target.a2 - c.a2) / n,
        ];
        self.remaining = self.ramp;
    }

    pub fn set_immediate(&mut self, coeffs: Coeffs) {
        self.current = coeffs;
        self.target = coeffs;
        self.remaining = 0;
    }

    /// Advances one sample and returns the coefficients to use for it.
    #[inline]
    pub fn next(&mut self) -> Coeffs {
        if self.remaining > 0 {
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            } else {
                let s = &self.step;
                let c = &mut self.current;
                c.b0 += s[0];
                c.b1 += s[1];
                c.b2 += s[2];
                c.a1 += s[3];
                c.a2 += s[4];
            }
        }
        self.current
    }

    pub fn is_ramping(&self) -> bool {
        self.remaining > 0
    }

    pub fn current(&self) -> Coeffs {
        self.current
    }

    pub fn target(&self) -> Coeffs {
        self.target
    }
}

/// Stereo biquad sharing one smoothed coefficient set across both channels.
/// Skips all work while the coefficients are settled at identity.
#[derive(Clone, Copy, Debug)]
pub struct StereoBiquad {
    coeffs: SmoothCoeffs,
    zl: [f64; 2],
    zr: [f64; 2],
}

impl Default for StereoBiquad {
    fn default() -> Self {
        StereoBiquad::new(Coeffs::IDENTITY)
    }
}

impl StereoBiquad {
    pub fn new(coeffs: Coeffs) -> StereoBiquad {
        StereoBiquad {
            coeffs: SmoothCoeffs::new(coeffs),
            zl: [0.0; 2],
            zr: [0.0; 2],
        }
    }

    pub fn set_ramp_time(&mut self, sample_rate: f32, seconds: f32) {
        self.coeffs.set_ramp_time(sample_rate, seconds);
    }

    pub fn set_ramp_samples(&mut self, samples: u32) {
        self.coeffs.set_ramp_samples(samples);
    }

    /// Moves toward new coefficients over the ramp (safe under automation).
    pub fn set_target(&mut self, coeffs: Coeffs) {
        self.coeffs.set_target(coeffs);
    }

    /// Jumps to new coefficients without ramping (state is kept).
    pub fn set_immediate(&mut self, coeffs: Coeffs) {
        self.coeffs.set_immediate(coeffs);
    }

    pub fn reset(&mut self) {
        self.zl = [0.0; 2];
        self.zr = [0.0; 2];
    }

    /// True while the coefficients are still moving toward a new target.
    pub fn is_ramping(&self) -> bool {
        self.coeffs.is_ramping()
    }

    /// True when the filter is settled at identity and does nothing.
    pub fn is_bypassed(&self) -> bool {
        !self.coeffs.is_ramping() && self.coeffs.current().is_identity()
    }

    pub fn target(&self) -> Coeffs {
        self.coeffs.target()
    }

    #[inline]
    fn tick(z: &mut [f64; 2], c: &Coeffs, x: f32) -> f32 {
        let x = f64::from(x);
        let y = c.b0 * x + z[0];
        z[0] = flush(c.b1 * x - c.a1 * y + z[1]);
        z[1] = flush(c.b2 * x - c.a2 * y);
        if y.is_finite() {
            y as f32
        } else {
            *z = [0.0; 2];
            0.0
        }
    }

    /// Processes one stereo frame.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.is_bypassed() {
            self.reset();
            return (l, r);
        }
        let c = self.coeffs.next();
        (
            Self::tick(&mut self.zl, &c, l),
            Self::tick(&mut self.zr, &c, r),
        )
    }

    /// Processes equal-length buffers in place.
    pub fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.is_bypassed() {
            self.reset();
            return;
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (a, b) = self.process(*l, *r);
            *l = a;
            *r = b;
        }
    }
}

/// One-pole low-pass (6 dB/oct) with unity DC gain.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePoleLowpass {
    a: f32,
    z: f32,
}

impl OnePoleLowpass {
    pub fn new(sample_rate: f32, freq: f32) -> OnePoleLowpass {
        let mut f = OnePoleLowpass::default();
        f.set_cutoff(sample_rate, freq);
        f
    }

    pub fn set_cutoff(&mut self, sample_rate: f32, freq: f32) {
        let f = freq.clamp(1.0, sample_rate.max(2.0) * 0.49);
        self.a = 1.0 - (-std::f32::consts::TAU * f / sample_rate.max(1.0)).exp();
    }

    pub fn reset(&mut self) {
        self.z = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.z += self.a * (x - self.z);
        if !(self.z.abs() > 1e-30 && self.z.is_finite()) {
            self.z = 0.0;
        }
        self.z
    }
}

/// One-pole high-pass (6 dB/oct), built as input minus a low-pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePoleHighpass {
    lp: OnePoleLowpass,
}

impl OnePoleHighpass {
    pub fn new(sample_rate: f32, freq: f32) -> OnePoleHighpass {
        OnePoleHighpass {
            lp: OnePoleLowpass::new(sample_rate, freq),
        }
    }

    pub fn set_cutoff(&mut self, sample_rate: f32, freq: f32) {
        self.lp.set_cutoff(sample_rate, freq);
    }

    pub fn reset(&mut self) {
        self.lp.reset();
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        x - self.lp.process(x)
    }
}

/// DC blocker: `y[n] = x[n] - x[n-1] + R y[n-1]`.
#[derive(Clone, Copy, Debug)]
pub struct DcBlocker {
    r: f64,
    x1: f64,
    y1: f64,
}

impl DcBlocker {
    /// `cutoff_hz` of about 5 to 20 Hz is typical.
    pub fn new(sample_rate: f32, cutoff_hz: f32) -> DcBlocker {
        let r = (1.0 - 2.0 * PI * f64::from(cutoff_hz) / f64::from(sample_rate.max(1.0)))
            .clamp(0.9, 0.999_99);
        DcBlocker {
            r,
            x1: 0.0,
            y1: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let y = flush(x - self.x1 + self.r * self.y1);
        self.x1 = x;
        self.y1 = y;
        if y.is_finite() {
            y as f32
        } else {
            self.reset();
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn run_sine(c: Coeffs, freq: f32) -> f32 {
        let mut f = Biquad::new(c);
        let n = 8192;
        let mut peak = 0.0f32;
        for i in 0..n {
            let x = (std::f32::consts::TAU * freq * i as f32 / SR).sin();
            let y = f.process(x);
            if i > n / 2 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn peak_gain_at_center() {
        for gain in [-12.0f32, -3.0, 6.0, 15.0] {
            let c = Coeffs::design(FilterKind::Peak, SR, 1000.0, 1.0, gain);
            assert!((c.magnitude_db(SR, 1000.0) - gain).abs() < 0.01, "{gain}");
            // Far away the bell is flat.
            assert!(c.magnitude_db(SR, 30.0).abs() < 0.3);
        }
    }

    #[test]
    fn shelves_reach_their_gain() {
        let lo = Coeffs::design(FilterKind::LowShelf, SR, 200.0, 0.707, 9.0);
        assert!((lo.magnitude_db(SR, 20.0) - 9.0).abs() < 0.1);
        assert!(lo.magnitude_db(SR, 10_000.0).abs() < 0.1);
        let hi = Coeffs::design(FilterKind::HighShelf, SR, 4000.0, 0.707, -9.0);
        assert!((hi.magnitude_db(SR, 20_000.0) + 9.0).abs() < 0.2);
        assert!(hi.magnitude_db(SR, 100.0).abs() < 0.1);
    }

    #[test]
    fn lowpass_highpass_corners() {
        let lp = Coeffs::design(
            FilterKind::LowPass,
            SR,
            2000.0,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        );
        assert!((lp.magnitude_db(SR, 2000.0) + 3.01).abs() < 0.05);
        assert!(lp.magnitude_db(SR, 100.0).abs() < 0.01);
        let hp = Coeffs::design(
            FilterKind::HighPass,
            SR,
            2000.0,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        );
        assert!((hp.magnitude_db(SR, 2000.0) + 3.01).abs() < 0.05);
        assert!(hp.magnitude_db(SR, 15_000.0).abs() < 0.2);
        let bp = Coeffs::design(FilterKind::BandPass, SR, 1000.0, 2.0, 0.0);
        assert!(bp.magnitude_db(SR, 1000.0).abs() < 0.01);
        let notch = Coeffs::design(FilterKind::Notch, SR, 1000.0, 2.0, 0.0);
        assert!(notch.magnitude_db(SR, 1000.0) < -60.0);
        let ap = Coeffs::design(FilterKind::AllPass, SR, 1000.0, 1.0, 0.0);
        for f in [50.0, 1000.0, 9000.0] {
            assert!(ap.magnitude_db(SR, f).abs() < 0.001);
        }
    }

    #[test]
    fn processing_matches_analytic_response() {
        let c = Coeffs::design(FilterKind::Peak, SR, 3000.0, 2.0, 12.0);
        let measured = run_sine(c, 3000.0);
        let expected = c.magnitude(SR, 3000.0);
        assert!((measured - expected).abs() / expected < 0.01);
    }

    #[test]
    fn first_order_corner_is_minus_three_db() {
        let lp = Coeffs::first_order_lowpass(SR, 1000.0);
        assert!((lp.magnitude_db(SR, 1000.0) + 3.01).abs() < 0.02);
        let hp = Coeffs::first_order_highpass(SR, 1000.0);
        assert!((hp.magnitude_db(SR, 1000.0) + 3.01).abs() < 0.02);
    }

    #[test]
    fn butterworth_cascades() {
        let mut out = [Coeffs::IDENTITY; 4];
        for order in 1..=8u32 {
            for hp in [false, true] {
                let n = butterworth(hp, order, SR, 1000.0, &mut out);
                assert_eq!(n, butterworth_section_count(order));
                let total =
                    |f: f32| -> f32 { out[..n].iter().map(|c| c.magnitude_db(SR, f)).sum::<f32>() };
                assert!((total(1000.0) + 3.01).abs() < 0.1, "order {order} hp {hp}");
                // Roll-off one octave into the stopband: ~6 dB per order.
                let f_stop = if hp { 500.0 } else { 2000.0 };
                let expected = -10.0 * (1.0 + 2f32.powi(2 * order as i32)).log10();
                assert!(
                    (total(f_stop) - expected).abs() < 1.5,
                    "order {order} hp {hp}: {} vs {expected}",
                    total(f_stop)
                );
            }
        }
    }

    #[test]
    fn interpolated_coeffs_stay_stable() {
        // Randomly retarget a very resonant low-frequency filter every few
        // samples; output must stay bounded.
        let mut f = StereoBiquad::default();
        f.set_ramp_samples(32);
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let kinds = [
            FilterKind::LowPass,
            FilterKind::HighPass,
            FilterKind::Peak,
            FilterKind::LowShelf,
            FilterKind::HighShelf,
            FilterKind::BandPass,
        ];
        let mut peak = 0.0f32;
        for i in 0..200_000 {
            if i % 37 == 0 {
                let kind = kinds[(rnd() * kinds.len() as f32) as usize % kinds.len()];
                let freq = 20.0 * 1000f32.powf(rnd());
                let q = 0.1 + rnd() * 17.0;
                let gain = (rnd() - 0.5) * 48.0;
                f.set_target(Coeffs::design(kind, SR, freq, q, gain));
            }
            let x = rnd() * 2.0 - 1.0;
            let (l, r) = f.process(x, -x);
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak < 1000.0, "peak {peak}");
    }

    #[test]
    fn stereo_biquad_bypasses_identity() {
        let mut f = StereoBiquad::default();
        assert!(f.is_bypassed());
        let (l, r) = f.process(0.25, -0.5);
        assert_eq!((l, r), (0.25, -0.5));
        f.set_target(Coeffs::design(FilterKind::LowPass, SR, 500.0, 0.7, 0.0));
        assert!(!f.is_bypassed());
        for _ in 0..200 {
            f.process(1.0, 1.0);
        }
        f.set_target(Coeffs::IDENTITY);
        for _ in 0..200 {
            f.process(0.0, 0.0);
        }
        assert!(f.is_bypassed());
    }

    #[test]
    fn dc_blocker_removes_offset() {
        let mut d = DcBlocker::new(SR, 10.0);
        let mut y = 0.0;
        for _ in 0..48_000 {
            y = d.process(0.5);
        }
        assert!(y.abs() < 1e-3);
    }

    #[test]
    fn one_pole_filters() {
        let mut lp = OnePoleLowpass::new(SR, 100.0);
        let mut y = 0.0;
        for _ in 0..48_000 {
            y = lp.process(1.0);
        }
        assert!((y - 1.0).abs() < 1e-3);
        let mut hp = OnePoleHighpass::new(SR, 100.0);
        for _ in 0..48_000 {
            y = hp.process(1.0);
        }
        assert!(y.abs() < 1e-3);
    }

    #[test]
    fn non_finite_input_does_not_poison_state() {
        let mut f = Biquad::new(Coeffs::design(FilterKind::LowPass, SR, 1000.0, 1.0, 0.0));
        let _ = f.process(f32::NAN);
        for _ in 0..16 {
            assert!(f.process(0.1).is_finite());
        }
    }
}
