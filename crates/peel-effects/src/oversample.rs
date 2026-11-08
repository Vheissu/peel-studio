//! Linear-phase half-band oversampling (2x and 4x) for nonlinear effects.
//!
//! A half-band FIR low-pass is designed with a Kaiser window: it is exactly
//! zero at every even offset from the center tap except the center itself,
//! passes `0..0.42 fs` flat and rejects everything above `0.58 fs` by 75 dB
//! or more. One filter serves both directions of a 2x stage:
//!
//! * [`Interpolator2`] zero-stuffs and filters (gain 2), computed as two
//!   polyphase branches so no multiplies are wasted on zeros.
//! * [`Decimator2`] filters and drops every second sample.
//!
//! Chaining two stages gives 4x. Latency is the center tap index of each
//! filter, see [`stage_latency`]: for a 2x stage pair it is `c` samples at the
//! base rate, for the inner stage of 4x it is `c_b / 2` more.

use peel_dsp::delay::DelayLine;

/// Kaiser window shape for about 80 dB of stopband rejection.
const KAISER_BETA: f64 = 8.0;

/// Modified Bessel function of the first kind, order zero.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x * 0.25;
    for k in 1..64 {
        term *= q / (f64::from(k) * f64::from(k));
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Designs a half-band low-pass of `taps` coefficients (odd), returning the
/// nonzero entries as `(index, value)`. DC gain is 1.
fn design_halfband(taps: usize) -> Vec<(usize, f32)> {
    let c = (taps - 1) / 2;
    let i0_beta = bessel_i0(KAISER_BETA);
    let mut h: Vec<f64> = (0..taps)
        .map(|k| {
            let d = k as f64 - c as f64;
            // Ideal half-band: sin(pi d / 2) / (pi d), 0.5 at the center.
            let ideal = if k == c {
                0.5
            } else {
                (std::f64::consts::FRAC_PI_2 * d).sin() / (std::f64::consts::PI * d)
            };
            let r = d / c.max(1) as f64;
            let w = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / i0_beta;
            ideal * w
        })
        .collect();
    // Exact zeros at even offsets, then normalize DC gain to 1.
    for (k, v) in h.iter_mut().enumerate() {
        if k != c && (k as isize - c as isize) % 2 == 0 {
            *v = 0.0;
        }
    }
    let sum: f64 = h.iter().sum();
    h.iter()
        .enumerate()
        .filter(|(_, v)| v.abs() > 1e-12)
        .map(|(k, v)| (k, (v / sum) as f32))
        .collect()
}

/// Center tap index of a half-band stage with `taps` coefficients.
pub(crate) const fn stage_latency(taps: usize) -> usize {
    (taps - 1) / 2
}

/// Zero-stuffing 2x upsampler with the half-band filter folded in.
pub(crate) struct Interpolator2 {
    even: Vec<(usize, f32)>,
    odd: Vec<(usize, f32)>,
    hist: DelayLine,
}

impl Interpolator2 {
    pub fn new(taps: usize) -> Interpolator2 {
        let h = design_halfband(taps);
        // y[2m] uses even k at x[m - k/2]; y[2m+1] uses odd k at x[m - (k-1)/2].
        // The filter has gain 2 here, which compensates the zero-stuffing.
        let even = h
            .iter()
            .filter(|(k, _)| k % 2 == 0)
            .map(|(k, v)| (k / 2, v * 2.0))
            .collect();
        let odd = h
            .iter()
            .filter(|(k, _)| k % 2 == 1)
            .map(|(k, v)| ((k - 1) / 2, v * 2.0))
            .collect();
        Interpolator2 {
            even,
            odd,
            hist: DelayLine::new(taps / 2 + 4),
        }
    }

    pub fn clear(&mut self) {
        self.hist.clear();
    }

    /// Takes one input sample, returns the two output samples in time order.
    #[inline]
    pub fn process(&mut self, x: f32) -> (f32, f32) {
        self.hist.push(x);
        // After the push, read(1) is x[m], read(1 + j) is x[m - j].
        let a: f32 = self
            .even
            .iter()
            .map(|(j, v)| v * self.hist.read(1 + j))
            .sum();
        let b: f32 = self
            .odd
            .iter()
            .map(|(j, v)| v * self.hist.read(1 + j))
            .sum();
        (a, b)
    }
}

/// Half-band low-pass followed by dropping every second sample.
pub(crate) struct Decimator2 {
    taps: Vec<(usize, f32)>,
    hist: DelayLine,
}

impl Decimator2 {
    pub fn new(taps: usize) -> Decimator2 {
        Decimator2 {
            taps: design_halfband(taps),
            hist: DelayLine::new(taps + 4),
        }
    }

    pub fn clear(&mut self) {
        self.hist.clear();
    }

    /// Takes two input samples in time order, returns one output sample.
    #[inline]
    pub fn process(&mut self, first: f32, second: f32) -> f32 {
        self.hist.push(first);
        // Newest sample is `first` (read(1)); read(1 + k) is k samples older.
        let y: f32 = self
            .taps
            .iter()
            .map(|(k, v)| v * self.hist.read(1 + k))
            .sum();
        self.hist.push(second);
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Accurate sine sample: the phase is reduced in f64 so long test
    /// signals do not pick up f32 phase noise.
    fn sin_at(freq: f64, rate: f64, i: usize) -> f32 {
        let cycles = (i as f64 * freq / rate).fract();
        (std::f64::consts::TAU * cycles).sin() as f32
    }

    #[test]
    fn halfband_has_expected_structure() {
        let h = design_halfband(63);
        let sum: f32 = h.iter().map(|(_, v)| v).sum();
        assert!((sum - 1.0).abs() < 1e-5);
        let c = 31;
        for (k, v) in &h {
            if *k != c {
                assert_eq!((*k as isize - c as isize).rem_euclid(2), 1, "tap {k}");
            } else {
                assert!((v - 0.5).abs() < 0.01);
            }
        }
        // Symmetric.
        for (k, v) in &h {
            let mirror = h.iter().find(|(m, _)| *m == 62 - k).unwrap().1;
            assert!((v - mirror).abs() < 1e-7);
        }
    }

    fn roundtrip_gain_db(freq: f32, sr: f32, four_x: bool) -> f32 {
        let mut up_a = Interpolator2::new(63);
        let mut dn_a = Decimator2::new(63);
        let mut up_b = Interpolator2::new(21);
        let mut dn_b = Decimator2::new(21);
        let n = 8192;
        let mut energy = 0.0f64;
        let mut count = 0usize;
        for i in 0..n {
            let x = sin_at(f64::from(freq), f64::from(sr), i);
            let (u0, u1) = up_a.process(x);
            let y = if four_x {
                let (v0, v1) = up_b.process(u0);
                let (w0, w1) = up_b.process(u1);
                let z0 = dn_b.process(v0, v1);
                let z1 = dn_b.process(w0, w1);
                dn_a.process(z0, z1)
            } else {
                dn_a.process(u0, u1)
            };
            if i > n / 2 {
                energy += f64::from(y) * f64::from(y);
                count += 1;
            }
        }
        // RMS of a unit sine is 1/sqrt(2).
        (10.0 * (2.0 * energy / count as f64).log10()) as f32
    }

    #[test]
    fn roundtrip_is_flat_in_the_passband() {
        for four_x in [false, true] {
            for f in [100.0, 1_000.0, 8_000.0, 16_000.0, 19_500.0] {
                let g = roundtrip_gain_db(f, 48_000.0, four_x);
                assert!(g.abs() < 0.15, "{f} Hz, four_x {four_x}: {g} dB");
            }
        }
    }

    #[test]
    fn roundtrip_latency_matches_center_taps() {
        let mut up_a = Interpolator2::new(63);
        let mut dn_a = Decimator2::new(63);
        let mut up_b = Interpolator2::new(21);
        let mut dn_b = Decimator2::new(21);
        // 2x
        let mut out2 = Vec::new();
        // 4x
        let mut out4 = Vec::new();
        for i in 0..200 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (u0, u1) = up_a.process(x);
            out2.push(dn_a.process(u0, u1));
        }
        up_a.clear();
        dn_a.clear();
        for i in 0..200 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (u0, u1) = up_a.process(x);
            let (v0, v1) = up_b.process(u0);
            let (w0, w1) = up_b.process(u1);
            let z0 = dn_b.process(v0, v1);
            let z1 = dn_b.process(w0, w1);
            out4.push(dn_a.process(z0, z1));
        }
        let peak_at = |v: &[f32]| {
            v.iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap()
                .0
        };
        assert_eq!(peak_at(&out2), stage_latency(63));
        assert_eq!(peak_at(&out4), stage_latency(63) + stage_latency(21) / 2);
    }

    #[test]
    fn out_of_band_content_is_rejected() {
        // A 30 kHz tone at a 96 kHz rate must vanish after decimation to 48.
        let hi = 96_000.0;
        let mut dn = Decimator2::new(63);
        let mut peak = 0.0f32;
        for m in 0..4000 {
            let a = sin_at(30_000.0, hi, 2 * m);
            let b = sin_at(30_000.0, hi, 2 * m + 1);
            let y = dn.process(a, b);
            if m > 500 {
                peak = peak.max(y.abs());
            }
        }
        assert!(20.0 * peak.log10() < -75.0, "{}", 20.0 * peak.log10());
    }
}
