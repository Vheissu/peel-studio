//! Analog-style building blocks shared by the Poly and Bass synths.
//!
//! * A minBLEP oscillator ([`BlOsc`]) for alias-free saw and pulse waves.
//! * Zero-delay-feedback state-variable and ladder filters with soft
//!   saturation, whose coefficients glide linearly across each control
//!   interval so fast modulation never steps ([`SatSvf`], [`SatLadder`]).
//! * A 2x halfband oversampler ([`Up2`], [`Down2`]) for nonlinear stages.
//! * Envelopes with an adjustable exponential curve ([`Env`], [`PitchEnv`]).
//! * Slow random drift ([`Drift`]), a DC blocker and waveshapers.

use peel_dsp::envelope::Stage;
use peel_dsp::filter::{resonance_to_q, tanh_approx};
use peel_dsp::noise::Noise;
use peel_dsp::osc::{self, Waveform};

use crate::minblep_table::{BL_DELAY, BL_K, BL_OS, MINBLEP_RESIDUAL};

// ---------------------------------------------------------------- oscillator

const BL_MASK: usize = BL_K - 1;

/// Phase-accumulator oscillator with minBLEP anti-aliasing for saw and pulse
/// waves.
///
/// Every discontinuity adds a precomputed band-limited residual (see
/// `minblep_table`) into a small ring buffer at its exact fractional
/// position, so the output stays clean (below about -75 dBr aliasing at C8)
/// without oversampling and costs next to nothing per sample. Sine and
/// triangle come from `peel_dsp`. Because the residual is minimum phase it
/// only ever follows an edge, so pitch can be modulated freely.
#[derive(Clone, Copy, Debug)]
pub struct BlOsc {
    pub phase: f32,
    ring: [f32; BL_K],
    pos: usize,
}

impl Default for BlOsc {
    fn default() -> Self {
        BlOsc::new(0.0)
    }
}

impl BlOsc {
    pub const fn new(phase: f32) -> BlOsc {
        BlOsc {
            phase,
            ring: [0.0; BL_K],
            pos: 0,
        }
    }

    /// Restarts at `phase` with no pending edges.
    pub fn reset(&mut self, phase: f32) {
        self.phase = phase;
        self.ring = [0.0; BL_K];
        self.pos = 0;
    }

    /// Adds a step of height `amp` that happened `d` samples (0..1) before the
    /// next sample to be produced.
    #[inline(always)]
    fn edge(&mut self, amp: f32, d: f32) {
        let t = d * BL_OS as f32;
        let j0 = t as usize;
        let fr = t - j0 as f32;
        for (p, k) in (self.pos..).zip(0..BL_K) {
            let j = k * BL_OS + j0;
            let a = MINBLEP_RESIDUAL[j];
            let b = MINBLEP_RESIDUAL[j + 1];
            // The naive edge is delayed by BL_DELAY: until then the residual
            // is the band-limited step on its own.
            let naive = if (k as f32 + d) < BL_DELAY { 1.0 } else { 0.0 };
            self.ring[p & BL_MASK] += amp * (a + (b - a) * fr + naive);
        }
    }

