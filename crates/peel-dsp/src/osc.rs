//! Band-limited oscillators and a low-frequency oscillator.
//!
//! Saw and square use PolyBLEP, triangle uses PolyBLAMP, so the waveforms stay
//! clean up to high fundamentals without oversampling. Phases are normalized
//! (`0..1`) and `dt` is the per-sample phase increment (`frequency / sample
//! rate`). Everything here is allocation free and realtime safe.

use crate::noise::Noise;
use crate::util::TAU;

/// Largest phase increment the oscillators accept (just under Nyquist/2 so a
/// correction window never spans more than one edge).
pub const MAX_DT: f32 = 0.49;

/// Band-limited step residual (PolyBLEP). `t` is the phase, `dt` the phase
/// increment. Non-zero only within one sample of the discontinuity at `t = 0`.
#[inline]
pub fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        -(1.0 - x) * (1.0 - x)
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt + 1.0;
        x * x
    } else {
        0.0
    }
}

/// Band-limited slope-change residual (PolyBLAMP), normalized so a corner
/// whose slope changes by `ds` (value per unit phase) is corrected with
/// `ds * dt * poly_blamp(t, dt)`.
#[inline]
pub fn poly_blamp(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = 1.0 - t / dt;
        x * x * x * (1.0 / 6.0)
    } else if t > 1.0 - dt {
        let x = 1.0 + (t - 1.0) / dt;
        x * x * x * (1.0 / 6.0)
    } else {
        0.0
    }
}

/// Sine of `turns` full cycles (`sin(2*pi*turns)`), accurate to about 4e-6 and
/// several times faster than `f32::sin`. Any finite input is accepted.
#[inline]
pub fn fast_sin(turns: f32) -> f32 {
    let x = turns - turns.round();
    let x = if x > 0.25 {
        0.5 - x
    } else if x < -0.25 {
        -0.5 - x
    } else {
        x
    };
    let th = x * TAU;
    let t2 = th * th;
    th * (1.0
        + t2 * (-1.0 / 6.0 + t2 * (1.0 / 120.0 + t2 * (-1.0 / 5040.0 + t2 * (1.0 / 362_880.0)))))
}

/// Oscillator waveform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Waveform {
    Sine,
    Triangle,
    Saw,
    /// Pulse wave; the width is passed separately (0.5 is a square).
    Square,
}

impl Waveform {
    /// Maps a choice index (0 sine, 1 triangle, 2 saw, 3 square).
    pub fn from_index(i: usize) -> Waveform {
        match i {
            0 => Waveform::Sine,
            1 => Waveform::Triangle,
            2 => Waveform::Saw,
            _ => Waveform::Square,
        }
    }
}

/// A free-running phase accumulator that renders band-limited waveforms.
#[derive(Clone, Copy, Debug, Default)]
pub struct Osc {
    pub phase: f32,
}

impl Osc {
    pub const fn new() -> Osc {
        Osc { phase: 0.0 }
    }

    pub const fn with_phase(phase: f32) -> Osc {
        Osc { phase }
    }

    pub fn reset(&mut self, phase: f32) {
        self.phase = phase - phase.floor();
    }

    /// Renders one sample and advances the phase.
    #[inline]
    pub fn tick(&mut self, wave: Waveform, dt: f32, pw: f32) -> f32 {
        let dt = dt.clamp(0.0, MAX_DT);
        let v = eval(wave, self.phase, dt, pw);
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        v
    }

    /// Advances the phase without rendering.
    #[inline]
    pub fn advance(&mut self, dt: f32) {
        self.phase += dt.clamp(0.0, MAX_DT);
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
    }

    #[inline]
    pub fn saw(&mut self, dt: f32) -> f32 {
        self.tick(Waveform::Saw, dt, 0.5)
    }

    #[inline]
    pub fn square(&mut self, dt: f32, pw: f32) -> f32 {
        self.tick(Waveform::Square, dt, pw)
    }

