//! Instrument filters: a zero-delay-feedback state-variable filter, a
//! four-pole ladder low-pass with soft saturation, and a one-pole helper.
//!
//! Both resonant filters are topology-preserving transforms (trapezoidal
//! integration with the feedback loop solved exactly), so their cutoff is
//! accurate and they stay stable under audio-rate modulation of cutoff and
//! resonance.

use std::f32::consts::PI;

/// Smooth saturator: Pade approximation of `tanh`, exactly +/-1 beyond |3|.
#[inline]
pub fn tanh_approx(x: f32) -> f32 {
    if x >= 3.0 {
        1.0
    } else if x <= -3.0 {
        -1.0
    } else {
        let x2 = x * x;
        x * (27.0 + x2) / (27.0 + 9.0 * x2)
    }
}

/// Highest cutoff the filters accept, as a fraction of the sample rate.
const MAX_CUTOFF_RATIO: f32 = 0.45;

#[inline]
fn clamp_cutoff(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let hi = sample_rate * MAX_CUTOFF_RATIO;
    if cutoff_hz.is_finite() {
        cutoff_hz.clamp(10.0_f32.min(hi), hi)
    } else {
        1_000.0_f32.min(hi)
    }
}

/// Maps a 0..1 resonance control to the SVF quality factor (0.55 to 27).
#[inline]
pub fn resonance_to_q(res: f32) -> f32 {
    0.55 * (res.clamp(0.0, 1.0) * 3.912_023).exp()
}

/// Response selected from the state-variable filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvfMode {
    LowPass,
    BandPass,
    HighPass,
    Notch,
}

impl SvfMode {
    pub fn from_index(i: usize) -> SvfMode {
        match i {
            0 => SvfMode::LowPass,
            1 => SvfMode::BandPass,
            2 => SvfMode::HighPass,
            _ => SvfMode::Notch,
        }
    }
}

/// Simultaneous outputs of the state-variable filter.
#[derive(Clone, Copy, Debug, Default)]
pub struct SvfOut {
    pub low: f32,
    /// Band-pass whose peak gain equals the quality factor.
    pub band: f32,
    /// Band-pass normalized to unity gain at the center frequency.
    pub band_unity: f32,
    pub high: f32,
    pub notch: f32,
}

impl SvfOut {
    #[inline]
    pub fn select(&self, mode: SvfMode) -> f32 {
        match mode {
            SvfMode::LowPass => self.low,
            SvfMode::BandPass => self.band_unity,
            SvfMode::HighPass => self.high,
            SvfMode::Notch => self.notch,
        }
    }
}

/// 12 dB/octave state-variable filter (Simper's trapezoidal form).
#[derive(Clone, Copy, Debug)]
pub struct Svf {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

impl Default for Svf {
    fn default() -> Self {
        Svf::new()
    }
}

impl Svf {
    pub fn new() -> Svf {
        let mut f = Svf {
            k: 1.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            ic1: 0.0,
            ic2: 0.0,
        };
        f.set_q(1_000.0, std::f32::consts::FRAC_1_SQRT_2, 48_000.0);
        f
    }

    /// Sets cutoff and a 0..1 resonance control.
    pub fn set(&mut self, cutoff_hz: f32, resonance: f32, sample_rate: f32) {
        self.set_q(cutoff_hz, resonance_to_q(resonance), sample_rate);
    }

    /// Sets cutoff and quality factor directly.
    pub fn set_q(&mut self, cutoff_hz: f32, q: f32, sample_rate: f32) {
        let fc = clamp_cutoff(cutoff_hz, sample_rate);
        let g = (PI * fc / sample_rate).tan();
        let k = 1.0 / q.clamp(0.3, 200.0);
        self.k = k;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// Copies the coefficients (not the state) from another filter, so a
    /// stereo pair can share one coefficient computation.
    pub fn copy_coeffs(&mut self, other: &Svf) {
        self.k = other.k;
        self.a1 = other.a1;
        self.a2 = other.a2;
        self.a3 = other.a3;
    }

    /// Filters one sample, returning every response.
    #[inline]
    pub fn tick(&mut self, x: f32) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        // Flush denormals out of the state; the filter is otherwise linear.
        if self.ic1.abs() < 1.0e-25 {
            self.ic1 = 0.0;
        }
        if self.ic2.abs() < 1.0e-25 {
            self.ic2 = 0.0;
        }
        let kv1 = self.k * v1;
        SvfOut {
            low: v2,
            band: v1,
            band_unity: kv1,
            high: x - kv1 - v2,
            notch: x - kv1,
        }
    }

