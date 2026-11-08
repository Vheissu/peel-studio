//! Saturation with four curves, oversampling, tone and mix.
//!
//! * **Curves**: *Tape* is a symmetric `tanh` soft clip, *Tube* is an
//!   asymmetric `tanh` with bias (adds even harmonics, DC is removed
//!   afterwards), *Hard Clip* limits at +/-1 and *Fold* reflects the signal
//!   back from +/-1 like a wavefolder. All have unity gain for small signals.
//! * **Oversampling**: Off, 2x or 4x with linear-phase half-band filters
//!   (75 dB or better rejection above 0.58 of the base Nyquist), so
//!   harmonics created above Nyquist are filtered out before they can fold
//!   back as aliasing. At 2x and 4x each curve also runs through first-order
//!   antiderivative anti-aliasing (ADAA), which smooths the corners of the
//!   hard clip and the folds of the wavefolder before the filters see them.
//!   "Off" is the raw curve and will alias; it exists for CPU savings and
//!   deliberately lo-fi use.
//! * **Latency** is fixed at [`LATENCY`] samples in every oversampling mode
//!   (shorter filters are padded), so switching quality never changes the
//!   delay the host compensates. The dry signal in the mix is delayed to
//!   match.
//! * **Tone** tilts the spectrum after saturation: negative is darker,
//!   positive brighter, pivoting around 1 kHz.

use peel_dsp::biquad::{Coeffs, DcBlocker, FilterKind, StereoBiquad};
use peel_dsp::delay::DelayLine;
use peel_dsp::device::{DeviceDescriptor, DeviceKind, ProcessContext};
use peel_dsp::param::{ParamDescriptor, ParamId, Unit};
use peel_dsp::preset::Preset;
use peel_dsp::util::{Smoothed, db_to_gain};

use crate::common::{Fx, ParamStore, RAMP_SECONDS, SilenceTracker, impl_device, refresh_all};
use crate::oversample::{Decimator2, Interpolator2, stage_latency};

pub const ID_DRIVE: ParamId = 0;
pub const ID_TYPE: ParamId = 1;
pub const ID_OVERSAMPLING: ParamId = 2;
pub const ID_TONE: ParamId = 3;
pub const ID_MIX: ParamId = 4;
pub const ID_OUTPUT: ParamId = 5;

pub const TYPE_TAPE: u32 = 0;
pub const TYPE_TUBE: u32 = 1;
pub const TYPE_HARD: u32 = 2;
pub const TYPE_FOLD: u32 = 3;

pub const OS_OFF: u32 = 0;
pub const OS_2X: u32 = 1;
pub const OS_4X: u32 = 2;

const TYPES: &[&str] = &["Tape", "Tube", "Hard Clip", "Fold"];
const OVERSAMPLING: &[&str] = &["Off", "2x", "4x"];

/// Outer half-band filter length (base rate to 2x).
const TAPS_OUTER: usize = 63;
/// Inner half-band filter length (2x to 4x).
const TAPS_INNER: usize = 21;

/// Fixed processing latency in samples, the same for every oversampling mode.
pub const LATENCY: u32 = (stage_latency(TAPS_OUTER) + stage_latency(TAPS_INNER) / 2) as u32;
/// Padding needed in 2x mode to reach [`LATENCY`].
const PAD_2X: usize = LATENCY as usize - stage_latency(TAPS_OUTER);

