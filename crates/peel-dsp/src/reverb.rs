//! Reverb building blocks: Schroeder all-pass, damped comb, orthogonal
//! mixing matrices and an eight-line feedback delay network (FDN) with
//! modulated delay lines.
//!
//! The FDN follows Jot's design: each line has a gain chosen so the whole
//! network reaches -60 dB after the requested decay time (`T60`), a one-pole
//! low-pass in the loop makes high frequencies die faster, and an orthogonal
//! mixing matrix (Hadamard or Householder) keeps the loop lossless apart from
//! those gains. Realtime rules: only constructors allocate.

use crate::delay::{DelayLine, fast_sin_turns};
use crate::util::{TAU, sanitize};

/// Schroeder all-pass diffuser: `H(z) = (-g + z^-D) / (1 - g z^-D)`.
/// Flat magnitude response; smears transients in time.
#[derive(Clone, Debug)]
pub struct Allpass {
    buf: Vec<f32>,
    pos: usize,
    gain: f32,
}

impl Allpass {
    /// `delay` in samples (at least 1).
    pub fn new(delay: usize, gain: f32) -> Allpass {
        Allpass {
            buf: vec![0.0; delay.max(1)],
            pos: 0,
            gain: gain.clamp(-0.99, 0.99),
        }
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain.clamp(-0.99, 0.99);
    }

    pub fn delay(&self) -> usize {
        self.buf.len()
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let d = self.buf[self.pos];
        let v = sanitize(x + self.gain * d);
        self.buf[self.pos] = v;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        d - self.gain * v
    }
}

/// Feedback comb filter with a one-pole damping low-pass in the loop
/// (the Freeverb "lowpass-feedback comb").
#[derive(Clone, Debug)]
pub struct Comb {
    buf: Vec<f32>,
    pos: usize,
    feedback: f32,
    damp: f32,
    filt: f32,
}

impl Comb {
    /// `delay` in samples (at least 1); `feedback` below 1 for stability.
    pub fn new(delay: usize, feedback: f32, damping: f32) -> Comb {
        Comb {
            buf: vec![0.0; delay.max(1)],
            pos: 0,
            feedback: feedback.clamp(-0.999, 0.999),
            damp: damping.clamp(0.0, 0.999),
            filt: 0.0,
        }
    }

    /// Feedback gain per pass.
    pub fn set_feedback(&mut self, feedback: f32) {
        self.feedback = feedback.clamp(-0.999, 0.999);
    }

    /// Damping from 0 (bright) to just under 1 (very dark).
    pub fn set_damping(&mut self, damping: f32) {
        self.damp = damping.clamp(0.0, 0.999);
    }

    /// Feedback gain that gives a -60 dB decay in `t60` seconds for a comb
    /// of `delay` samples.
    pub fn feedback_for_t60(delay: usize, t60: f32, sample_rate: f32) -> f32 {
        10f32.powf(-3.0 * delay as f32 / (t60.max(1e-3) * sample_rate.max(1.0)))
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
        self.filt = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let out = self.buf[self.pos];
        self.filt = sanitize(out * (1.0 - self.damp) + self.filt * self.damp);
        self.buf[self.pos] = sanitize(x + self.filt * self.feedback);
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        out
    }
}

/// Feedback mixing matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mixing {
    /// Fast Walsh-Hadamard, scaled to be orthogonal. Every output depends on
    /// every input with equal magnitude: dense, even mixing.
    Hadamard,
    /// `x - (2/N) sum(x)`. Cheaper, but each line keeps most of its own
    /// signal, so it builds density more slowly.
    Householder,
}

/// In-place orthonormal 8-point Hadamard transform.
#[inline]
pub fn hadamard8(x: &mut [f32; 8]) {
    // Three butterfly stages, then normalize by 1/sqrt(8).
    for i in 0..4 {
        let (a, b) = (x[i], x[i + 4]);
        x[i] = a + b;
        x[i + 4] = a - b;
    }
    for base in [0usize, 4] {
        for i in 0..2 {
            let (a, b) = (x[base + i], x[base + i + 2]);
            x[base + i] = a + b;
            x[base + i + 2] = a - b;
        }
    }
    for base in [0usize, 2, 4, 6] {
        let (a, b) = (x[base], x[base + 1]);
        x[base] = a + b;
        x[base + 1] = a - b;
    }
    const NORM: f32 = 0.353_553_4; // 1 / sqrt(8)
    for v in x.iter_mut() {
        *v *= NORM;
    }
}

