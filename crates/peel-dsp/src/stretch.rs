//! Time stretching without changing pitch (WSOLA).
//!
//! Waveform-similarity overlap-add: the output is built from overlapping
//! Hann-windowed grains read from the input at the stretched position, each
//! nudged within a small search window to the offset whose waveform best
//! continues the previous grain. It keeps transients and pitch, which suits
//! drum loops and most rhythmic material. Runs offline, never on the audio
//! thread.

/// Grain length and hop at 48 kHz, scaled for other rates.
const GRAIN_SECONDS: f64 = 0.040;
const SEARCH_SECONDS: f64 = 0.008;

/// Stretches every channel by `ratio` (output length = input length ×
/// `ratio`; 2.0 plays twice as long at the same pitch). A ratio of 1
/// returns a copy.
///
/// The input is cut at its onsets (drum hits, note attacks) and each piece
/// is stretched on its own, so every attack starts its piece at full level
/// and lands exactly at its stretched time.
pub fn stretch(channels: &[Vec<f32>], ratio: f64, sample_rate: f32) -> Vec<Vec<f32>> {
    let n_in = channels.iter().map(Vec::len).min().unwrap_or(0);
    if n_in == 0 || !ratio.is_finite() || ratio <= 0.0 {
        return channels.iter().map(|_| Vec::new()).collect();
    }
    if (ratio - 1.0).abs() < 1e-6 {
        return channels.iter().map(|c| c[..n_in].to_vec()).collect();
    }
    let n_out = (n_in as f64 * ratio).round() as usize;
    let mut bounds = onsets(channels, n_in, sample_rate);
    bounds.insert(0, 0);
    bounds.push(n_in);
    bounds.dedup();
    let mut out: Vec<Vec<f32>> = channels.iter().map(|_| Vec::with_capacity(n_out)).collect();
    let fade = ((sample_rate * 0.002) as usize).max(1);
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        // Output positions from the cumulative input position, so rounding
        // never drifts.
        let (oa, ob) = (
            (a as f64 * ratio).round() as usize,
            (b as f64 * ratio).round() as usize,
        );
        let slices: Vec<&[f32]> = channels.iter().map(|c| &c[a..b]).collect();
        let mut piece = wsola(&slices, ratio, sample_rate, ob - oa);
        // A short fade where a shortened piece is cut before the next onset.
        for p in &mut piece {
            let n = p.len();
            for i in 0..fade.min(n) {
                p[n - 1 - i] *= i as f32 / fade as f32;
            }
        }
        for (o, p) in out.iter_mut().zip(piece) {
            o.extend(p);
        }
    }
    out
}

/// Onset positions: where short-term energy jumps well above what came just
/// before. At least 60 ms apart.
fn onsets(channels: &[Vec<f32>], n: usize, sample_rate: f32) -> Vec<usize> {
    let frame = ((sample_rate * 0.004) as usize).max(16);
    let energy: Vec<f32> = (0..n / frame)
        .map(|f| {
            let mut e = 0.0;
            for c in channels {
                for v in &c[f * frame..(f + 1) * frame] {
                    e += v * v;
                }
            }
            e / (frame * channels.len()) as f32
        })
        .collect();
    let history = 12;
    let min_gap = ((sample_rate * 0.060) as usize / frame).max(1);
    let mut out = Vec::new();
    let mut last: Option<usize> = None;
    for f in 1..energy.len() {
        let lo = f.saturating_sub(history);
        let before = energy[lo..f].iter().sum::<f32>() / (f - lo) as f32;
        let loud = energy[f] > 1e-5;
        if loud && energy[f] > before * 6.0 + 1e-7 && last.is_none_or(|l| f - l >= min_gap) {
            // Start the piece a little before the frame that jumped.
            out.push((f * frame).saturating_sub(frame / 2));
            last = Some(f);
        }
    }
    out
}

