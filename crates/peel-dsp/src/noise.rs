//! Fast deterministic noise sources (xorshift white, filtered pink).

/// Marsaglia xorshift32 white-noise generator. Deterministic for a given seed
/// and cheap enough to run per voice.
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    state: u32,
}

impl Noise {
    pub fn new(seed: u32) -> Noise {
        let mut n = Noise { state: 1 };
        n.reseed(seed);
        n
    }

    /// Restarts the sequence. The seed is scrambled so that small consecutive
    /// seeds give unrelated sequences, and zero is valid.
    pub fn reseed(&mut self, seed: u32) {
        let mut s = seed.wrapping_mul(0x9E37_79B9).wrapping_add(0x7F4A_7C15);
        s ^= s >> 15;
        s = s.wrapping_mul(0x85EB_CA6B);
        s ^= s >> 13;
        self.state = if s == 0 { 0xA511_E9B3 } else { s };
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Uniform white noise in `[-1, 1)`.
    #[inline]
    pub fn white(&mut self) -> f32 {
        (self.next_u32() as i32) as f32 * (1.0 / 2_147_483_648.0)
    }

    /// Uniform value in `[0, 1)`.
    #[inline]
    pub fn uniform(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }
}

/// Pink (-3 dB/octave) noise using Paul Kellet's economy filter. Output is
/// roughly in `[-1, 1]`.
#[derive(Clone, Copy, Debug)]
pub struct Pink {
    rng: Noise,
    b0: f32,
    b1: f32,
    b2: f32,
}

impl Pink {
    pub fn new(seed: u32) -> Pink {
        Pink {
            rng: Noise::new(seed),
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
        }
    }

    pub fn reset(&mut self, seed: u32) {
        self.rng.reseed(seed);
        self.b0 = 0.0;
        self.b1 = 0.0;
        self.b2 = 0.0;
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        let w = self.rng.white();
        self.b0 = 0.997_65 * self.b0 + w * 0.099_046;
        self.b1 = 0.963_00 * self.b1 + w * 0.296_516_4;
        self.b2 = 0.570_00 * self.b2 + w * 1.052_691_3;
        (self.b0 + self.b1 + self.b2 + w * 0.1848) * 0.25
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_per_seed() {
        let mut a = Noise::new(42);
        let mut b = Noise::new(42);
        let mut c = Noise::new(43);
        let mut differs = false;
        for _ in 0..100 {
            let (x, y, z) = (a.white(), b.white(), c.white());
            assert_eq!(x, y);
            differs |= x != z;
        }
        assert!(differs);
    }

    #[test]
    fn white_is_bounded_and_zero_mean() {
        let mut n = Noise::new(0);
        let mut sum = 0.0f64;
        let mut sq = 0.0f64;
        let count = 200_000;
        for _ in 0..count {
            let v = n.white();
            assert!((-1.0..1.0).contains(&v));
            sum += v as f64;
            sq += (v * v) as f64;
        }
        assert!((sum / count as f64).abs() < 0.01);
        // Uniform on [-1,1) has variance 1/3.
        assert!((sq / count as f64 - 1.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn uniform_in_unit_interval() {
        let mut n = Noise::new(5);
        for _ in 0..10_000 {
            let u = n.uniform();
            assert!((0.0..1.0).contains(&u));
        }
    }

    #[test]
    fn pink_has_more_low_than_high_energy() {
        let mut p = Pink::new(3);
        let mut prev = 0.0f32;
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for _ in 0..100_000 {
            let v = p.tick();
            assert!(v.abs() < 2.0);
            lo += ((v + prev) * 0.5).powi(2) as f64; // crude lowpass
            hi += ((v - prev) * 0.5).powi(2) as f64; // crude highpass
            prev = v;
        }
        assert!(lo > hi * 4.0, "lo {lo} hi {hi}");
    }
}