    /// Renders one sample of `w` and advances by `dt` (phase per sample).
    #[inline]
    pub fn tick(&mut self, w: Waveform, dt: f32, pw: f32) -> f32 {
        let dt = dt.clamp(0.0, 0.45);
        match w {
            Waveform::Saw => {
                // The naive ramp is delayed like the band-limited edges, so
                // the pair stays balanced (zero DC at any pitch).
                let q = self.phase - BL_DELAY * dt;
                let q = q - q.floor();
                let v = 2.0 * q - 1.0 + self.ring[self.pos];
                self.ring[self.pos] = 0.0;
                self.pos = (self.pos + 1) & BL_MASK;
                let p = self.phase + dt;
                if p >= 1.0 {
                    let p = p - 1.0;
                    self.phase = p;
                    self.edge(-2.0, (p / dt).min(0.9999));
                } else {
                    self.phase = p;
                }
                v
            }
            Waveform::Square => {
                let q = self.phase - BL_DELAY * dt;
                let q = q - q.floor();
                let naive = if q < pw { 1.0 } else { -1.0 };
                let v = naive + self.ring[self.pos];
                self.ring[self.pos] = 0.0;
                self.pos = (self.pos + 1) & BL_MASK;
                let p0 = self.phase;
                let mut p = p0 + dt;
                if p0 < pw && p >= pw {
                    self.edge(-2.0, ((p - pw) / dt).min(0.9999));
                }
                if p >= 1.0 {
                    p -= 1.0;
                    self.edge(2.0, (p / dt).min(0.9999));
                }
                self.phase = p;
                v
            }
            _ => {
                let v = osc::eval(w, self.phase, dt, pw);
                advance(&mut self.phase, dt);
                v
            }
        }
    }
}

/// Advances a phase by `dt`, wrapping into `0..1`.
#[inline(always)]
pub fn advance(phase: &mut f32, dt: f32) {
    *phase += dt;
    if *phase >= 1.0 {
        *phase -= 1.0;
        if *phase >= 1.0 {
            *phase = 0.0;
        }
    }
}

// ------------------------------------------------------------------- filters

/// Soft state limiter: transparent for small values, saturates at `lim`.
#[inline(always)]
fn soft_state(x: f32, lim: f32) -> f32 {
    tanh_approx(x * (1.0 / lim)) * lim
}

#[inline(always)]
fn flush(x: f32) -> f32 {
    if x.abs() < 1.0e-20 { 0.0 } else { x }
}

/// Target coefficients of [`SatSvf`].
#[derive(Clone, Copy, Debug)]
pub struct SvfCoef {
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
    drive: f32,
    comp: f32,
}

impl SvfCoef {
    pub fn new(cutoff_hz: f32, resonance: f32, drive: f32, sr: f32) -> SvfCoef {
        let fc = cutoff_hz.clamp(10.0, 0.45 * sr);
        let g = (std::f32::consts::PI * fc / sr).tan();
        let k = 1.0 / resonance_to_q(resonance).clamp(0.3, 200.0);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let drive = drive.clamp(1.0, 30.0);
        SvfCoef {
            a1,
            a2: g * a1,
            a3: g * g * a1,
            k,
            drive,
            comp: 1.0 / drive.sqrt(),
        }
    }
}

/// 12 dB/octave trapezoidal state-variable filter (Simper) with a soft limit
/// on its integrator states so heavy resonance and drive stay musical.
#[derive(Clone, Copy, Debug)]
pub struct SatSvf {
    ic1: f32,
    ic2: f32,
    c: SvfCoef,
    d: [f32; 4],
    span: f32,
}

impl Default for SatSvf {
    fn default() -> Self {
        SatSvf::new()
    }
}