/// In-place orthogonal Householder reflection for 8 lines.
#[inline]
pub fn householder8(x: &mut [f32; 8]) {
    let sum: f32 = x.iter().sum();
    let k = sum * 0.25;
    for v in x.iter_mut() {
        *v -= k;
    }
}

/// Per-pass gain that gives a -60 dB decay in `t60` seconds for a loop whose
/// delay is `len_samples`.
#[inline]
pub fn t60_gain(len_samples: f32, t60: f32, sample_rate: f32) -> f32 {
    10f32.powf(-3.0 * len_samples / (t60.max(1e-3) * sample_rate.max(1.0)))
}

/// Eight-line feedback delay network with modulated delays.
///
/// Feed it an eight-element input vector per sample (the caller decides how
/// the stereo input is spread over the lines) and read the eight damped line
/// outputs (the caller decides how they fold down to stereo).
#[derive(Clone, Debug)]
pub struct Fdn8 {
    lines: [DelayLine; 8],
    sample_rate: f32,
    max_len: f32,
    len_target: [f32; 8],
    len_cur: [f32; 8],
    len_coeff: f32,
    t60: f32,
    gains: [f32; 8],
    damp_a: f32,
    damp_state: [f32; 8],
    mod_depth: f32,
    lfo_phase: [f32; 8],
    lfo_inc: [f32; 8],
    mixing: Mixing,
    ctrl: u32,
}

/// Samples between recomputations of the per-line decay gains.
const FDN_CONTROL_PERIOD: u32 = 32;

/// LFO rates in Hz for the eight lines. Distinct and unrelated, so the
/// modulation never lines up into a audible pattern.
const FDN_LFO_HZ: [f32; 8] = [0.31, 0.43, 0.57, 0.71, 0.83, 0.97, 1.13, 1.27];

impl Fdn8 {
    /// `max_len` is the longest base delay in samples that will be requested;
    /// room for modulation is added internally.
    pub fn new(sample_rate: f32, max_len: usize) -> Fdn8 {
        let headroom = 64;
        let lines = std::array::from_fn(|_| DelayLine::new(max_len + headroom));
        let mut lfo_phase = [0.0; 8];
        let mut lfo_inc = [0.0; 8];
        for i in 0..8 {
            lfo_phase[i] = i as f32 / 8.0;
            lfo_inc[i] = FDN_LFO_HZ[i] / sample_rate.max(1.0);
        }
        let default_len = [
            1_327.0, 1_523.0, 1_789.0, 1_993.0, 2_251.0, 2_503.0, 2_777.0, 3_037.0,
        ];
        let mut f = Fdn8 {
            lines,
            sample_rate,
            max_len: max_len as f32,
            len_target: default_len,
            len_cur: default_len,
            len_coeff: 1.0 - (-1.0 / (0.15 * sample_rate.max(1.0))).exp(),
            t60: 2.0,
            gains: [0.9; 8],
            damp_a: 0.0,
            damp_state: [0.0; 8],
            mod_depth: 0.0,
            lfo_phase,
            lfo_inc,
            mixing: Mixing::Hadamard,
            ctrl: 0,
        };
        f.set_lengths(default_len, true);
        f.update_gains();
        f
    }

    /// Sets the base delay of each line in samples. With `immediate` the
    /// lengths jump; otherwise they glide (about 150 ms time constant), which
    /// bends the tail's pitch instead of clicking.
    pub fn set_lengths(&mut self, lengths: [f32; 8], immediate: bool) {
        for (i, len) in lengths.iter().enumerate() {
            self.len_target[i] = len.clamp(8.0, self.max_len);
            if immediate {
                self.len_cur[i] = self.len_target[i];
            }
        }
        if immediate {
            self.update_gains();
        }
    }

    /// Sets the time for the loop to decay by 60 dB.
    pub fn set_t60(&mut self, seconds: f32) {
        self.t60 = seconds.clamp(0.01, 600.0);
    }

    /// Sets the in-loop damping low-pass cutoff. At or above 0.45 of the
    /// sample rate the filter is bypassed.
    pub fn set_damping_hz(&mut self, hz: f32) {
        if hz >= self.sample_rate * 0.45 {
            self.damp_a = 0.0;
        } else {
            let hz = hz.max(20.0);
            self.damp_a = (-TAU * hz / self.sample_rate).exp();
        }
    }