static PARAMS: [ParamDescriptor; 6] = [
    ParamDescriptor::linear(
        ID_DRIVE,
        "drive",
        "Drive",
        "Drive",
        0.0,
        40.0,
        12.0,
        Unit::Decibels,
    ),
    ParamDescriptor::choice(ID_TYPE, "type", "Curve", "Drive", TYPES, 0),
    ParamDescriptor::linear(ID_TONE, "tone", "Tone", "Tone", -1.0, 1.0, 0.0, Unit::None),
    // A quality setting, not a performance control: switching clears the
    // oversampling filters, so it is not offered for automation.
    ParamDescriptor {
        automatable: false,
        ..ParamDescriptor::choice(
            ID_OVERSAMPLING,
            "oversampling",
            "Oversampling",
            "Quality",
            OVERSAMPLING,
            1,
        )
    },
    ParamDescriptor::linear(ID_MIX, "mix", "Mix", "Output", 0.0, 1.0, 1.0, Unit::Percent),
    ParamDescriptor::linear(
        ID_OUTPUT,
        "output",
        "Output",
        "Output",
        -24.0,
        12.0,
        0.0,
        Unit::Decibels,
    ),
];

pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    type_id: "peel.fx.drive",
    name: "Drive",
    description: "Tape, tube, hard clip and fold saturation with oversampling",
    category: "Distortion",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
};

pub static PRESETS: [Preset; 6] = [
    Preset {
        name: "Warm Tape",
        params: &[],
    },
    Preset {
        name: "Tube Glow",
        params: &[
            ("type", 1.0),
            ("drive", 9.0),
            ("tone", -0.15),
            ("output", -2.0),
        ],
    },
    Preset {
        name: "Crunch",
        params: &[
            ("type", 2.0),
            ("drive", 16.0),
            ("tone", 0.2),
            ("output", -6.0),
            ("mix", 0.7),
        ],
    },
    Preset {
        name: "Fold Over",
        params: &[
            ("type", 3.0),
            ("drive", 14.0),
            ("tone", 0.1),
            ("output", -5.0),
            ("mix", 0.6),
        ],
    },
    Preset {
        name: "Gentle Edge",
        params: &[
            ("type", 0.0),
            ("drive", 5.0),
            ("tone", 0.25),
            ("output", -1.0),
            ("mix", 0.5),
        ],
    },
    Preset {
        name: "Destroy",
        params: &[
            ("type", 2.0),
            ("drive", 34.0),
            ("tone", -0.3),
            ("oversampling", 2.0),
            ("output", -12.0),
        ],
    },
];

/// Bias of the tube curve; sets how asymmetric it is.
const TUBE_BIAS: f64 = 0.35;

/// Offset tanh with DC removed and slope 1 at the origin.
fn tube(x: f64) -> f64 {
    let b = TUBE_BIAS;
    let norm = 1.0 / (1.0 - b.tanh().powi(2));
    ((x + b).tanh() - b.tanh()) * norm
}

/// Triangle wave through the origin: slope 1, peaks at +/-1, period 4.
fn fold(x: f64) -> f64 {
    let t = x * 0.25 + 0.25;
    let t = t - t.floor();
    1.0 - 4.0 * (t - 0.5).abs()
}

/// `ln(cosh(x))` without overflow.
fn ln_cosh(x: f64) -> f64 {
    let a = x.abs();
    a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
}

/// Antiderivatives of the four curves, for first-order antiderivative
/// anti-aliasing (ADAA).
fn tape_f1(x: f64) -> f64 {
    ln_cosh(x)
}

fn tube_f1(x: f64) -> f64 {
    let b = TUBE_BIAS;
    let norm = 1.0 / (1.0 - b.tanh().powi(2));
    norm * (ln_cosh(x + b) - b.tanh() * x)
}

fn hard_f1(x: f64) -> f64 {
    if x.abs() <= 1.0 {
        0.5 * x * x
    } else {
        x.abs() - 0.5
    }
}

fn fold_f1(x: f64) -> f64 {
    let r = (x + 1.0).rem_euclid(4.0) - 1.0;
    if r <= 1.0 {
        0.5 * (r * r - 1.0)
    } else {
        -0.5 * r * r + 2.0 * r - 1.5
    }
}

/// A saturation curve with its antiderivative.
#[derive(Clone, Copy)]
struct Curve {
    /// Direct evaluation (used without ADAA and as the small-step fallback).
    f: fn(f64) -> f64,
    f1: fn(f64) -> f64,
}