impl SatSvf {
    pub fn new() -> SatSvf {
        SatSvf {
            ic1: 0.0,
            ic2: 0.0,
            c: SvfCoef::new(1_000.0, 0.1, 1.0, 48_000.0),
            d: [0.0; 4],
            span: 1.0,
        }
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// Completes a pending glide at once (a new voice starts on its first
    /// coefficients, not on stale ones).
    pub fn settle(&mut self) {
        self.c.a1 += self.d[0] * self.span;
        self.c.a2 += self.d[1] * self.span;
        self.c.a3 += self.d[2] * self.span;
        self.c.k += self.d[3] * self.span;
        self.d = [0.0; 4];
    }

    /// Glides to `c` linearly over `samples` samples.
    pub fn glide_to(&mut self, c: SvfCoef, samples: f32) {
        let inv = 1.0 / samples.max(1.0);
        self.span = samples.max(1.0);
        self.d = [
            (c.a1 - self.c.a1) * inv,
            (c.a2 - self.c.a2) * inv,
            (c.a3 - self.c.a3) * inv,
            (c.k - self.c.k) * inv,
        ];
        // Drive is already smooth (it comes from a ramped parameter).
        self.c.drive = c.drive;
        self.c.comp = c.comp;
    }

    /// Filters one sample. `mode`: 0 low-pass, 1 band-pass (unity peak),
    /// 2 high-pass, otherwise notch.
    #[inline]
    pub fn tick(&mut self, x: f32, mode: usize) -> f32 {
        let c = &mut self.c;
        c.a1 += self.d[0];
        c.a2 += self.d[1];
        c.a3 += self.d[2];
        c.k += self.d[3];
        let x = x * c.drive;
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = flush(soft_state(2.0 * v1 - self.ic1, 6.0));
        self.ic2 = flush(soft_state(2.0 * v2 - self.ic2, 8.0));
        let kv1 = c.k * v1;
        let y = match mode {
            0 => v2,
            1 => kv1,
            2 => x - kv1 - v2,
            _ => x - kv1,
        };
        y * c.comp
    }
}

/// Level at which the ladder's stages saturate. High enough that normal
/// signals pass almost linearly (so there is no aliasing without
/// oversampling); drive and resonance push into it.
const LADDER_LIMIT: f32 = 4.0;

/// Target coefficients of [`SatLadder`].
#[derive(Clone, Copy, Debug)]
pub struct LadderCoef {
    g: f32,
    k: f32,
    drive: f32,
    comp: f32,
}

impl LadderCoef {
    /// `resonance` and `drive` follow `peel_dsp::filter::Ladder`: 0..1 and
    /// 1.. (1 is clean).
    pub fn new(cutoff_hz: f32, resonance: f32, drive: f32, sr: f32) -> LadderCoef {
        let fc = cutoff_hz.clamp(10.0, 0.45 * sr);
        let g = (std::f32::consts::PI * fc / sr).tan();
        let res = resonance.clamp(0.0, 1.0);
        let drive = drive.clamp(1.0, 30.0);
        LadderCoef {
            g: g / (1.0 + g),
            // Slightly past the linear oscillation threshold (4): the
            // saturators, not the feedback gain, set the level.
            k: 4.2 * res * (1.0 + 0.06 * res),
            drive,
            comp: (1.0 + 0.5 * res) / drive.sqrt(),
        }
    }
}

/// Four-pole (24 dB/octave) ladder low-pass: the feedback loop is solved
/// exactly (zero delay) and every stage saturates, so the sound thickens
/// smoothly with level and self-oscillates cleanly near full resonance.
#[derive(Clone, Copy, Debug)]
pub struct SatLadder {
    s: [f32; 4],
    c: LadderCoef,
    dg: f32,
    dk: f32,
    span: f32,
}

impl Default for SatLadder {
    fn default() -> Self {
        SatLadder::new()
    }
}

impl SatLadder {
    pub fn new() -> SatLadder {
        SatLadder {
            s: [0.0; 4],
            c: LadderCoef::new(1_000.0, 0.0, 1.0, 48_000.0),
            dg: 0.0,
            dk: 0.0,
            span: 1.0,
        }
    }

    pub fn reset(&mut self) {
        self.s = [0.0; 4];
    }

    /// Completes a pending glide at once.
    pub fn settle(&mut self) {
        self.c.g += self.dg * self.span;
        self.c.k += self.dk * self.span;
        self.dg = 0.0;
        self.dk = 0.0;
    }