    /// Filters one sample with the selected response.
    #[inline]
    pub fn process(&mut self, x: f32, mode: SvfMode) -> f32 {
        self.tick(x).select(mode)
    }
}

/// Four-pole (24 dB/octave) low-pass ladder with a saturating input stage.
///
/// `resonance` runs 0..1 and self-oscillates near 1. The saturator limits the
/// oscillation level, and `drive` adds pre-gain for a harder, dirtier sound.
#[derive(Clone, Copy, Debug)]
pub struct Ladder {
    s: [f32; 4],
    big_g: f32,
    g4: f32,
    k: f32,
    drive: f32,
    comp: f32,
}

impl Default for Ladder {
    fn default() -> Self {
        Ladder::new()
    }
}

impl Ladder {
    pub fn new() -> Ladder {
        let mut l = Ladder {
            s: [0.0; 4],
            big_g: 0.0,
            g4: 0.0,
            k: 0.0,
            drive: 1.0,
            comp: 1.0,
        };
        l.set(1_000.0, 0.0, 1.0, 48_000.0);
        l
    }

    /// Sets cutoff (Hz), resonance (0..1) and input drive (1 is clean).
    pub fn set(&mut self, cutoff_hz: f32, resonance: f32, drive: f32, sample_rate: f32) {
        let fc = clamp_cutoff(cutoff_hz, sample_rate);
        let g = (PI * fc / sample_rate).tan();
        let big_g = g / (1.0 + g);
        self.big_g = big_g;
        self.g4 = big_g * big_g * big_g * big_g;
        let res = resonance.clamp(0.0, 1.0);
        // A little past the linear oscillation threshold (4) so the
        // saturator, not the feedback gain, decides the oscillation level.
        self.k = 4.3 * res;
        self.drive = drive.clamp(0.1, 30.0);
        // Make up some of the level lost to resonance feedback and drive
        // clipping so loudness stays similar across settings.
        self.comp = 1.0 + 0.6 * res;
    }

    pub fn reset(&mut self) {
        self.s = [0.0; 4];
    }

    /// Copies the coefficients (not the state) from another ladder.
    pub fn copy_coeffs(&mut self, other: &Ladder) {
        self.big_g = other.big_g;
        self.g4 = other.g4;
        self.k = other.k;
        self.drive = other.drive;
        self.comp = other.comp;
    }

    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let g = self.big_g;
        let one_minus = 1.0 - g;
        let s = one_minus * (g * g * g * self.s[0] + g * g * self.s[1] + g * self.s[2] + self.s[3]);
        let u = (x * self.drive - self.k * s) / (1.0 + self.k * self.g4);
        let mut input = tanh_approx(u);
        for st in self.s.iter_mut() {
            let v = (input - *st) * g;
            let y = v + *st;
            *st = y + v;
            if st.abs() < 1.0e-25 {
                *st = 0.0;
            }
            input = y;
        }
        // Undo the input drive gain on loud signals so drive changes color
        // more than level.
        input * self.comp / self.drive.sqrt().max(1.0)
    }
}

/// One-pole low-pass (6 dB/octave) with a high-pass companion output.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    a: f32,
    z: f32,
}

impl OnePole {
    pub fn new() -> OnePole {
        OnePole { a: 0.5, z: 0.0 }
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate: f32) {
        let fc = clamp_cutoff(cutoff_hz, sample_rate);
        self.a = 1.0 - (-2.0 * PI * fc / sample_rate).exp();
    }

    pub fn reset(&mut self) {
        self.z = 0.0;
    }

    #[inline]
    pub fn low(&mut self, x: f32) -> f32 {
        self.z += self.a * (x - self.z);
        if self.z.abs() < 1.0e-25 {
            self.z = 0.0;
        }
        self.z
    }