const CURVES: [Curve; 4] = [
    Curve {
        f: |x| x.tanh(),
        f1: tape_f1,
    },
    Curve {
        f: tube,
        f1: tube_f1,
    },
    Curve {
        f: |x| x.clamp(-1.0, 1.0),
        f1: hard_f1,
    },
    Curve {
        f: fold,
        f1: fold_f1,
    },
];

/// Applies a curve to a continuous stream of samples, either directly or
/// with first-order ADAA. ADAA replaces `f(x[n])` with the average of `f`
/// over the segment from `x[n-1]` to `x[n]`, which strongly reduces the
/// aliasing of the corners and folds in `f` (at the cost of half a sample
/// of delay and a mild high-frequency roll-off at the stream rate).
#[derive(Clone, Copy)]
struct Shaper {
    curve: Curve,
    x1: f64,
    f1_prev: f64,
}

impl Shaper {
    fn new(kind: u32) -> Shaper {
        Shaper {
            curve: CURVES[(kind as usize).min(3)],
            x1: 0.0,
            f1_prev: 0.0,
        }
    }

    fn set_kind(&mut self, kind: u32) {
        self.curve = CURVES[(kind as usize).min(3)];
        // Re-seed the history so the first ADAA step is well defined.
        self.f1_prev = (self.curve.f1)(self.x1);
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.f1_prev = (self.curve.f1)(0.0);
    }

    #[inline]
    fn direct(&self, x: f32) -> f32 {
        (self.curve.f)(f64::from(x)) as f32
    }

    #[inline]
    fn adaa(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let f1 = (self.curve.f1)(x);
        let dx = x - self.x1;
        let y = if dx.abs() < 1e-6 {
            (self.curve.f)(0.5 * (x + self.x1))
        } else {
            (f1 - self.f1_prev) / dx
        };
        self.x1 = x;
        self.f1_prev = f1;
        y as f32
    }
}

/// Per-channel oversampling chain.
struct Channel {
    up_outer: Interpolator2,
    down_outer: Decimator2,
    up_inner: Interpolator2,
    down_inner: Decimator2,
    pad: DelayLine,
    dry: DelayLine,
    dc: DcBlocker,
    shaper: Shaper,
}

impl Channel {
    fn new(sample_rate: f32) -> Channel {
        Channel {
            up_outer: Interpolator2::new(TAPS_OUTER),
            down_outer: Decimator2::new(TAPS_OUTER),
            up_inner: Interpolator2::new(TAPS_INNER),
            down_inner: Decimator2::new(TAPS_INNER),
            pad: DelayLine::new(LATENCY as usize + 4),
            dry: DelayLine::new(LATENCY as usize + 4),
            dc: DcBlocker::new(sample_rate, 8.0),
            shaper: Shaper::new(TYPE_TAPE),
        }
    }

    /// Clears the wet path only (used when the oversampling mode changes,
    /// so the dry signal in the mix keeps playing).
    fn clear_wet(&mut self) {
        self.up_outer.clear();
        self.down_outer.clear();
        self.up_inner.clear();
        self.down_inner.clear();
        self.pad.clear();
        self.shaper.reset();
    }

    fn clear(&mut self) {
        self.clear_wet();
        self.dry.clear();
        self.dc.reset();
    }