    pub fn glide_to(&mut self, c: LadderCoef, samples: f32) {
        let inv = 1.0 / samples.max(1.0);
        self.span = samples.max(1.0);
        self.dg = (c.g - self.c.g) * inv;
        self.dk = (c.k - self.c.k) * inv;
        self.c.drive = c.drive;
        self.c.comp = c.comp;
    }

    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let c = &mut self.c;
        c.g += self.dg;
        c.k += self.dk;
        let g = c.g;
        let g2 = g * g;
        let g4 = g2 * g2;
        // Linear prediction of what the last stage will feed back.
        let pred = (1.0 - g) * (g2 * g * self.s[0] + g2 * self.s[1] + g * self.s[2] + self.s[3]);
        let u = (x * c.drive - c.k * pred) / (1.0 + c.k * g4);
        let mut input = soft_state(u, LADDER_LIMIT);
        for (i, st) in self.s.iter_mut().enumerate() {
            let v = (input - *st) * g;
            let y = v + *st;
            *st = flush(y + v);
            input = if i < 3 {
                soft_state(y, LADDER_LIMIT)
            } else {
                y
            };
        }
        input * c.comp
    }
}

// -------------------------------------------------------------- oversampling

/// Halfband FIR taps `h[2j+1]` (Kaiser window, 74 dB stopband, flat to 0.17 of
/// the oversampled rate).
const HB: [f32; 8] = [
    0.314_303_88,
    -0.094_594_33,
    0.046_054_72,
    -0.023_740_32,
    0.011_623_75,
    -0.005_036_99,
    0.001_760_13,
    -0.000_394_36,
];
const HB_J: usize = HB.len();

/// 2x upsampler (halfband interpolation filter). Latency is `HB_J` input
/// samples.
#[derive(Clone, Copy, Debug)]
pub struct Up2 {
    hist: [f32; 2 * HB_J],
    pos: usize,
}

impl Default for Up2 {
    fn default() -> Self {
        Up2::new()
    }
}

impl Up2 {
    pub const fn new() -> Up2 {
        Up2 {
            hist: [0.0; 2 * HB_J],
            pos: 0,
        }
    }

    pub fn reset(&mut self) {
        self.hist = [0.0; 2 * HB_J];
    }

    /// Takes one sample, returns the two samples at twice the rate.
    #[inline]
    pub fn tick(&mut self, x: f32) -> (f32, f32) {
        const N: usize = 2 * HB_J;
        self.pos = (self.pos + N - 1) % N;
        self.hist[self.pos] = x;
        // Lag `l` lives at `(pos + l) % N`.
        let at = |l: usize| self.hist[(self.pos + l) % N];
        let mut acc = 0.0;
        for (j, h) in HB.iter().enumerate() {
            acc += h * (at(HB_J + j) + at(HB_J - 1 - j));
        }
        (at(HB_J), 2.0 * acc)
    }
}

/// 2x downsampler (halfband decimation filter). Latency is `2 * HB_J - 1`
/// high-rate samples.
#[derive(Clone, Copy, Debug)]
pub struct Down2 {
    buf: [f32; 64],
    pos: usize,
}

impl Default for Down2 {
    fn default() -> Self {
        Down2::new()
    }
}

impl Down2 {
    pub const fn new() -> Down2 {
        Down2 {
            buf: [0.0; 64],
            pos: 0,
        }
    }

    pub fn reset(&mut self) {
        self.buf = [0.0; 64];
    }

    /// Takes two consecutive high-rate samples, returns one low-rate sample.
    #[inline]
    pub fn tick(&mut self, a: f32, b: f32) -> f32 {
        self.pos = (self.pos + 63) & 63;
        self.buf[self.pos] = a;
        self.pos = (self.pos + 63) & 63;
        self.buf[self.pos] = b;
        let at = |l: usize| self.buf[(self.pos + l) & 63];
        let c = 2 * HB_J - 1;
        let mut acc = 0.5 * at(c);
        for (j, h) in HB.iter().enumerate() {
            let o = 2 * j + 1;
            acc += h * (at(c - o) + at(c + o));
        }
        acc
    }
}

// ----------------------------------------------------------------- envelopes

/// Overshoot ratio for the attack stage.
const ATTACK_RATIO: f32 = 0.3;
const IDLE_LEVEL: f32 = 1.0e-5;

