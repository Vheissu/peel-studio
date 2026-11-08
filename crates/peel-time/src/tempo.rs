use serde::{Deserialize, Serialize};

use crate::Time;

/// A bar interpretation. Independent of tempo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        TimeSignature {
            numerator: 4,
            denominator: 4,
        }
    }
}

impl TimeSignature {
    /// Length of one bar in quarter notes.
    pub fn bar_length(self) -> Time {
        Time::new(
            4 * self.numerator.max(1) as i64,
            self.denominator.max(1) as i64,
        )
    }

    /// Length of one counted beat in quarter notes (a quarter in 4/4, an
    /// eighth in 6/8).
    pub fn beat_length(self) -> Time {
        Time::new(4, self.denominator.max(1) as i64)
    }
}

/// One tempo segment. The tempo is `bpm` at `start` and, when `ramp_to` is
/// set, changes linearly (BPM against quarter-note position) until the start
/// of the following segment.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TempoSegment {
    pub start: Time,
    pub bpm: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramp_to: Option<f64>,
}

/// Compiled tempo map with invertible beat/second conversion.
///
/// All query methods are allocation-free and safe to call on a realtime
/// thread once the map has been built.
#[derive(Clone, Debug, PartialEq)]
pub struct TempoMap {
    segs: Vec<CompiledSegment>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CompiledSegment {
    start_beat: f64,
    start_sec: f64,
    bpm: f64,
    /// BPM change per quarter note (0 for constant tempo).
    slope: f64,
}

const MIN_BPM: f64 = 1.0;
const MAX_BPM: f64 = 999.0;
const SLOPE_EPS: f64 = 1e-9;

impl CompiledSegment {
    fn seconds_at(&self, beat: f64) -> f64 {
        let q = beat - self.start_beat;
        self.start_sec + seconds_for(self.bpm, self.slope, q)
    }

    fn beat_at(&self, sec: f64) -> f64 {
        let s = sec - self.start_sec;
        self.start_beat + beats_for(self.bpm, self.slope, s)
    }

    fn bpm_at(&self, beat: f64) -> f64 {
        (self.bpm + self.slope * (beat - self.start_beat)).clamp(MIN_BPM, MAX_BPM)
    }
}

/// Elapsed seconds over `q` quarter notes starting at tempo `b0` with slope `k`.
fn seconds_for(b0: f64, k: f64, q: f64) -> f64 {
    if k.abs() < SLOPE_EPS {
        60.0 * q / b0
    } else {
        // 60/k * ln((b0 + k q) / b0), computed with ln_1p for stability.
        60.0 / k * (k * q / b0).ln_1p()
    }
}

/// Inverse of [`seconds_for`].
fn beats_for(b0: f64, k: f64, s: f64) -> f64 {
    if k.abs() < SLOPE_EPS {
        s * b0 / 60.0
    } else {
        b0 / k * (k * s / 60.0).exp_m1()
    }
}

impl TempoMap {
    /// A constant tempo.
    pub fn constant(bpm: f64) -> TempoMap {
        TempoMap::from_segments(&[TempoSegment {
            start: Time::ZERO,
            bpm,
            ramp_to: None,
        }])
    }

    /// Builds a map from document segments. Segments are sorted by start; the
    /// first segment is treated as starting at beat 0.
    pub fn from_segments(segments: &[TempoSegment]) -> TempoMap {
        let mut sorted: Vec<TempoSegment> = segments.to_vec();
        sorted.sort_by_key(|a| a.start);
        if sorted.is_empty() {
            sorted.push(TempoSegment {
                start: Time::ZERO,
                bpm: 120.0,
                ramp_to: None,
            });
        }
        let mut segs = Vec::with_capacity(sorted.len());
        let mut sec = 0.0;
        for (i, seg) in sorted.iter().enumerate() {
            let start_beat = if i == 0 { 0.0 } else { seg.start.as_f64() };
            let bpm = seg.bpm.clamp(MIN_BPM, MAX_BPM);
            let slope = match (seg.ramp_to, sorted.get(i + 1)) {
                (Some(target), Some(next)) => {
                    let len = next.start.as_f64() - start_beat;
                    if len > 0.0 {
                        (target.clamp(MIN_BPM, MAX_BPM) - bpm) / len
                    } else {
                        0.0
                    }
                }
                _ => 0.0,
            };
            if let Some(prev) = segs.last() {
                let prev: &CompiledSegment = prev;
                sec = prev.seconds_at(start_beat);
            }
            segs.push(CompiledSegment {
                start_beat,
                start_sec: sec,
                bpm,
                slope,
            });
        }
        TempoMap { segs }
    }

