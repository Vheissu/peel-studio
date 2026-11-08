//! Small realtime-safe helpers shared by every device.

use std::f32::consts::PI;

pub const TAU: f32 = 2.0 * PI;

/// Decibels to linear gain. Values at or below -144 dB map to 0.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -144.0 {
        0.0
    } else {
        10f32.powf(db * 0.05)
    }
}

/// Linear gain to decibels, floored at -144 dB.
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 1e-8 {
        -144.0
    } else {
        20.0 * gain.log10()
    }
}

/// MIDI key to frequency (A4 = key 69 = 440 Hz).
#[inline]
pub fn key_to_hz(key: f32) -> f32 {
    440.0 * 2f32.powf((key - 69.0) / 12.0)
}

/// Semitone offset to frequency ratio.
#[inline]
pub fn semitones_to_ratio(st: f32) -> f32 {
    2f32.powf(st / 12.0)
}

/// Equal-power pan law normalized to unity at center. `pan` is -1..=1 and
/// the result is `(left, right)`; a hard pan gives +3 dB on one side.
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let p = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * PI;
    // Normalize so center = (1, 1): divide by cos(pi/4).
    let k = std::f32::consts::SQRT_2;
    (p.cos() * k, p.sin() * k)
}

/// Flushes denormals and non-finite values to zero.
#[inline]
pub fn sanitize(x: f32) -> f32 {
    if x.is_finite() && x.abs() > 1e-30 {
        x
    } else {
        0.0
    }
}

/// One-pole coefficient that reaches ~63% of a step in `time_s` seconds.
#[inline]
pub fn one_pole_coeff(time_s: f32, sample_rate: f32) -> f32 {
    if time_s <= 0.0 {
        1.0
    } else {
        1.0 - (-1.0 / (time_s * sample_rate)).exp()
    }
}

/// Linear-in-time parameter smoother that removes zipper noise.
#[derive(Clone, Copy, Debug)]
pub struct Smoothed {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
    ramp_samples: u32,
}

impl Smoothed {
    pub fn new(value: f32) -> Smoothed {
        Smoothed {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
            ramp_samples: 1,
        }
    }

    /// Sets the ramp duration (call from `prepare`).
    pub fn set_ramp(&mut self, sample_rate: f32, seconds: f32) {
        self.ramp_samples = ((sample_rate * seconds) as u32).max(1);
    }

    pub fn set_target(&mut self, target: f32) {
        if (target - self.target).abs() < f32::EPSILON && self.remaining == 0 {
            return;
        }
        self.target = target;
        self.remaining = self.ramp_samples;
        self.step = (target - self.current) / self.ramp_samples as f32;
    }

    /// Jumps to a value without ramping.
    pub fn set_immediate(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.remaining = 0;
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.current = if self.remaining == 0 {
                self.target
            } else {
                self.current + self.step
            };
        }
        self.current
    }

    pub fn value(&self) -> f32 {
        self.current
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn is_smoothing(&self) -> bool {
        self.remaining > 0
    }
}

/// Peak of a buffer.
pub fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |m, &x| m.max(x.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
        assert!((gain_to_db(0.5) + 6.0206).abs() < 1e-3);
        assert!((key_to_hz(69.0) - 440.0).abs() < 1e-3);
        assert!((key_to_hz(81.0) - 880.0).abs() < 1e-2);
        let (l, r) = pan_gains(0.0);
        assert!((l - 1.0).abs() < 1e-5 && (r - 1.0).abs() < 1e-5);
        let (l, r) = pan_gains(1.0);
        assert!(l.abs() < 1e-5 && (r - std::f32::consts::SQRT_2).abs() < 1e-4);
    }

    #[test]
    fn smoother_reaches_target() {
        let mut s = Smoothed::new(0.0);
        s.set_ramp(1000.0, 0.01);
        s.set_target(1.0);
        let mut last = 0.0;
        for _ in 0..10 {
            last = s.next();
        }
        assert_eq!(last, 1.0);
        assert!(!s.is_smoothing());
    }
}
