//! Dynamics building blocks: level detection, gain computer, gain
//! ballistics and a lookahead brickwall limiter core.
//!
//! The compressor in `peel-effects` composes [`EnvelopeFollower`],
//! [`GainComputer`] and [`GainSmoother`]; the limiter wraps
//! [`LookaheadLimiter`]. Realtime rules: only constructors allocate.

use crate::util::{db_to_gain, gain_to_db};

/// Level detector flavor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectorMode {
    /// Rectified peak with separate attack and release.
    Peak,
    /// Mean-square tracking; the output is the root of the smoothed power.
    Rms,
}

/// One-pole coefficient for a time constant, so that 63% of a step is
/// reached after `seconds`. Zero or negative time gives an instant response.
#[inline]
pub fn time_coeff(seconds: f32, sample_rate: f32) -> f32 {
    if seconds <= 0.0 {
        0.0
    } else {
        (-1.0 / (seconds * sample_rate.max(1.0))).exp()
    }
}

/// Peak or RMS envelope follower with separate attack and release.
#[derive(Clone, Copy, Debug)]
pub struct EnvelopeFollower {
    mode: DetectorMode,
    attack: f32,
    release: f32,
    state: f32,
}

impl EnvelopeFollower {
    pub fn new(mode: DetectorMode) -> EnvelopeFollower {
        EnvelopeFollower {
            mode,
            attack: 0.0,
            release: 0.999,
            state: 0.0,
        }
    }

    pub fn set_mode(&mut self, mode: DetectorMode) {
        if mode != self.mode {
            self.mode = mode;
            // Power and amplitude differ by a square; start clean.
            self.state = 0.0;
        }
    }

    pub fn set_times(&mut self, attack_s: f32, release_s: f32, sample_rate: f32) {
        self.attack = time_coeff(attack_s, sample_rate);
        self.release = time_coeff(release_s, sample_rate);
    }

    pub fn reset(&mut self) {
        self.state = 0.0;
    }

    /// Feeds one sample and returns the current linear level.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let v = match self.mode {
            DetectorMode::Peak => x.abs(),
            DetectorMode::Rms => x * x,
        };
        let v = if v.is_finite() { v } else { 0.0 };
        let coeff = if v > self.state {
            self.attack
        } else {
            self.release
        };
        self.state = v + coeff * (self.state - v);
        if self.state < 1e-30 {
            self.state = 0.0;
        }
        match self.mode {
            DetectorMode::Peak => self.state,
            DetectorMode::Rms => self.state.sqrt(),
        }
    }
}

/// Static compression curve with a quadratic soft knee (Giannoulis, Massberg
/// and Reiss). Works on levels in decibels.
#[derive(Clone, Copy, Debug)]
pub struct GainComputer {
    pub threshold_db: f32,
    /// Input:output ratio, 1 or more. `f32::INFINITY` limits.
    pub ratio: f32,
    /// Width of the soft knee in dB, centered on the threshold.
    pub knee_db: f32,
}

impl GainComputer {
    pub fn new(threshold_db: f32, ratio: f32, knee_db: f32) -> GainComputer {
        GainComputer {
            threshold_db,
            ratio,
            knee_db,
        }
    }

    /// Gain change in dB (zero or negative) for an input level in dB.
    #[inline]
    pub fn gain_db(&self, level_db: f32) -> f32 {
        let slope = 1.0 / self.ratio.max(1.0) - 1.0;
        let over = level_db - self.threshold_db;
        let knee = self.knee_db.max(0.0);
        if 2.0 * over <= -knee {
            0.0
        } else if knee > 0.0 && 2.0 * over.abs() < knee {
            let x = over + knee * 0.5;
            slope * x * x / (2.0 * knee)
        } else {
            slope * over
        }
    }

    /// Output level in dB for an input level in dB.
    pub fn output_db(&self, level_db: f32) -> f32 {
        level_db + self.gain_db(level_db)
    }
}

/// Gain-reduction ballistics in the log domain. Attack applies while the
/// reduction is increasing, release while it recovers.
#[derive(Clone, Copy, Debug)]
pub struct GainSmoother {
    attack: f32,
    release: f32,
    state_db: f32,
}

impl GainSmoother {
    pub fn new() -> GainSmoother {
        GainSmoother {
            attack: 0.0,
            release: 0.999,
            state_db: 0.0,
        }
    }

    pub fn set_times(&mut self, attack_s: f32, release_s: f32, sample_rate: f32) {
        self.attack = time_coeff(attack_s, sample_rate);
        self.release = time_coeff(release_s, sample_rate);
    }

    pub fn reset(&mut self) {
        self.state_db = 0.0;
    }

    /// Current smoothed gain in dB (zero or negative).
    pub fn value_db(&self) -> f32 {
        self.state_db
    }