    #[inline]
    pub fn high(&mut self, x: f32) -> f32 {
        x - self.low(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::Noise;
    use crate::util::TAU;

    const SR: f32 = 48_000.0;

    /// RMS gain of a filter for a sine at `freq`, after settling.
    fn gain_at(mut f: impl FnMut(f32) -> f32, freq: f32) -> f32 {
        let n = 24_000;
        let (mut sin_sq, mut out_sq) = (0.0f64, 0.0f64);
        for i in 0..n {
            let x = (TAU * freq * i as f32 / SR).sin();
            let y = f(x);
            if i > 8_000 {
                sin_sq += (x * x) as f64;
                out_sq += (y * y) as f64;
            }
        }
        (out_sq / sin_sq).sqrt() as f32
    }

    #[test]
    fn svf_lowpass_response() {
        let mut f = Svf::new();
        f.set_q(1_000.0, std::f32::consts::FRAC_1_SQRT_2, SR);
        let pass = gain_at(|x| f.process(x, SvfMode::LowPass), 100.0);
        f.reset();
        let at_cut = gain_at(|x| f.process(x, SvfMode::LowPass), 1_000.0);
        f.reset();
        let stop = gain_at(|x| f.process(x, SvfMode::LowPass), 8_000.0);
        assert!((pass - 1.0).abs() < 0.02, "passband {pass}");
        assert!((at_cut - 0.707).abs() < 0.03, "cutoff gain {at_cut}");
        assert!(stop < 0.03, "stopband {stop}");
    }

    #[test]
    fn svf_highpass_bandpass_notch() {
        let mut f = Svf::new();
        f.set_q(2_000.0, 2.0, SR);
        let hp_low = gain_at(|x| f.process(x, SvfMode::HighPass), 100.0);
        f.reset();
        let hp_high = gain_at(|x| f.process(x, SvfMode::HighPass), 15_000.0);
        f.reset();
        let bp_center = gain_at(|x| f.process(x, SvfMode::BandPass), 2_000.0);
        f.reset();
        let notch = gain_at(|x| f.process(x, SvfMode::Notch), 2_000.0);
        assert!(hp_low < 0.02, "hp low {hp_low}");
        assert!((hp_high - 1.0).abs() < 0.1, "hp high {hp_high}");
        assert!((bp_center - 1.0).abs() < 0.05, "bp {bp_center}");
        assert!(notch < 0.05, "notch {notch}");
    }

    #[test]
    fn svf_stable_under_fast_modulation() {
        let mut f = Svf::new();
        let mut rng = Noise::new(9);
        let mut peak = 0.0f32;
        for i in 0..200_000 {
            let cutoff = 20.0 * (1000.0f32).powf(rng.uniform());
            let res = if i % 2 == 0 { 1.0 } else { rng.uniform() };
            f.set(cutoff, res, SR);
            let y = f.process(rng.white(), SvfMode::LowPass);
            assert!(y.is_finite());
            peak = peak.max(y.abs());
        }
        assert!(peak < 200.0, "peak {peak}");
    }

    #[test]
    fn svf_extreme_settings_stay_finite() {
        let mut f = Svf::new();
        for cutoff in [0.0, 1.0, 20.0, 20_000.0, 1.0e9, f32::NAN] {
            for res in [0.0, 1.0, 5.0, -1.0] {
                f.reset();
                f.set(cutoff, res, SR);
                for i in 0..2_000 {
                    let y = f.process(if i == 0 { 1.0 } else { 0.0 }, SvfMode::LowPass);
                    assert!(y.is_finite());
                }
            }
        }
    }

    #[test]
    fn ladder_rolls_off_24db() {
        let mut f = Ladder::new();
        f.set(1_000.0, 0.0, 1.0, SR);
        let pass = gain_at(|x| f.tick(x * 0.1) * 10.0, 100.0);
        f.reset();
        let oct_up = gain_at(|x| f.tick(x * 0.1) * 10.0, 4_000.0);
        assert!((pass - 1.0).abs() < 0.1, "pass {pass}");
        // Two octaves above cutoff a 24 dB/oct filter is ~ -48 dB (+warp).
        assert!(oct_up < 0.02, "stop {oct_up}");
    }

    #[test]
    fn ladder_self_oscillates_at_high_resonance() {
        let mut f = Ladder::new();
        f.set(500.0, 1.0, 1.0, SR);
        let mut peak = 0.0f32;
        for i in 0..48_000 {
            let y = f.tick(if i == 0 { 1.0 } else { 0.0 });
            assert!(y.is_finite());
            if i > 24_000 {
                peak = peak.max(y.abs());
            }
        }
        assert!(peak > 0.05, "no oscillation, peak {peak}");
        assert!(peak < 5.0);
    }

    #[test]
    fn ladder_stable_under_fast_modulation_and_drive() {
        let mut f = Ladder::new();
        let mut rng = Noise::new(21);
        for i in 0..200_000 {
            let cutoff = 20.0 * (1000.0f32).powf(rng.uniform());
            f.set(
                cutoff,
                if i % 3 == 0 { 1.0 } else { rng.uniform() },
                30.0,
                SR,
            );
            let y = f.tick(rng.white() * 4.0);
            assert!(y.is_finite() && y.abs() < 20.0, "y = {y}");
        }
    }

    #[test]
    fn one_pole_splits_signal() {
        let mut f = OnePole::new();
        f.set_cutoff(1_000.0, SR);
        let lo = gain_at(|x| f.low(x), 50.0);
        f.reset();
        let hi_through_low = gain_at(|x| f.low(x), 12_000.0);
        f.reset();
        let hp = gain_at(|x| f.high(x), 50.0);
        assert!(lo > 0.95 && hi_through_low < 0.15 && hp < 0.1);
    }

    #[test]
    fn tanh_approx_is_close_and_bounded() {
        for i in -60..=60 {
            let x = i as f32 * 0.1;
            assert!((tanh_approx(x) - x.tanh()).abs() < 0.025, "x = {x}");
        }
        assert_eq!(tanh_approx(50.0), 1.0);
        assert_eq!(tanh_approx(-50.0), -1.0);
    }
}