fn exp_coef(samples: f32, ratio: f32) -> f32 {
    (-((1.0 + ratio) / ratio).ln() / samples.max(1.0)).exp()
}

/// ADSR with exponential segments whose curvature is adjustable.
///
/// `curve` runs 0..1: 0 gives near-linear segments, 0.5 an analog-style
/// curve, 1 a very fast initial fall that settles slowly, the shape of a
/// snappy pluck. `decay` and `release` are the times to reach
/// silence.
#[derive(Clone, Copy, Debug)]
pub struct Env {
    stage: Stage,
    level: f32,
    sr: f32,
    sustain: f32,
    attack_coef: f32,
    attack_base: f32,
    decay_coef: f32,
    decay_base: f32,
    release_coef: f32,
    release_base: f32,
    sustain_coef: f32,
}

impl Env {
    pub fn new(sr: f32) -> Env {
        let mut e = Env {
            stage: Stage::Idle,
            level: 0.0,
            sr: sr.max(1.0),
            sustain: 0.7,
            attack_coef: 0.0,
            attack_base: 0.0,
            decay_coef: 0.0,
            decay_base: 0.0,
            release_coef: 0.0,
            release_base: 0.0,
            sustain_coef: 0.0,
        };
        e.set(0.005, 0.2, 0.7, 0.3, 0.5);
        e
    }

    pub fn set(&mut self, attack: f32, decay: f32, sustain: f32, release: f32, curve: f32) {
        let sr = self.sr;
        let a = attack.max(0.0005) * sr;
        let d = decay.max(0.0005) * sr;
        let r = release.max(0.0005) * sr;
        let curve = curve.clamp(0.0, 1.0);
        let ratio = 10.0f32.powf(-1.0 - 2.7 * curve);
        // Low curve: near-linear attack (risers); high: a fast concave one.
        let attack_ratio = ATTACK_RATIO * 10.0f32.powf((0.5 - curve) * 3.0);
        self.sustain = sustain.clamp(0.0, 1.0);
        self.attack_coef = exp_coef(a, attack_ratio);
        self.attack_base = (1.0 + attack_ratio) * (1.0 - self.attack_coef);
        self.decay_coef = exp_coef(d, ratio);
        self.decay_base = (self.sustain - ratio) * (1.0 - self.decay_coef);
        self.release_coef = exp_coef(r, ratio);
        self.release_base = -ratio * (1.0 - self.release_coef);
        self.sustain_coef = 1.0 - (-1.0 / (0.01 * sr)).exp();
    }

    /// Starts (or restarts) the attack from the current level.
    pub fn note_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub fn note_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

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

    #[inline]
    pub fn tick(&mut self) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.level = self.attack_base + self.level * self.attack_coef;
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

/// Exponentially falling 1 -> 0 envelope for pitch dives.
#[derive(Clone, Copy, Debug)]
pub struct PitchEnv {
    level: f32,
    coef: f32,
}

impl PitchEnv {
    pub const fn new() -> PitchEnv {
        PitchEnv {
            level: 0.0,
            coef: 0.999,
        }
    }

    /// `t60`: seconds to fall by 60 dB.
    pub fn set_decay(&mut self, t60: f32, sr: f32) {
        self.coef = (-6.907_755 / (t60.max(0.002) * sr)).exp();
    }

    pub fn trigger(&mut self) {
        self.level = 1.0;
    }

    pub fn reset(&mut self) {
        self.level = 0.0;
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        self.level *= self.coef;
        if self.level < 1.0e-5 {
            self.level = 0.0;
        }
        self.level
    }
}

impl Default for PitchEnv {
    fn default() -> Self {
        PitchEnv::new()
    }
}

// --------------------------------------------------------------------- misc

/// Slow random wander in `-1..1`, stepped once per control interval.
#[derive(Clone, Copy, Debug)]
pub struct Drift {
    value: f32,
    target: f32,
    hold: u32,
    rng: Noise,
}

impl Drift {
    pub fn new(seed: u32) -> Drift {
        Drift {
            value: 0.0,
            target: 0.0,
            hold: 0,
            rng: Noise::new(seed),
        }
    }

