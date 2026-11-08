//! Envelope generators.
//!
//! [`Adsr`] uses exponential decay and release segments (analog style) with a
//! choice of exponential or linear attack. A retrigger starts the attack from
//! the current level, so there is never a discontinuity. [`Decay`] is a simple
//! percussive envelope for drums.

/// Shortest attack/decay/release the envelopes accept; avoids clicks and
/// divisions by zero.
pub const MIN_TIME: f32 = 0.000_5;

/// Overshoot ratio that shapes the exponential attack.
const ATTACK_RATIO: f32 = 0.3;
/// Overshoot ratio for decay and release; smaller reaches the end sooner
/// relative to the nominal time.
const DECAY_RATIO: f32 = 0.000_3;
/// Level below which a releasing envelope is considered finished.
const IDLE_LEVEL: f32 = 1.0e-5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

fn exp_coef(samples: f32, ratio: f32) -> f32 {
    (-((1.0 + ratio) / ratio).ln() / samples.max(1.0)).exp()
}

/// Attack/decay/sustain/release envelope.
///
/// Times are in seconds. `decay` and `release` are the time a full-scale fall
/// takes to reach silence, so the audible tail is shorter than the number
/// suggests (the first half of the time covers about 30 dB).
#[derive(Clone, Copy, Debug)]
pub struct Adsr {
    stage: Stage,
    level: f32,
    sample_rate: f32,
    sustain: f32,
    linear_attack: bool,
    attack_inc: f32,
    attack_coef: f32,
    attack_base: f32,
    decay_coef: f32,
    decay_base: f32,
    release_coef: f32,
    release_base: f32,
    sustain_coef: f32,
}