    /// Shapes one sample (already scaled by the drive gain) and returns the
    /// wet signal aligned to [`LATENCY`] samples.
    #[inline]
    fn wet(&mut self, x: f32, os: u32) -> f32 {
        match os {
            OS_4X => {
                let (u0, u1) = self.up_outer.process(x);
                let (v0, v1) = self.up_inner.process(u0);
                let (w0, w1) = self.up_inner.process(u1);
                let (s0, s1) = (self.shaper.adaa(v0), self.shaper.adaa(v1));
                let (s2, s3) = (self.shaper.adaa(w0), self.shaper.adaa(w1));
                let z0 = self.down_inner.process(s0, s1);
                let z1 = self.down_inner.process(s2, s3);
                self.down_outer.process(z0, z1)
            }
            OS_2X => {
                let (u0, u1) = self.up_outer.process(x);
                let (s0, s1) = (self.shaper.adaa(u0), self.shaper.adaa(u1));
                let y = self.down_outer.process(s0, s1);
                self.pad.push(y);
                self.pad.read(PAD_2X + 1)
            }
            _ => {
                // No ADAA here: "Off" is the raw, unfiltered curve.
                let y = self.shaper.direct(x);
                self.pad.push(y);
                self.pad.read(LATENCY as usize + 1)
            }
        }
    }
}

/// Drive effect. See the module docs.
pub struct Drive {
    store: ParamStore,
    sample_rate: f32,
    ch: [Channel; 2],
    oversampling: u32,
    drive: Smoothed,
    output: Smoothed,
    dry: Smoothed,
    wet: Smoothed,
    tilt_lo: StereoBiquad,
    tilt_hi: StereoBiquad,
    silence: SilenceTracker,
}

impl Default for Drive {
    fn default() -> Self {
        Drive::new()
    }
}

impl Drive {
    pub fn new() -> Drive {
        let mut d = Drive {
            store: ParamStore::new(&DESCRIPTOR),
            sample_rate: 48_000.0,
            ch: [Channel::new(48_000.0), Channel::new(48_000.0)],
            oversampling: OS_2X,
            drive: Smoothed::new(1.0),
            output: Smoothed::new(1.0),
            dry: Smoothed::new(0.0),
            wet: Smoothed::new(1.0),
            tilt_lo: StereoBiquad::default(),
            tilt_hi: StereoBiquad::default(),
            silence: SilenceTracker::default(),
        };
        Fx::setup(&mut d, 48_000.0, 512);
        d
    }

    fn update_tone(&mut self, tone: f32) {
        let sr = self.sample_rate;
        if tone.abs() < 1e-3 {
            self.tilt_lo.set_target(Coeffs::IDENTITY);
            self.tilt_hi.set_target(Coeffs::IDENTITY);
        } else {
            let db = tone * 8.0;
            let q = std::f32::consts::FRAC_1_SQRT_2;
            self.tilt_lo
                .set_target(Coeffs::design(FilterKind::LowShelf, sr, 500.0, q, -db));
            self.tilt_hi
                .set_target(Coeffs::design(FilterKind::HighShelf, sr, 2_000.0, q, db));
        }
    }
}

impl Fx for Drive {
    fn store(&self) -> &ParamStore {
        &self.store
    }

    fn store_mut(&mut self) -> &mut ParamStore {
        &mut self.store
    }

    fn setup(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
        let sr = self.sample_rate;
        self.ch = [Channel::new(sr), Channel::new(sr)];
        for s in [
            &mut self.drive,
            &mut self.output,
            &mut self.dry,
            &mut self.wet,
        ] {
            s.set_ramp(sr, RAMP_SECONDS);
        }
        self.tilt_lo.set_ramp_time(sr, 0.01);
        self.tilt_hi.set_ramp_time(sr, 0.01);
        refresh_all(self);
        for s in [
            &mut self.drive,
            &mut self.output,
            &mut self.dry,
            &mut self.wet,
        ] {
            let t = s.target();
            s.set_immediate(t);
        }
        for f in [&mut self.tilt_lo, &mut self.tilt_hi] {
            let t = f.target();
            f.set_immediate(t);
        }
        self.clear();
    }