    #[inline]
    pub fn triangle(&mut self, dt: f32) -> f32 {
        self.tick(Waveform::Triangle, dt, 0.5)
    }

    #[inline]
    pub fn sine(&mut self, dt: f32) -> f32 {
        self.tick(Waveform::Sine, dt, 0.5)
    }
}

/// Stateless waveform evaluation at `phase` (0..1) with increment `dt`.
#[inline]
pub fn eval(wave: Waveform, phase: f32, dt: f32, pw: f32) -> f32 {
    match wave {
        Waveform::Sine => fast_sin(phase),
        Waveform::Saw => 2.0 * phase - 1.0 - poly_blep(phase, dt),
        Waveform::Square => {
            let pw = pw.clamp(0.02, 0.98);
            let naive = if phase < pw { 1.0 } else { -1.0 };
            let mut fall = phase - pw;
            if fall < 0.0 {
                fall += 1.0;
            }
            naive + poly_blep(phase, dt) - poly_blep(fall, dt)
        }
        Waveform::Triangle => {
            let naive = 1.0 - 4.0 * (phase - 0.5).abs();
            let mut peak = phase - 0.5;
            if peak < 0.0 {
                peak += 1.0;
            }
            naive + 8.0 * dt * (poly_blamp(phase, dt) - poly_blamp(peak, dt))
        }
    }
}

/// LFO shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LfoShape {
    Sine,
    Triangle,
    /// Rising ramp.
    Saw,
    Square,
    /// Random value held for one cycle.
    SampleHold,
}

impl LfoShape {
    pub fn from_index(i: usize) -> LfoShape {
        match i {
            0 => LfoShape::Sine,
            1 => LfoShape::Triangle,
            2 => LfoShape::Saw,
            3 => LfoShape::Square,
            _ => LfoShape::SampleHold,
        }
    }
}

/// Bipolar (-1..1) low-frequency oscillator.
#[derive(Clone, Copy, Debug)]
pub struct Lfo {
    pub phase: f32,
    held: f32,
    rng: Noise,
}

impl Lfo {
    pub fn new(seed: u32) -> Lfo {
        let mut rng = Noise::new(seed);
        let held = rng.white();
        Lfo {
            phase: 0.0,
            held,
            rng,
        }
    }

    pub fn reset(&mut self, phase: f32) {
        self.phase = phase - phase.floor();
    }

    /// Value at the current phase without advancing.
    #[inline]
    pub fn value(&self, shape: LfoShape) -> f32 {
        let p = self.phase;
        match shape {
            LfoShape::Sine => fast_sin(p),
            LfoShape::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
            LfoShape::Saw => 2.0 * p - 1.0,
            LfoShape::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoShape::SampleHold => self.held,
        }
    }

    /// Advances by `dt` cycles (`rate_hz / sample_rate`) and returns the new
    /// value.
    #[inline]
    pub fn tick(&mut self, shape: LfoShape, dt: f32) -> f32 {
        self.phase += dt.clamp(0.0, 0.5);
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.held = self.rng.white();
        }
        self.value(shape)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn crossings(wave: Waveform, freq: f32, seconds: f32) -> usize {
        let mut osc = Osc::new();
        let dt = freq / SR;
        let n = (SR * seconds) as usize;
        let mut prev = osc.tick(wave, dt, 0.5);
        let mut count = 0;
        for _ in 0..n {
            let v = osc.tick(wave, dt, 0.5);
            if prev < 0.0 && v >= 0.0 {
                count += 1;
            }
            prev = v;
        }
        count
    }