impl Adsr {
    pub fn new(sample_rate: f32) -> Adsr {
        let mut e = Adsr {
            stage: Stage::Idle,
            level: 0.0,
            sample_rate: sample_rate.max(1.0),
            sustain: 0.7,
            linear_attack: false,
            attack_inc: 0.0,
            attack_coef: 0.0,
            attack_base: 0.0,
            decay_coef: 0.0,
            decay_base: 0.0,
            release_coef: 0.0,
            release_base: 0.0,
            sustain_coef: 0.0,
        };
        e.set(0.005, 0.2, 0.7, 0.3, false);
        e
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    /// Updates all stage parameters. Safe to call while the envelope runs; the
    /// level continues smoothly.
    pub fn set(&mut self, attack: f32, decay: f32, sustain: f32, release: f32, linear: bool) {
        let sr = self.sample_rate;
        let a = attack.max(MIN_TIME) * sr;
        let d = decay.max(MIN_TIME) * sr;
        let r = release.max(MIN_TIME) * sr;
        self.sustain = sustain.clamp(0.0, 1.0);
        self.linear_attack = linear;
        self.attack_inc = 1.0 / a;
        self.attack_coef = exp_coef(a, ATTACK_RATIO);
        self.attack_base = (1.0 + ATTACK_RATIO) * (1.0 - self.attack_coef);
        self.decay_coef = exp_coef(d, DECAY_RATIO);
        self.decay_base = (self.sustain - DECAY_RATIO) * (1.0 - self.decay_coef);
        self.release_coef = exp_coef(r, DECAY_RATIO);
        self.release_base = -DECAY_RATIO * (1.0 - self.release_coef);
        // Sustain level changes glide over ~10 ms instead of jumping.
        self.sustain_coef = 1.0 - (-1.0 / (0.01 * sr)).exp();
    }

    /// Starts (or restarts) the attack from the current level.
    pub fn note_on(&mut self) {
        self.stage = Stage::Attack;
    }

    /// Starts the release from the current level.
    pub fn note_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    /// Forces the envelope to silence.
    pub fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    pub fn is_releasing(&self) -> bool {
        self.stage == Stage::Release
    }

    /// Advances one sample and returns the new level.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                if self.linear_attack {
                    self.level += self.attack_inc;
                } else {
                    self.level = self.attack_base + self.level * self.attack_coef;
                }
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = self.decay_base + self.level * self.decay_coef;
                if self.level <= self.sustain {
                    self.level = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {
                self.level += (self.sustain - self.level) * self.sustain_coef;
            }
            Stage::Release => {
                self.level = self.release_base + self.level * self.release_coef;
                if self.level <= IDLE_LEVEL {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

/// Percussive envelope: a short linear attack followed by an exponential
/// fall. `t60` is the time for the level to fall by 60 dB.
#[derive(Clone, Copy, Debug)]
pub struct Decay {
    level: f32,
    peak: f32,
    coef: f32,
    attack_inc: f32,
    rising: bool,
}

/// Converts a 60 dB fall time to a per-sample multiplier.
pub fn t60_to_coef(t60: f32, sample_rate: f32) -> f32 {
    let samples = (t60 * sample_rate).max(1.0);
    (-6.907_755 / samples).exp()
}

impl Decay {
    pub const fn new() -> Decay {
        Decay {
            level: 0.0,
            peak: 0.0,
            coef: 0.0,
            attack_inc: 1.0,
            rising: false,
        }
    }

    /// Changes the decay time without restarting.
    pub fn set_t60(&mut self, t60: f32, sample_rate: f32) {
        self.coef = t60_to_coef(t60, sample_rate);
    }

    /// Starts a hit at `peak` with a linear attack of `attack` seconds.
    pub fn trigger(&mut self, peak: f32, attack: f32, sample_rate: f32) {
        self.peak = peak;
        let n = (attack * sample_rate).max(1.0);
        self.attack_inc = peak / n;
        self.rising = true;
    }

    pub fn reset(&mut self) {
        self.level = 0.0;
        self.rising = false;
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.rising {
            self.level += self.attack_inc;
            if self.level >= self.peak {
                self.level = self.peak;
                self.rising = false;
            }
        } else {
            self.level *= self.coef;
            if self.level < 1.0e-7 {
                self.level = 0.0;
            }
        }
        self.level
    }

    pub fn is_idle(&self) -> bool {
        !self.rising && self.level == 0.0
    }
}

impl Default for Decay {
    fn default() -> Self {
        Decay::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn run_until(env: &mut Adsr, limit: usize, mut stop: impl FnMut(f32) -> bool) -> usize {
        for i in 0..limit {
            let v = env.tick();
            if stop(v) {
                return i + 1;
            }
        }
        limit
    }

    #[test]
    fn linear_attack_takes_the_set_time() {
        let mut e = Adsr::new(SR);
        e.set(0.010, 0.1, 0.5, 0.1, true);
        e.note_on();
        let n = run_until(&mut e, 10_000, |v| v >= 1.0);
        let expect = (0.010 * SR) as usize;
        assert!(
            n.abs_diff(expect) <= 2,
            "attack took {n}, expected {expect}"
        );
    }

    #[test]
    fn exponential_attack_is_close_to_set_time() {
        let mut e = Adsr::new(SR);
        e.set(0.020, 0.1, 0.5, 0.1, false);
        e.note_on();
        let n = run_until(&mut e, 100_000, |v| v >= 1.0) as f32 / SR;
        assert!((0.010..0.030).contains(&n), "attack took {n} s");
    }

    #[test]
    fn decays_to_sustain_and_holds() {
        let mut e = Adsr::new(SR);
        e.set(0.001, 0.05, 0.4, 0.1, false);
        e.note_on();
        for _ in 0..(SR as usize) {
            e.tick();
        }
        assert_eq!(e.stage(), Stage::Sustain);
        assert!((e.level() - 0.4).abs() < 1e-3);
    }

    #[test]
    fn release_ends_within_set_time() {
        let mut e = Adsr::new(SR);
        e.set(0.001, 0.05, 1.0, 0.2, false);
        e.note_on();
        for _ in 0..(0.1 * SR) as usize {
            e.tick();
        }
        e.note_off();
        let n = run_until(&mut e, 100_000, |_| false);
        assert_eq!(n, 100_000);
        assert!(e.is_idle());
        let mut e2 = e;
        e2.note_on();
        for _ in 0..(0.1 * SR) as usize {
            e2.tick();
        }
        e2.note_off();
        let n = run_until(&mut e2, 100_000, |v| v == 0.0) as f32 / SR;
        assert!(n <= 0.21, "release took {n} s");
        assert!(n >= 0.12, "release too quick: {n} s");
    }

    #[test]
    fn retrigger_never_jumps() {
        let mut e = Adsr::new(SR);
        e.set(0.005, 0.2, 0.6, 0.3, false);
        e.note_on();
        for _ in 0..4_000 {
            e.tick();
        }
        e.note_off();
        for _ in 0..2_000 {
            e.tick();
        }
        let before = e.level();
        assert!(before > 0.05);
        e.note_on();
        let mut prev = before;
        for _ in 0..2_000 {
            let v = e.tick();
            assert!((v - prev).abs() < 0.02, "jump {prev} -> {v}");
            prev = v;
        }
    }

    #[test]
    fn sustain_change_glides() {
        let mut e = Adsr::new(SR);
        e.set(0.001, 0.01, 0.8, 0.1, false);
        e.note_on();
        for _ in 0..10_000 {
            e.tick();
        }
        e.set(0.001, 0.01, 0.2, 0.1, false);
        let first = e.tick();
        assert!(first > 0.75, "level jumped to {first}");
    }

    #[test]
    fn decay_envelope_hits_60db_in_time() {
        let mut d = Decay::new();
        d.set_t60(0.5, SR);
        d.trigger(1.0, 0.0005, SR);
        let mut n = 0;
        while d.tick() > 1.0e-3 || n < 100 {
            n += 1;
            if n > 100_000 {
                break;
            }
        }
        let t = n as f32 / SR;
        assert!((t - 0.5).abs() < 0.05, "t60 {t}");
    }
}