    fn param_changed(&mut self, id: ParamId, value: f32) {
        match id {
            ID_DRIVE => self.drive.set_target(db_to_gain(value)),
            ID_TYPE => {
                for c in &mut self.ch {
                    c.shaper.set_kind(value as u32);
                }
            }
            ID_OVERSAMPLING => {
                let os = value as u32;
                if os != self.oversampling {
                    self.oversampling = os;
                    // Filters and pad hold stale data from another mode.
                    for c in &mut self.ch {
                        c.clear_wet();
                    }
                }
            }
            ID_TONE => self.update_tone(value),
            ID_MIX => {
                self.dry.set_target(1.0 - value);
                self.wet.set_target(value);
            }
            ID_OUTPUT => self.output.set_target(db_to_gain(value)),
            _ => {}
        }
    }

    fn clear(&mut self) {
        for c in &mut self.ch {
            c.clear();
        }
        self.tilt_lo.reset();
        self.tilt_hi.reset();
        self.silence.reset();
    }

    fn render(&mut self, _ctx: &ProcessContext, left: &mut [f32], right: &mut [f32]) {
        self.silence.feed(left, right);
        let os = self.oversampling;
        let lat = LATENCY as usize;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = self.drive.next();
            let (in_l, in_r) = (*l, *r);
            let mut wl = self.ch[0].wet(in_l * g, os);
            let mut wr = self.ch[1].wet(in_r * g, os);
            wl = self.ch[0].dc.process(wl);
            wr = self.ch[1].dc.process(wr);
            let (wl, wr) = self.tilt_lo.process(wl, wr);
            let (wl, wr) = self.tilt_hi.process(wl, wr);

            self.ch[0].dry.push(in_l);
            self.ch[1].dry.push(in_r);
            let dl = self.ch[0].dry.read(lat + 1);
            let dr = self.ch[1].dry.read(lat + 1);

            let (dry, wet, out) = (self.dry.next(), self.wet.next(), self.output.next());
            *l = (dl * dry + wl * wet) * out;
            *r = (dr * dry + wr * wet) * out;
        }
    }

    fn latency_samples(&self) -> u32 {
        LATENCY
    }

    fn tail(&self) -> Option<f32> {
        Some(LATENCY as f32 / self.sample_rate)
    }

    fn silent(&self) -> bool {
        self.silence.quiet_for(LATENCY + 4096)
    }
}