/// WSOLA over one segment, producing exactly `n_out` frames. Channels share
/// one set of grain positions so stereo stays aligned.
fn wsola(channels: &[&[f32]], ratio: f64, sample_rate: f32, n_out: usize) -> Vec<Vec<f32>> {
    let n_in = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    if n_in == 0 || n_out == 0 {
        return channels.iter().map(|_| vec![0.0; n_out]).collect();
    }
    let rate = f64::from(sample_rate.max(1.0));
    let grain = ((GRAIN_SECONDS * rate) as usize).max(64) & !1;
    let hop = grain / 2;
    let search = ((SEARCH_SECONDS * rate) as usize).max(16);
    let window: Vec<f32> = (0..grain)
        .map(|i| {
            let x = std::f32::consts::PI * i as f32 / grain as f32;
            x.sin() * x.sin()
        })
        .collect();
    // Mono guide for the similarity search.
    let guide: Vec<f32> = (0..n_in)
        .map(|i| channels.iter().map(|c| c[i]).sum::<f32>() / channels.len() as f32)
        .collect();
    let mut out: Vec<Vec<f32>> = channels.iter().map(|_| vec![0.0; n_out + grain]).collect();
    let mut norm = vec![0.0f32; n_out + grain];
    // Where the previous grain would naturally continue in the input.
    let mut prev_tail: Option<usize> = None;
    let mut out_pos = 0usize;
    while out_pos < n_out {
        let nominal = (out_pos as f64 / ratio) as isize;
        let mut best = nominal;
        if let Some(tail) = prev_tail {
            // Compare the candidate's first half with what the previous grain
            // continued into; pick the best match within the search range.
            let similarity = |c: isize| {
                let mut score = 0.0f32;
                let mut k = 0;
                while k < hop {
                    let a = guide.get(tail + k).copied().unwrap_or(0.0);
                    let b = guide.get(c as usize + k).copied().unwrap_or(0.0);
                    score += a * b;
                    k += 4;
                }
                score
            };
            // Start from the nominal position and move only for a clearly
            // better match, so silence and ties keep their timing.
            let mut best_score = similarity(nominal.max(0));
            let lo = (nominal - search as isize).max(0);
            let hi = (nominal + search as isize).min(n_in as isize - 1);
            let mut c = lo;
            while c <= hi {
                let score = similarity(c);
                let margin = best_score.abs() * 1e-3 + 1e-9;
                if score > best_score + margin
                    || (score >= best_score - margin
                        && (c - nominal).abs() < (best - nominal).abs())
                {
                    best_score = score;
                    best = c;
                }
                c += 2;
            }
        }
        let start = best.max(0) as usize;
        for (ch, o) in channels.iter().zip(out.iter_mut()) {
            for i in 0..grain {
                let s = ch.get(start + i).copied().unwrap_or(0.0);
                o[out_pos + i] += s * window[i];
            }
        }
        for i in 0..grain {
            norm[out_pos + i] += window[i];
        }
        prev_tail = Some(start + hop);
        out_pos += hop;
    }
    for o in &mut out {
        for (v, w) in o.iter_mut().zip(&norm) {
            if *w > 1e-3 {
                *v /= w;
            }
        }
        o.truncate(n_out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, seconds: f32) -> Vec<f32> {
        (0..(SR * seconds) as usize)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / SR).sin() * 0.5)
            .collect()
    }

    /// Frequency from upward zero crossings over the middle of a signal.
    fn frequency(x: &[f32]) -> f32 {
        let mid = &x[x.len() / 4..x.len() * 3 / 4];
        let crossings = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        crossings as f32 * SR / mid.len() as f32
    }

    #[test]
    fn stretching_changes_length_not_pitch() {
        let x = sine(440.0, 1.0);
        for ratio in [0.75, 1.25, 1.5, 2.0] {
            let y = &stretch(std::slice::from_ref(&x), ratio, SR)[0];
            assert_eq!(y.len(), (x.len() as f64 * ratio).round() as usize);
            let f = frequency(y);
            assert!((f - 440.0).abs() < 4.0, "ratio {ratio}: {f} Hz");
            let rms = (y.iter().map(|v| v * v).sum::<f32>() / y.len() as f32).sqrt();
            assert!((rms - 0.3535).abs() < 0.03, "ratio {ratio}: level {rms}");
        }
    }

    #[test]
    fn transients_land_at_their_stretched_time() {
        // Clicks every quarter second land at their stretched times.
        let mut x = vec![0.0f32; SR as usize * 2];
        for k in 0..8 {
            let at = k * SR as usize / 4;
            for i in 0..200 {
                x[at + i] = (1.0 - i as f32 / 200.0) * if i % 2 == 0 { 1.0 } else { -1.0 };
            }
        }
        for ratio in [0.7, 1.5] {
            let y = &stretch(std::slice::from_ref(&x), ratio, SR)[0];
            for k in 1..7 {
                // Within 3 ms of the stretched time, at full level.
                let expect = (k as f64 * 0.25 * ratio * SR as f64) as usize;
                let win = &y[expect - 150..expect + 150];
                let peak = win.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                assert!(
                    peak > 0.9,
                    "ratio {ratio}: click {k} near {expect} peaks at {peak}"
                );
            }
        }
    }

    #[test]
    fn ratio_one_is_a_copy_and_bad_ratios_are_empty() {
        let x = sine(220.0, 0.1);
        assert_eq!(stretch(std::slice::from_ref(&x), 1.0, SR)[0], x);
        assert!(stretch(std::slice::from_ref(&x), 0.0, SR)[0].is_empty());
        assert!(stretch(&[x], f64::NAN, SR)[0].is_empty());
    }
}