    #[test]
    fn frequency_accuracy_by_zero_crossings() {
        for wave in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Saw,
            Waveform::Square,
        ] {
            for freq in [55.0, 440.0, 3_000.0] {
                let seconds = 1.0;
                let got = crossings(wave, freq, seconds) as f32;
                let expect = freq * seconds;
                assert!(
                    (got - expect).abs() <= 1.0,
                    "{wave:?} {freq} Hz: {got} crossings, expected {expect}"
                );
            }
        }
    }

    #[test]
    fn fast_sin_is_accurate() {
        let mut worst = 0.0f32;
        let mut x = -3.0f32;
        while x < 3.0 {
            let e = (fast_sin(x) - (x * TAU).sin()).abs();
            worst = worst.max(e);
            x += 0.001_37;
        }
        assert!(worst < 2e-5, "worst error {worst}");
    }

    #[test]
    fn waveforms_stay_bounded_and_centered() {
        for wave in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Saw,
            Waveform::Square,
        ] {
            let mut osc = Osc::new();
            let dt = 1_234.5 / SR;
            let mut sum = 0.0f64;
            let n = 48_000;
            for _ in 0..n {
                let v = osc.tick(wave, dt, 0.5);
                assert!(v.abs() < 1.3, "{wave:?} out of range: {v}");
                sum += v as f64;
            }
            assert!((sum / n as f64).abs() < 0.02, "{wave:?} has DC offset");
        }
    }

    #[test]
    fn blep_reduces_aliasing_for_saw() {
        // Compare high-frequency energy of the band-limited saw against a naive
        // saw by looking at a Goertzel bin at an aliased frequency.
        fn bin(buf: &[f32], freq: f32) -> f32 {
            let w = TAU * freq / SR;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &v) in buf.iter().enumerate() {
                let a = w as f64 * i as f64;
                re += v as f64 * a.cos();
                im += v as f64 * a.sin();
            }
            ((re * re + im * im).sqrt() / buf.len() as f64) as f32
        }
        let f0 = 4_321.0;
        let dt = f0 / SR;
        let n = 16_384;
        let mut naive = Vec::with_capacity(n);
        let mut clean = Vec::with_capacity(n);
        let mut o = Osc::new();
        let mut phase = 0.0f32;
        for _ in 0..n {
            naive.push(2.0 * phase - 1.0);
            phase += dt;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            clean.push(o.saw(dt));
        }
        // 5th harmonic = 21605 Hz folds to 2395 Hz, which is not a harmonic of f0.
        let alias = SR - 5.0 * f0;
        let a_naive = bin(&naive, alias);
        let a_clean = bin(&clean, alias);
        assert!(
            a_clean < a_naive * 0.5,
            "alias not reduced: naive {a_naive}, clean {a_clean}"
        );
    }

    #[test]
    fn triangle_matches_ideal_at_low_frequency() {
        let mut osc = Osc::new();
        let dt = 50.0 / SR;
        for _ in 0..2000 {
            let phase = osc.phase;
            let v = osc.triangle(dt);
            let ideal = 1.0 - 4.0 * (phase - 0.5).abs();
            assert!((v - ideal).abs() < 0.01, "{v} vs {ideal}");
        }
    }

    #[test]
    fn pulse_width_changes_duty_cycle() {
        let mut osc = Osc::new();
        let dt = 100.0 / SR;
        let mut high = 0;
        let n = 48_000;
        for _ in 0..n {
            if osc.square(dt, 0.25) > 0.0 {
                high += 1;
            }
        }
        let duty = high as f32 / n as f32;
        assert!((duty - 0.25).abs() < 0.01, "duty {duty}");
    }

    #[test]
    fn lfo_period_and_range() {
        let mut lfo = Lfo::new(7);
        let dt = 5.0 / SR;
        let mut prev = lfo.value(LfoShape::Sine);
        let mut up = 0;
        for _ in 0..48_000 {
            let v = lfo.tick(LfoShape::Sine, dt);
            assert!((-1.001..=1.001).contains(&v));
            if prev < 0.0 && v >= 0.0 {
                up += 1;
            }
            prev = v;
        }
        assert!((4..=6).contains(&up), "{up} cycles in 1 s at 5 Hz");
    }
}