impl_device!(Drive, DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use peel_dsp::device::Device;

    const SR: f32 = 48_000.0;

    fn process_all(d: &mut Drive, l: &mut [f32], r: &mut [f32]) {
        let ctx = ProcessContext::new(SR);
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            d.process(&ctx, &[], cl, cr);
        }
    }

    #[test]
    fn curves_have_unity_small_signal_gain_and_bounded_output() {
        for (k, c) in CURVES.iter().enumerate() {
            let f = c.f;
            assert!(f(0.0).abs() < 1e-9);
            assert!((f(1e-4) / 1e-4 - 1.0).abs() < 0.01, "curve {k}");
            for i in -2000..2000 {
                let y = f(f64::from(i) * 0.01);
                // Tube peaks at about -1.51 on the negative side, 0.75 positive.
                assert!(y.abs() <= 1.6, "curve {k}: {y}");
            }
        }
        let tape = CURVES[0].f;
        let tube = CURVES[1].f;
        let fold = CURVES[3].f;
        // Tube is asymmetric, tape is odd-symmetric.
        assert!((tape(0.7) + tape(-0.7)).abs() < 1e-9);
        assert!((tube(0.7) + tube(-0.7)).abs() > 0.05);
        // Fold reflects instead of clipping.
        assert!((fold(1.0) - 1.0).abs() < 1e-9);
        assert!((fold(1.5) - 0.5).abs() < 1e-9);
        assert!((fold(3.0) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn antiderivatives_match_their_curves() {
        for (k, c) in CURVES.iter().enumerate() {
            let h = 1e-5;
            // Avoid the kinks of the hard clip and the fold, where a central
            // difference straddles two linear pieces.
            for x in [-7.3, -2.6, -1.4, -0.45, 0.0, 0.33, 0.9, 1.3, 2.4, 5.1, 40.0] {
                let numeric = ((c.f1)(x + h) - (c.f1)(x - h)) / (2.0 * h);
                assert!(
                    (numeric - (c.f)(x)).abs() < 1e-4,
                    "curve {k} at {x}: {numeric}"
                );
            }
        }
    }

    #[test]
    fn adaa_tracks_the_direct_curve_for_slow_signals() {
        for kind in 0..4u32 {
            let mut s = Shaper::new(kind);
            for i in 0..200 {
                let x = 2.0 * (i as f32 * 0.003).sin();
                let a = s.adaa(x);
                let d = s.direct(x);
                if i > 5 {
                    assert!((a - d).abs() < 0.01, "kind {kind} sample {i}: {a} vs {d}");
                }
            }
        }
    }

    #[test]
    fn latency_is_constant_in_every_mode() {
        for os in [OS_OFF, OS_2X, OS_4X] {
            let mut d = Drive::new();
            d.set_param(ID_OVERSAMPLING, os as f32);
            d.set_param(ID_DRIVE, 0.0);
            d.set_param(ID_MIX, 1.0);
            d.prepare(SR, 256);
            assert_eq!(d.latency(), LATENCY);
            let mut l = vec![0.0; 400];
            l[0] = 0.01; // tiny: tanh is linear here
            let mut r = l.clone();
            process_all(&mut d, &mut l, &mut r);
            let peak = l
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap()
                .0;
            assert_eq!(peak, LATENCY as usize, "oversampling {os}");
            // ADAA averages adjacent oversampled points, which spreads the
            // impulse slightly; the peak stays on the same sample.
            assert!(l[peak] > 0.007 && l[peak] < 0.0105, "{}", l[peak]);
        }
    }

    #[test]
    fn dry_path_is_aligned_with_wet_path() {
        // Drive 0 dB on a small signal is nearly linear, so any mix gives the
        // same output only if dry and wet are time-aligned.
        let run = |mix: f32| {
            let mut d = Drive::new();
            d.set_param(ID_DRIVE, 0.0);
            d.set_param(ID_MIX, mix);
            d.set_param(ID_OVERSAMPLING, 2.0);
            d.prepare(SR, 256);
            let mut l: Vec<f32> = (0..4000)
                .map(|i| 0.005 * (std::f32::consts::TAU * 1_000.0 * i as f32 / SR).sin())
                .collect();
            let mut r = l.clone();
            process_all(&mut d, &mut l, &mut r);
            l
        };
        let a = run(0.0);
        let b = run(1.0);
        for i in 1000..4000 {
            assert!((a[i] - b[i]).abs() < 2e-4, "{i}: {} vs {}", a[i], b[i]);
        }
    }

    /// Accurate sine sample (phase reduced in f64).
    fn sin_at(freq: f64, i: usize) -> f32 {
        (std::f64::consts::TAU * (i as f64 * freq / f64::from(SR)).fract()).sin() as f32
    }

    /// Power at an exact frequency using a DFT over `len` samples.
    fn power_at(x: &[f32], freq: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let ph = std::f64::consts::TAU * f64::from(freq) * i as f64 / f64::from(SR);
            re += f64::from(*v) * ph.cos();
            im -= f64::from(*v) * ph.sin();
        }
        ((re * re + im * im) / (x.len() as f64).powi(2)) as f32
    }

    fn aliasing_energy(os: u32, curve: u32) -> (f32, f32) {
        let mut d = Drive::new();
        d.set_param(ID_TYPE, curve as f32);
        d.set_param(ID_DRIVE, 24.0);
        d.set_param(ID_OVERSAMPLING, os as f32);
        d.set_param(ID_MIX, 1.0);
        d.prepare(SR, 256);
        let n = 9600; // 0.2 s: every multiple of 10 Hz is an exact bin
        let f0 = 10_000.0;
        let mut l: Vec<f32> = (0..n).map(|i| 0.5 * sin_at(f64::from(f0), i)).collect();
        let mut r = l.clone();
        process_all(&mut d, &mut l, &mut r);
        let seg = &l[4800..];
        // 10 kHz harmonics fold to these frequencies at 48 kHz: 30k->18k,
        // 50k->2k, 70k->22k, 90k->6k, 110k->14k.
        let alias: f32 = [2_000.0, 6_000.0, 14_000.0, 18_000.0, 22_000.0]
            .iter()
            .map(|f| power_at(seg, *f))
            .sum();
        (power_at(seg, f0), alias)
    }

    #[test]
    fn oversampling_suppresses_aliasing() {
        for curve in [TYPE_HARD, TYPE_FOLD, TYPE_TAPE] {
            let (fund_off, alias_off) = aliasing_energy(OS_OFF, curve);
            let (fund_2x, alias_2x) = aliasing_energy(OS_2X, curve);
            let (fund_4x, alias_4x) = aliasing_energy(OS_4X, curve);
            assert!(fund_off > 0.01 && fund_2x > 0.01 && fund_4x > 0.01);
            let db = |a: f32, b: f32| 10.0 * (a / b.max(1e-20)).log10();
            let off_to_2x = db(alias_off, alias_2x);
            let off_to_4x = db(alias_off, alias_4x);
            assert!(
                off_to_2x > 12.0,
                "curve {curve}: 2x improves only {off_to_2x} dB"
            );
            assert!(
                off_to_4x > 30.0,
                "curve {curve}: 4x improves only {off_to_4x} dB"
            );
            assert!(alias_4x < alias_2x, "curve {curve}");
        }
    }

    #[test]
    fn tone_tilts_the_spectrum() {
        let run = |tone: f32, f: f32| {
            let mut d = Drive::new();
            d.set_param(ID_DRIVE, 0.0);
            d.set_param(ID_TONE, tone);
            d.prepare(SR, 256);
            let mut l: Vec<f32> = (0..9600)
                .map(|i| 0.01 * (std::f32::consts::TAU * f * i as f32 / SR).sin())
                .collect();
            let mut r = l.clone();
            process_all(&mut d, &mut l, &mut r);
            l[4800..].iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        assert!(run(1.0, 8_000.0) > run(0.0, 8_000.0) * 1.8);
        assert!(run(1.0, 100.0) < run(0.0, 100.0) * 0.6);
        assert!(run(-1.0, 8_000.0) < run(0.0, 8_000.0) * 0.6);
    }

    #[test]
    fn tube_output_has_no_dc() {
        let mut d = Drive::new();
        d.set_param(ID_TYPE, 1.0);
        d.set_param(ID_DRIVE, 20.0);
        d.prepare(SR, 256);
        let mut l: Vec<f32> = (0..48_000)
            .map(|i| 0.4 * (std::f32::consts::TAU * 200.0 * i as f32 / SR).sin())
            .collect();
        let mut r = l.clone();
        process_all(&mut d, &mut l, &mut r);
        let mean: f32 = l[24_000..].iter().sum::<f32>() / 24_000.0;
        assert!(mean.abs() < 0.01, "{mean}");
    }

    #[test]
    fn switching_oversampling_live_is_safe() {
        let mut d = Drive::new();
        d.set_param(ID_DRIVE, 30.0);
        d.prepare(SR, 256);
        let ctx = ProcessContext::new(SR);
        for block in 0..60 {
            let mut l: Vec<f32> = (0..256)
                .map(|i| {
                    0.8 * (std::f32::consts::TAU * 440.0 * (block * 256 + i) as f32 / SR).sin()
                })
                .collect();
            let mut r = l.clone();
            if block % 10 == 0 {
                d.set_param(ID_OVERSAMPLING, (block / 10 % 3) as f32);
            }
            d.process(&ctx, &[], &mut l, &mut r);
            assert!(l.iter().all(|v| v.is_finite() && v.abs() < 4.0));
        }
    }
}