    pub fn reseed(&mut self, seed: u32) {
        self.rng.reseed(seed);
        self.value = self.rng.white() * 0.5;
        self.target = self.rng.white();
        self.hold = 0;
    }

    /// `slew` is the per-call approach rate, `hold_ticks` the mean number of
    /// calls between new random targets.
    #[inline]
    pub fn tick(&mut self, slew: f32, hold_ticks: u32) -> f32 {
        if self.hold == 0 {
            self.target = self.rng.white();
            self.hold = (hold_ticks as f32 * (0.5 + self.rng.uniform())) as u32 + 1;
        }
        self.hold -= 1;
        self.value += (self.target - self.value) * slew;
        self.value
    }
}

/// First-order DC blocker.
#[derive(Clone, Copy, Debug, Default)]
pub struct DcBlock {
    x1: f32,
    y1: f32,
    r: f32,
}

impl DcBlock {
    pub fn new(sr: f32) -> DcBlock {
        DcBlock {
            x1: 0.0,
            y1: 0.0,
            r: 1.0 - std::f32::consts::TAU * 8.0 / sr,
        }
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = flush(y);
        y
    }
}

/// Waveshapers for the bass distortion: soft clip, hard clip, wave folding.
/// All map a full-scale input to roughly full scale for any `gain`.
#[inline]
pub fn shape_by(kind: usize, x: f32, gain: f32) -> f32 {
    let v = x * gain;
    match kind {
        0 => tanh_approx(v) / tanh_approx(gain),
        1 => hard_knee(v) / hard_knee(gain),
        _ => osc::fast_sin(v * 0.25),
    }
}

/// Clip at +/-1 with a smooth cubic knee (reaches 1 at 1.5).
#[inline]
fn hard_knee(v: f32) -> f32 {
    let a = v.clamp(-1.5, 1.5);
    a - a * a * a * (4.0 / 27.0)
}

/// Bus protection: untouched below 0.8, then a smooth knee towards a ceiling
/// of 1.6, so big chord pile-ups round off instead of hard clipping while
/// normal levels stay perfectly clean.
#[inline]
pub fn soft_ceiling(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.8 {
        x
    } else {
        (0.8 + 0.8 * tanh_approx((a - 0.8) * 1.25)).copysign(x)
    }
}

/// Semitone offset to a frequency ratio.
#[inline]
pub fn ratio(st: f32) -> f32 {
    (st * (std::f32::consts::LN_2 / 12.0)).exp()
}

/// Ratio for a small offset in cents (linear approximation).
#[inline]
pub fn cents_ratio(cents: f32) -> f32 {
    1.0 + cents * 5.776_8e-4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halfband_round_trip_is_unity_gain_below_the_edge() {
        let mut up = Up2::new();
        let mut down = Down2::new();
        let n = 4096;
        let f = 0.1; // cycles per sample
        let mut out = vec![0.0f32; n];
        for (i, o) in out.iter_mut().enumerate() {
            let x = (std::f32::consts::TAU * f * i as f32).sin();
            let (a, b) = up.tick(x);
            *o = down.tick(a, b);
        }
        let rms = (out[1024..].iter().map(|v| v * v).sum::<f32>() / (n - 1024) as f32).sqrt();
        assert!(
            (rms - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01,
            "rms {rms}"
        );
    }

    #[test]
    fn ladder_and_svf_stay_bounded_at_full_resonance_with_fast_modulation() {
        let sr = 48_000.0;
        let mut lad = SatLadder::new();
        let mut svf = SatSvf::new();
        let mut rng = Noise::new(3);
        let mut peak = 0.0f32;
        for i in 0..96_000 {
            if i % 16 == 0 {
                let fc = 60.0 * (1.0 + 0.5 * rng.uniform() * 300.0);
                lad.glide_to(LadderCoef::new(fc, 1.0, 4.0, sr), 16.0);
                svf.glide_to(SvfCoef::new(fc, 1.0, 4.0, sr), 16.0);
            }
            let x = 0.5 * rng.white();
            let a = lad.tick(x);
            let b = svf.tick(x, 0);
            assert!(a.is_finite() && b.is_finite());
            peak = peak.max(a.abs()).max(b.abs());
        }
        assert!(peak < 8.0, "peak {peak}");
    }

    /// Gain of a filter at `f` Hz, measured with a small sine (linear region).
    fn gain_at(mut f: impl FnMut(f32) -> f32, hz: f32, sr: f32) -> f32 {
        let n = 24_000;
        let mut acc = 0.0f64;
        let mut cnt = 0usize;
        for i in 0..n {
            let x = 0.01 * (std::f32::consts::TAU * hz * i as f32 / sr).sin();
            let y = f(x);
            if i > n / 2 {
                acc += (y as f64) * (y as f64);
                cnt += 1;
            }
        }
        ((acc / cnt as f64).sqrt() / (0.01 / 2.0f64.sqrt())) as f32
    }

    #[test]
    fn filters_put_their_cutoff_where_asked_at_any_pitch() {
        let sr = 48_000.0;
        for fc in [120.0, 1_000.0, 5_000.0, 12_000.0] {
            // Ladder: four poles, each -3 dB at the cutoff -> -12 dB.
            let mut lad = SatLadder::new();
            lad.reset();
            let c = LadderCoef::new(fc, 0.0, 1.0, sr);
            lad.glide_to(c, 1.0);
            lad.settle();
            let g = gain_at(|x| lad.tick(x), fc, sr);
            let db = 20.0 * g.log10();
            assert!((db + 12.04).abs() < 0.8, "ladder {fc} Hz: {db} dB");
            // SVF: gain at the cutoff equals Q (0.55 at zero resonance).
            let mut svf = SatSvf::new();
            let c = SvfCoef::new(fc, 0.0, 1.0, sr);
            svf.glide_to(c, 1.0);
            svf.settle();
            let g = gain_at(|x| svf.tick(x, 0), fc, sr);
            assert!((g - 0.55).abs() < 0.04, "svf {fc} Hz: {g}");
        }
    }

    #[test]
    fn ladder_self_oscillates_at_its_cutoff() {
        let sr = 48_000.0;
        for fc in [200.0, 800.0, 3_000.0] {
            let mut lad = SatLadder::new();
            let c = LadderCoef::new(fc, 1.0, 1.0, sr);
            lad.glide_to(c, 1.0);
            lad.settle();
            let mut out = Vec::new();
            for i in 0..48_000 {
                out.push(lad.tick(if i == 0 { 0.5 } else { 0.0 }));
            }
            let tail = &out[24_000..];
            assert!(
                tail.iter().fold(0.0f32, |m, v| m.max(v.abs())) > 0.2,
                "{fc} Hz: did not sustain"
            );
            let crossings = tail
                .windows(2)
                .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
                .count();
            let hz = crossings as f32 / (tail.len() as f32 / sr);
            assert!((hz / fc - 1.0).abs() < 0.03, "{fc} Hz rang at {hz}");
        }
    }

    /// Radix-2 FFT power spectrum with a 4-term Blackman-Harris window.
    fn power_spectrum(x: &[f32]) -> Vec<f64> {
        let n = x.len();
        let mut re: Vec<f64> = x
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let t = std::f64::consts::TAU * i as f64 / n as f64;
                let w = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos()
                    - 0.01168 * (3.0 * t).cos();
                v as f64 * w
            })
            .collect();
        let mut im = vec![0.0f64; n];
        let mut j = 0;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j ^= bit;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let ang = -std::f64::consts::TAU / len as f64;
            for s in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                    let (a, b) = (s + k, s + k + len / 2);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
        (0..n / 2).map(|i| re[i] * re[i] + im[i] * im[i]).collect()
    }