    /// Peak delay deviation of the modulation in samples.
    pub fn set_mod_depth(&mut self, samples: f32) {
        self.mod_depth = samples.clamp(0.0, 32.0);
    }

    pub fn set_mixing(&mut self, mixing: Mixing) {
        self.mixing = mixing;
    }

    pub fn len_target(&self) -> [f32; 8] {
        self.len_target
    }

    pub fn clear(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        self.damp_state = [0.0; 8];
        self.ctrl = 0;
    }

    fn update_gains(&mut self) {
        for i in 0..8 {
            self.gains[i] = t60_gain(self.len_cur[i], self.t60, self.sample_rate);
        }
    }

    /// Advances one sample. Returns the damped line outputs.
    #[inline]
    pub fn process(&mut self, input: &[f32; 8]) -> [f32; 8] {
        if self.ctrl == 0 {
            self.update_gains();
            self.ctrl = FDN_CONTROL_PERIOD;
        }
        self.ctrl -= 1;

        let mut fed = [0.0f32; 8];
        let mut outs = [0.0f32; 8];
        for i in 0..8 {
            self.len_cur[i] += (self.len_target[i] - self.len_cur[i]) * self.len_coeff;
            let m = if self.mod_depth > 0.0 {
                self.mod_depth * fast_sin_turns(self.lfo_phase[i])
            } else {
                0.0
            };
            self.lfo_phase[i] += self.lfo_inc[i];
            if self.lfo_phase[i] >= 1.0 {
                self.lfo_phase[i] -= 1.0;
            }
            let tap = self.lines[i].read_hermite(self.len_cur[i] + m);
            let damped = if self.damp_a > 0.0 {
                sanitize((1.0 - self.damp_a) * tap + self.damp_a * self.damp_state[i])
            } else {
                tap
            };
            self.damp_state[i] = damped;
            outs[i] = damped;
            fed[i] = damped * self.gains[i];
        }
        match self.mixing {
            Mixing::Hadamard => hadamard8(&mut fed),
            Mixing::Householder => householder8(&mut fed),
        }
        for i in 0..8 {
            self.lines[i].push(sanitize(input[i] + fed[i]));
        }
        outs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn energy(x: &[f32; 8]) -> f32 {
        x.iter().map(|v| v * v).sum()
    }

    #[test]
    fn mixing_matrices_are_orthogonal() {
        let base = [0.3f32, -1.2, 0.7, 0.05, 2.0, -0.4, 0.9, -0.6];
        let mut h = base;
        hadamard8(&mut h);
        assert!((energy(&h) - energy(&base)).abs() < 1e-4);
        let mut q = base;
        householder8(&mut q);
        assert!((energy(&q) - energy(&base)).abs() < 1e-4);
        // Applying the symmetric Hadamard twice gives the identity.
        hadamard8(&mut h);
        for i in 0..8 {
            assert!((h[i] - base[i]).abs() < 1e-5);
        }
        // An impulse spreads evenly under Hadamard.
        let mut imp = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        hadamard8(&mut imp);
        for v in imp {
            assert!((v.abs() - 0.353_553).abs() < 1e-5);
        }
    }

    #[test]
    fn allpass_has_flat_energy_and_rings() {
        let mut ap = Allpass::new(113, 0.6);
        let mut seed = 1u32;
        let (mut ein, mut eout) = (0.0f64, 0.0f64);
        for _ in 0..50_000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = ((seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0;
            let y = ap.process(x);
            ein += f64::from(x * x);
            eout += f64::from(y * y);
        }
        assert!((eout / ein - 1.0).abs() < 0.02, "{}", eout / ein);
        // Impulse response: first output is -g, then the echo at D.
        let mut ap = Allpass::new(10, 0.5);
        let first = ap.process(1.0);
        assert!((first + 0.5).abs() < 1e-6);
        let mut echo = 0.0;
        for n in 1..=10 {
            let y = ap.process(0.0);
            if n == 10 {
                echo = y;
            }
        }
        assert!((echo - 0.75).abs() < 1e-5, "{echo}");
    }

    #[test]
    fn comb_decays_at_requested_t60() {
        let d = 1000;
        let fb = Comb::feedback_for_t60(d, 1.0, SR);
        let mut c = Comb::new(d, fb, 0.0);
        let mut out = vec![c.process(1.0)];
        for _ in 1..(SR as usize * 2) {
            out.push(c.process(0.0));
        }
        // Echo k arrives at k*d with level fb^(k-1); echo 49 is -60 dB.
        let echo49 = out[49 * d].abs();
        assert!((echo49 - 1e-3).abs() < 2e-5, "{echo49}");
        // Damping makes the tail darker (less energy per echo).
        let mut dark = Comb::new(d, fb, 0.7);
        dark.process(1.0);
        let mut e_dark = 0.0;
        for _ in 0..(d * 3) {
            e_dark += dark.process(0.0).powi(2);
        }
        let mut bright = Comb::new(d, fb, 0.0);
        bright.process(1.0);
        let mut e_bright = 0.0;
        for _ in 0..(d * 3) {
            e_bright += bright.process(0.0).powi(2);
        }
        assert!(e_dark < e_bright);
    }

    #[test]
    fn fdn_is_lossless_when_undamped_and_infinite() {
        let mut f = Fdn8::new(SR, 4000);
        let lens = [
            1009.0, 1117.0, 1301.0, 1423.0, 1559.0, 1667.0, 1783.0, 1913.0,
        ];
        f.set_lengths(lens, true);
        f.set_t60(600.0);
        f.set_damping_hz(SR);
        let mut input = [0.0; 8];
        input[0] = 1.0;
        input[3] = -0.5;
        let mut win = [0.0f64; 4];
        for n in 0..(SR as usize * 4) {
            let out = f.process(&input);
            input = [0.0; 8];
            win[n / SR as usize] += f64::from(energy(&out));
        }
        // t60 of 600 s leaves ~0.4% per second, so windows stay close.
        assert!(win[0] > 0.1);
        for w in 1..4 {
            assert!((win[w] / win[0] - 1.0).abs() < 0.12, "{win:?}");
        }
    }

    fn measure_t60(damping_hz: f32, t60: f32) -> f32 {
        let mut f = Fdn8::new(SR, 4000);
        f.set_lengths(
            [
                1009.0, 1117.0, 1301.0, 1423.0, 1559.0, 1667.0, 1783.0, 1913.0,
            ],
            true,
        );
        f.set_t60(t60);
        f.set_damping_hz(damping_hz);
        let mut input = [0.2; 8];
        let mut e = Vec::new();
        let window = 2400;
        let total = (SR * t60 * 0.8) as usize;
        let mut acc = 0.0f32;
        for n in 0..total {
            let out = f.process(&input);
            input = [0.0; 8];
            acc += energy(&out);
            if (n + 1) % window == 0 {
                e.push(10.0 * (acc / window as f32).max(1e-30).log10());
                acc = 0.0;
            }
        }
        // Slope between 10% and 70% of the run, dB per second -> T60.
        let k0 = e.len() / 10;
        let k1 = e.len() * 7 / 10;
        let secs = (k1 - k0) as f32 * window as f32 / SR;
        let slope = (e[k1] - e[k0]) / secs;
        -60.0 / slope
    }

    #[test]
    fn fdn_decays_at_requested_t60() {
        for t60 in [0.8f32, 2.5] {
            let measured = measure_t60(SR, t60);
            assert!(
                (measured / t60 - 1.0).abs() < 0.2,
                "t60 {t60}: measured {measured}"
            );
        }
        // Damping shortens the decay of the (broadband) energy.
        let damped = measure_t60(2000.0, 2.5);
        assert!(damped < 2.5 * 0.9, "{damped}");
    }

    #[test]
    fn fdn_modulation_and_gliding_lengths_stay_finite() {
        let mut f = Fdn8::new(SR, 6000);
        f.set_mod_depth(20.0);
        f.set_t60(30.0);
        let mut seed = 5u32;
        for n in 0..200_000 {
            if n % 20_000 == 0 {
                let s = if (n / 20_000) % 2 == 0 { 0.2 } else { 1.0 };
                f.set_lengths(
                    [
                        1009.0 * s,
                        1117.0 * s,
                        1301.0 * s,
                        1423.0 * s,
                        1559.0 * s,
                        1667.0 * s,
                        1783.0 * s,
                        1913.0 * s,
                    ],
                    false,
                );
            }
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = ((seed >> 8) as f32 / (1u32 << 24) as f32) * 0.1;
            let out = f.process(&[x; 8]);
            for v in out {
                assert!(v.is_finite() && v.abs() < 50.0);
            }
        }
    }
}