    /// Moves toward `target_db` (zero or negative) and returns the new gain.
    #[inline]
    pub fn process(&mut self, target_db: f32) -> f32 {
        let coeff = if target_db < self.state_db {
            self.attack
        } else {
            self.release
        };
        self.state_db = target_db + coeff * (self.state_db - target_db);
        if self.state_db > -1e-3 && target_db >= 0.0 {
            self.state_db = 0.0;
        }
        self.state_db
    }
}

impl Default for GainSmoother {
    fn default() -> Self {
        GainSmoother::new()
    }
}

/// Monotonic-deque sliding-window minimum over the last `window` pushes.
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
    fn new(window: usize) -> SlidingMin {
        let cap = window.max(1) + 1;
        SlidingMin {
            vals: vec![1.0; cap],
            pos: vec![0; cap],
            head: 0,
            len: 0,
            window: window.max(1) as u64,
            t: 0,
        }
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

/// Running average over a fixed window, `f64` accumulator.
#[derive(Clone, Debug)]
struct MovingAverage {
    ring: Vec<f32>,
    pos: usize,
    sum: f64,
    init: f32,
}

impl MovingAverage {
    fn new(window: usize, init: f32) -> MovingAverage {
        let n = window.max(1);
        MovingAverage {
            ring: vec![init; n],
            pos: 0,
            sum: f64::from(init) * n as f64,
            init,
        }
    }

    fn reset(&mut self) {
        self.ring.fill(self.init);
        self.pos = 0;
        self.sum = f64::from(self.init) * self.ring.len() as f64;
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

/// Stereo-linked lookahead brickwall limiter.
///
/// Signal flow per sample: the (already gained) input is written to a delay
/// of `lookahead` samples; a peak detector looks at the undelayed signal,
/// optionally with an inter-sample estimate; the required gain goes through a
/// sliding minimum (so the reduction is in place before the peak arrives), an
/// instant-attack / exponential-release smoother, and finally a moving
/// average whose window equals the lookahead, which turns the gain steps into
/// linear ramps. Since the gain is always at or below the sliding minimum, the
/// delayed sample times the gain cannot exceed the ceiling. A final clamp
/// removes any rounding residue, so **sample peaks never exceed the ceiling**.
///
/// With `true_peak` enabled the detector also checks the midpoint between
/// adjacent samples using a 4-point interpolator, which catches most
/// inter-sample overs (typically within 1 dB of the real peak).
///
/// Latency equals `lookahead` samples.
#[derive(Clone, Debug)]
pub struct LookaheadLimiter {
    lookahead: usize,
    ring_l: Vec<f32>,
    ring_r: Vec<f32>,
    mask: usize,
    write: usize,
    min: SlidingMin,
    avg: MovingAverage,
    ceiling: f32,
    release_coeff: f32,
    released: f32,
    true_peak: bool,
    gain: f32,
}

impl LookaheadLimiter {
    /// `lookahead` is in samples (at least 2) and is also the latency.
    pub fn new(lookahead: usize) -> LookaheadLimiter {
        let lookahead = lookahead.max(2);
        let cap = (lookahead + 8).next_power_of_two();
        LookaheadLimiter {
            lookahead,
            ring_l: vec![0.0; cap],
            ring_r: vec![0.0; cap],
            mask: cap - 1,
            write: 0,
            min: SlidingMin::new(lookahead),
            avg: MovingAverage::new(lookahead, 1.0),
            ceiling: 1.0,
            release_coeff: 0.999,
            released: 1.0,
            true_peak: true,
            gain: 1.0,
        }
    }

    /// Lookahead (and latency) in samples.
    pub fn lookahead(&self) -> usize {
        self.lookahead
    }

    /// Sets the ceiling in dBFS.
    pub fn set_ceiling_db(&mut self, db: f32) {
        self.ceiling = db_to_gain(db.clamp(-60.0, 6.0)).max(1e-6);
    }

    /// Time for the gain to recover about 63% of the way, in seconds.
    pub fn set_release(&mut self, seconds: f32, sample_rate: f32) {
        self.release_coeff = time_coeff(seconds.max(1e-4), sample_rate);
    }

    pub fn set_true_peak(&mut self, on: bool) {
        self.true_peak = on;
    }

    pub fn reset(&mut self) {
        self.ring_l.fill(0.0);
        self.ring_r.fill(0.0);
        self.write = 0;
        self.min.reset();
        self.avg.reset();
        self.released = 1.0;
        self.gain = 1.0;
    }

    /// Gain applied to the most recent sample, as a positive reduction in dB.
    pub fn gain_reduction_db(&self) -> f32 {
        -gain_to_db(self.gain)
    }

    #[inline]
    fn at(&self, back: usize, right: bool) -> f32 {
        let i = self.write.wrapping_sub(1 + back) & self.mask;
        if right {
            self.ring_r[i]
        } else {
            self.ring_l[i]
        }
    }

    /// Processes one stereo frame, returning the limited frame delayed by
    /// [`LookaheadLimiter::lookahead`] samples.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let l = if l.is_finite() { l } else { 0.0 };
        let r = if r.is_finite() { r } else { 0.0 };
        self.ring_l[self.write] = l;
        self.ring_r[self.write] = r;
        self.write = (self.write + 1) & self.mask;

        // Detection runs one sample behind the input so the midpoint between
        // the two most recent *detected* samples has its four neighbours.
        // After the push, back(0) is the newest sample (n); detect sample n-1.
        let mut peak = self.at(1, false).abs().max(self.at(1, true).abs());
        if self.true_peak {
            for right in [false, true] {
                let (x0, x1, x2, x3) = (
                    self.at(3, right),
                    self.at(2, right),
                    self.at(1, right),
                    self.at(0, right),
                );
                let mid = (9.0 * (x1 + x2) - (x0 + x3)) * 0.0625;
                peak = peak.max(mid.abs());
            }
        }
        let target = if peak > self.ceiling {
            self.ceiling / peak
        } else {
            1.0
        };
        let m = self.min.push(target);
        self.released = if m < self.released {
            m
        } else {
            m + self.release_coeff * (self.released - m)
        };
        let g = self.avg.push(self.released.min(1.0)).min(1.0);
        self.gain = g;

        // Audio is delayed by `lookahead`: newest is back(0) = x[n], so
        // x[n - lookahead] is back(lookahead).
        let dl = self.at(self.lookahead, false);
        let dr = self.at(self.lookahead, true);
        let c = self.ceiling;
        ((dl * g).clamp(-c, c), (dr * g).clamp(-c, c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn envelope_follower_time_constants() {
        let mut e = EnvelopeFollower::new(DetectorMode::Peak);
        e.set_times(0.010, 0.100, SR);
        let mut v = 0.0;
        for _ in 0..(0.010 * SR) as usize {
            v = e.process(1.0);
        }
        assert!((v - 0.632).abs() < 0.02, "attack {v}");
        for _ in 0..(0.100 * SR) as usize {
            v = e.process(0.0);
        }
        // 63.2% of the 0.632 level is gone after one release time constant.
        assert!((v - 0.632 * 0.368).abs() < 0.02, "release {v}");
    }

    #[test]
    fn rms_follower_reads_sine_rms() {
        let mut e = EnvelopeFollower::new(DetectorMode::Rms);
        e.set_times(0.02, 0.02, SR);
        let mut v = 0.0;
        for n in 0..48_000 {
            v = e.process((std::f32::consts::TAU * 1000.0 * n as f32 / SR).sin());
        }
        assert!((v - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02, "{v}");
    }

    #[test]
    fn gain_computer_curve() {
        let g = GainComputer::new(-20.0, 4.0, 0.0);
        assert_eq!(g.gain_db(-30.0), 0.0);
        assert!((g.gain_db(-8.0) - (-9.0)).abs() < 1e-5); // 12 dB over -> 3 dB over
        assert!((g.output_db(0.0) - (-15.0)).abs() < 1e-5);
        // Soft knee: zero well below, continuous through the knee.
        let k = GainComputer::new(-20.0, 4.0, 10.0);
        assert_eq!(k.gain_db(-26.0), 0.0);
        let mut prev = k.gain_db(-30.0);
        let mut level = -30.0;
        while level < -5.0 {
            level += 0.05;
            let now = k.gain_db(level);
            assert!(now <= prev + 1e-5);
            assert!((now - prev).abs() < 0.1, "jump at {level}");
            prev = now;
        }
        // At the threshold the knee gives half-width reduction.
        let at_t = k.gain_db(-20.0);
        assert!(at_t < 0.0 && at_t > -1.0);
        // Above the knee it follows the hard curve.
        assert!((k.gain_db(-10.0) + 7.5).abs() < 1e-4);
        // Infinite ratio pins output at the threshold.
        let inf = GainComputer::new(-6.0, f32::INFINITY, 0.0);
        assert!((inf.output_db(6.0) + 6.0).abs() < 1e-5);
    }

    #[test]
    fn smoother_attack_and_release() {
        let mut s = GainSmoother::new();
        s.set_times(0.005, 0.050, SR);
        let mut v = 0.0;
        for _ in 0..(0.005 * SR) as usize {
            v = s.process(-20.0);
        }
        assert!((v - -12.64).abs() < 0.4, "{v}");
        for _ in 0..(0.5 * SR) as usize {
            v = s.process(0.0);
        }
        assert_eq!(v, 0.0);
    }

    #[test]
    fn sliding_min_matches_brute_force() {
        let mut m = SlidingMin::new(7);
        let mut hist = Vec::new();
        let mut seed = 7u32;
        for _ in 0..500 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (seed >> 8) as f32 / (1u32 << 24) as f32;
            hist.push(x);
            let want = hist[hist.len().saturating_sub(7)..]
                .iter()
                .copied()
                .fold(f32::MAX, f32::min);
            assert_eq!(m.push(x), want);
        }
    }

    #[test]
    fn moving_average_is_exact() {
        let mut a = MovingAverage::new(4, 1.0);
        assert_eq!(a.push(1.0), 1.0);
        a.push(0.0);
        a.push(0.0);
        a.push(0.0);
        assert!((a.push(0.0)).abs() < 1e-7);
        assert!((a.push(1.0) - 0.25).abs() < 1e-6);
    }

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((*seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
    }

    #[test]
    fn limiter_never_exceeds_ceiling() {
        for (ceiling_db, lookahead) in [(-0.3f32, 240usize), (-6.0, 64), (-1.0, 5)] {
            let mut lim = LookaheadLimiter::new(lookahead);
            lim.set_ceiling_db(ceiling_db);
            lim.set_release(0.05, SR);
            let ceiling = db_to_gain(ceiling_db);
            let mut seed = 99u32;
            let mut worst = 0.0f32;
            for n in 0..96_000 {
                // Hot noise bursts plus a loud sine with isolated spikes.
                let burst = if (n / 8000) % 2 == 0 { 4.0 } else { 0.3 };
                let s = (std::f32::consts::TAU * 220.0 * n as f32 / SR).sin() * 3.0;
                let spike = if n % 5000 == 0 { 20.0 } else { 0.0 };
                let l = lcg(&mut seed) * burst + s + spike;
                let r = lcg(&mut seed) * burst - s;
                let (a, b) = lim.process(l, r);
                worst = worst.max(a.abs()).max(b.abs());
            }
            assert!(
                worst <= ceiling + 1e-6,
                "lookahead {lookahead}: peak {worst} > {ceiling}"
            );
            assert!(
                worst > ceiling * 0.9,
                "limiter should be working near ceiling"
            );
        }
    }

    #[test]
    fn limiter_is_transparent_below_ceiling_and_delays_by_lookahead() {
        let la = 100;
        let mut lim = LookaheadLimiter::new(la);
        lim.set_ceiling_db(0.0);
        let mut input = Vec::new();
        let mut out = Vec::new();
        for n in 0..2000 {
            let x = 0.5 * (n as f32 * 0.05).sin();
            input.push(x);
            out.push(lim.process(x, -x).0);
        }
        for n in la..2000 {
            assert!((out[n] - input[n - la]).abs() < 1e-6, "n {n}");
        }
        assert_eq!(lim.gain_reduction_db(), 0.0);
    }

    #[test]
    fn limiter_attack_is_ahead_of_the_peak_and_releases() {
        let la = 120;
        let mut lim = LookaheadLimiter::new(la);
        lim.set_ceiling_db(-6.0);
        lim.set_release(0.02, SR);
        lim.set_true_peak(false);
        let mut out = Vec::new();
        for n in 0..24_000 {
            let x = if (2000..2200).contains(&n) { 2.0 } else { 0.1 };
            out.push(lim.process(x, x).0);
        }
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak <= db_to_gain(-6.0) + 1e-6);
        // The quiet part afterwards returns to full level (gain 1).
        assert!((out[23_000] - 0.1).abs() < 1e-4);
        // Gain reduction began before the loud segment reached the output.
        let first_loud_out = 2000 + la;
        assert!(
            out[first_loud_out - 20] < 0.08,
            "attack ramp should already dip"
        );
    }

    #[test]
    fn limiter_survives_non_finite_input() {
        let mut lim = LookaheadLimiter::new(50);
        lim.set_ceiling_db(-1.0);
        for n in 0..1000 {
            let x = match n % 7 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                _ => 0.5,
            };
            let (a, b) = lim.process(x, x);
            assert!(a.is_finite() && b.is_finite());
        }
    }

    #[test]
    fn true_peak_catches_intersample_peaks() {
        // fs/4 sine sampled 45 degrees off its peaks: sample peak 0.707, true
        // peak 1.0. With a -3 dB ceiling the true-peak detector reduces more.
        let run = |tp: bool| {
            let mut lim = LookaheadLimiter::new(64);
            lim.set_ceiling_db(-3.0);
            lim.set_true_peak(tp);
            let mut gr = 0.0f32;
            for n in 0..4000 {
                let x =
                    (std::f32::consts::FRAC_PI_4 + std::f32::consts::FRAC_PI_2 * n as f32).sin();
                lim.process(x, x);
                gr = gr.max(lim.gain_reduction_db());
            }
            gr
        };
        assert!(run(true) > run(false) + 0.3);
    }
}