    /// Worst inharmonic line relative to the fundamental, in dB.
    fn worst_alias_db(x: &[f32], f0: f32, sr: f32) -> f32 {
        let sp = power_spectrum(x);
        let n = x.len();
        let df = sr / n as f32;
        let mut harmonic = vec![false; sp.len()];
        let mut k = 1;
        while k as f32 * f0 < sr / 2.0 + 100.0 {
            let c = (k as f32 * f0 / df).round() as isize;
            for b in (c - 7)..=(c + 7) {
                if b >= 0 && (b as usize) < harmonic.len() {
                    harmonic[b as usize] = true;
                }
            }
            k += 1;
        }
        let c0 = (f0 / df).round() as usize;
        let fund = sp[c0 - 5..=c0 + 5].iter().cloned().fold(0.0, f64::max);
        let mut worst = 0.0f64;
        for b in (100.0 / df) as usize..sp.len() - 1 {
            if !harmonic[b] && sp[b] > sp[b - 1] && sp[b] >= sp[b + 1] {
                worst = worst.max(sp[b]);
            }
        }
        (10.0 * (worst / fund).log10()) as f32
    }

    #[test]
    fn minblep_saw_and_pulse_alias_far_less_than_polyblep() {
        let sr = 48_000.0;
        let n = 16_384;
        for key in [72.0f32, 84.0, 96.0, 105.0] {
            let f0 = 440.0 * ((key - 69.0) / 12.0).exp2();
            let dt = f0 / sr;
            let mut old = peel_dsp::osc::Osc::new();
            let mut new = BlOsc::new(0.0);
            let mut pulse = BlOsc::new(0.0);
            let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
            for _ in 0..n + 4_096 {
                a.push(old.saw(dt));
                b.push(new.tick(Waveform::Saw, dt, 0.5));
                c.push(pulse.tick(Waveform::Square, dt, 0.3));
            }
            let old_db = worst_alias_db(&a[4_096..], f0, sr);
            let new_db = worst_alias_db(&b[4_096..], f0, sr);
            let pulse_db = worst_alias_db(&c[4_096..], f0, sr);
            assert!(new_db < -68.0, "key {key}: minBLEP saw alias {new_db} dB");
            assert!(
                pulse_db < -64.0,
                "key {key}: minBLEP pulse alias {pulse_db} dB"
            );
            assert!(
                new_db < old_db - 20.0,
                "key {key}: {new_db} dB vs PolyBLEP {old_db} dB"
            );
        }
    }

    #[test]
    fn minblep_saw_has_the_right_amplitude_and_no_dc() {
        let sr = 48_000.0;
        let dt = 220.0 / sr;
        let mut o = BlOsc::new(0.3);
        let v: Vec<f32> = (0..48_000)
            .map(|_| o.tick(Waveform::Saw, dt, 0.5))
            .collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        let rms = (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        assert!(mean.abs() < 0.01, "dc {mean}");
        // A unit saw has rms 1/sqrt(3).
        assert!((rms - 0.5774).abs() < 0.02, "rms {rms}");
    }

    #[test]
    fn curved_envelope_falls_faster_at_the_start_when_curve_is_high() {
        let sr = 48_000.0;
        let level_after = |curve: f32, t: f32| {
            let mut e = Env::new(sr);
            e.set(0.001, 0.5, 0.0, 0.2, curve);
            e.note_on();
            let mut v = 0.0;
            for _ in 0..(t * sr) as usize {
                v = e.tick();
            }
            v
        };
        // Same nominal decay time; the high curve is lower early on.
        assert!(level_after(0.9, 0.1) < level_after(0.2, 0.1) * 0.5);
    }
}
