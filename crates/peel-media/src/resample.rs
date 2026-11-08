//! Sample-rate conversion with a Kaiser-windowed sinc.
//!
//! Every output sample is the inner product of the input with a sinc low-pass
//! centered on the exact (fractional) input position, so any ratio works and
//! no rounding accumulates. The kernel is windowed with a Kaiser window
//! (beta 10, about 100 dB of stopband rejection) over 48 zero crossings on
//! each side, and stretched when downsampling so frequencies above the new
//! Nyquist are removed instead of aliased. The pass band reaches about 94 %
//! of the lower Nyquist frequency.

use peel_dsp::SampleBuffer;

/// Zero crossings of the sinc on each side of the center, at unit scale.
const ZERO_CROSSINGS: usize = 48;
const KAISER_BETA: f64 = 10.0;
/// Fraction of the lower Nyquist frequency where the filter starts to roll off.
const CUTOFF: f64 = 0.94;
/// Kernel table resolution: samples per unit of the sinc argument.
const TABLE_RES: usize = 2048;

/// Zeroth-order modified Bessel function of the first kind.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..64 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// The windowed sinc `sinc(u) * kaiser(u / ZERO_CROSSINGS)`, tabulated for
/// `u` in `[0, ZERO_CROSSINGS]` and evaluated by linear interpolation.
struct Kernel {
    table: Vec<f32>,
}

impl Kernel {
    fn new() -> Kernel {
        let denom = bessel_i0(KAISER_BETA);
        let n = ZERO_CROSSINGS * TABLE_RES;
        let table = (0..=n + 1)
            .map(|i| {
                let u = i as f64 / TABLE_RES as f64;
                let sinc = if u == 0.0 {
                    1.0
                } else {
                    (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u)
                };
                let r = u / ZERO_CROSSINGS as f64;
                let window = if r >= 1.0 {
                    0.0
                } else {
                    bessel_i0(KAISER_BETA * (1.0 - r * r).sqrt()) / denom
                };
                (sinc * window) as f32
            })
            .collect();
        Kernel { table }
    }

    /// Kernel value at distance `u` (in units of the stretched sinc).
    #[inline]
    fn at(&self, u: f64) -> f32 {
        let pos = u.abs() * TABLE_RES as f64;
        let i = pos as usize;
        if i >= ZERO_CROSSINGS * TABLE_RES {
            return 0.0;
        }
        let frac = (pos - i as f64) as f32;
        let a = self.table[i];
        let b = self.table[i + 1];
        a + (b - a) * frac
    }
}

/// Converts `buf` to `target_rate`. Returns a copy when the rates already
/// match. Length is `round(frames * target / source)`; channel layout, name,
/// root key and loop points are preserved (loop points scale with the rate).
///
/// Samples outside the buffer are treated as silence, so the first and last
/// few milliseconds roll in and out from zero.
pub fn resample(buf: &SampleBuffer, target_rate: f32) -> SampleBuffer {
    let src_rate = buf.sample_rate as f64;
    let dst_rate = target_rate as f64;
    let frames = buf.frames();
    if !(src_rate > 0.0 && dst_rate > 0.0) || (src_rate - dst_rate).abs() < 1e-6 || frames == 0 {
        let mut out = buf.clone();
        if dst_rate > 0.0 && frames == 0 {
            out.sample_rate = target_rate;
        }
        return out;
    }

    let ratio = dst_rate / src_rate;
    let out_frames = ((frames as f64) * ratio).round().max(1.0) as usize;
    // Below 1.0 the kernel stretches to low-pass at the new Nyquist.
    let scale = ratio.min(1.0) * CUTOFF;
    let half_width = ZERO_CROSSINGS as f64 / scale;
    let kernel = Kernel::new();

    let mut channels: Vec<Vec<f32>> = buf
        .channels
        .iter()
        .map(|_| Vec::with_capacity(out_frames))
        .collect();
    // Weights for one output sample, reused for every channel.
    let max_taps = (2.0 * half_width).ceil() as usize + 2;
    let mut weights = vec![0.0f32; max_taps];

    for j in 0..out_frames {
        let center = j as f64 / ratio;
        let first = (center - half_width).ceil() as i64;
        let last = (center + half_width).floor() as i64;
        let taps = ((last - first + 1).max(0) as usize).min(max_taps);

        // Normalizing makes the DC gain exactly 1 for every phase.
        let mut sum = 0.0f64;
        for (k, w) in weights[..taps].iter_mut().enumerate() {
            let d = (first + k as i64) as f64 - center;
            let v = kernel.at(d * scale) * scale as f32;
            *w = v;
            sum += v as f64;
        }
        let norm = if sum.abs() > 1e-12 {
            (1.0 / sum) as f32
        } else {
            1.0
        };

        for (dst, src) in channels.iter_mut().zip(&buf.channels) {
            let mut acc = 0.0f32;
            for (k, &w) in weights[..taps].iter().enumerate() {
                let idx = first + k as i64;
                if idx >= 0 && (idx as usize) < src.len() {
                    acc += src[idx as usize] * w;
                }
            }
            dst.push(acc * norm);
        }
    }

    let mut out = SampleBuffer::new(buf.name.clone(), target_rate, channels);
    out.root_key = buf.root_key;
    out.loop_range = buf.loop_range.map(|(s, e)| {
        let scale = |f: usize| ((f as f64 * ratio).round() as usize).min(out_frames);
        (scale(s), scale(e))
    });
    out
}