    fn segment_for_beat(&self, beat: f64) -> &CompiledSegment {
        let idx = self.segs.partition_point(|s| s.start_beat <= beat);
        &self.segs[idx.saturating_sub(1)]
    }

    fn segment_for_sec(&self, sec: f64) -> &CompiledSegment {
        let idx = self.segs.partition_point(|s| s.start_sec <= sec);
        &self.segs[idx.saturating_sub(1)]
    }

    /// Tempo at the given beat.
    pub fn bpm_at(&self, beat: f64) -> f64 {
        self.segment_for_beat(beat).bpm_at(beat)
    }

    pub fn beat_to_seconds(&self, beat: f64) -> f64 {
        self.segment_for_beat(beat).seconds_at(beat)
    }

    pub fn seconds_to_beat(&self, sec: f64) -> f64 {
        self.segment_for_sec(sec).beat_at(sec)
    }

    /// Absolute sample position of a beat, rounded to nearest (ties to even).
    pub fn beat_to_sample(&self, beat: f64, sample_rate: f64) -> i64 {
        (self.beat_to_seconds(beat) * sample_rate).round_ties_even() as i64
    }

    pub fn sample_to_beat(&self, sample: i64, sample_rate: f64) -> f64 {
        self.seconds_to_beat(sample as f64 / sample_rate)
    }

    pub fn is_constant(&self) -> bool {
        self.segs.len() == 1
    }
}

impl Default for TempoMap {
    fn default() -> Self {
        TempoMap::constant(120.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_tempo_round_trip() {
        let map = TempoMap::constant(120.0);
        assert_eq!(map.beat_to_seconds(4.0), 2.0);
        assert!((map.seconds_to_beat(2.0) - 4.0).abs() < 1e-12);
        assert_eq!(map.beat_to_sample(1.0, 48_000.0), 24_000);
    }

    #[test]
    fn ramp_matches_closed_form_and_inverts() {
        let map = TempoMap::from_segments(&[
            TempoSegment {
                start: Time::ZERO,
                bpm: 100.0,
                ramp_to: Some(140.0),
            },
            TempoSegment {
                start: Time::beats(16),
                bpm: 140.0,
                ramp_to: None,
            },
        ]);
        let k: f64 = 40.0 / 16.0;
        let expected = 60.0 / k * ((100.0 + k * 8.0) / 100.0f64).ln();
        assert!((map.beat_to_seconds(8.0) - expected).abs() < 1e-9);
        for b in [0.0, 0.5, 3.25, 15.999, 16.0, 20.0, 64.0] {
            let s = map.beat_to_seconds(b);
            assert!((map.seconds_to_beat(s) - b).abs() < 1e-9, "beat {b}");
        }
        // Continuity at the segment boundary.
        let a = map.beat_to_seconds(16.0 - 1e-9);
        let c = map.beat_to_seconds(16.0);
        assert!((a - c).abs() < 1e-6);
        assert!((map.bpm_at(16.0) - 140.0).abs() < 1e-9);
    }

    #[test]
    fn bar_lengths() {
        assert_eq!(
            TimeSignature {
                numerator: 4,
                denominator: 4
            }
            .bar_length(),
            Time::beats(4)
        );
        assert_eq!(
            TimeSignature {
                numerator: 6,
                denominator: 8
            }
            .bar_length(),
            Time::new(3, 1)
        );
        assert_eq!(
            TimeSignature {
                numerator: 7,
                denominator: 8
            }
            .bar_length(),
            Time::new(7, 2)
        );
    }
}
