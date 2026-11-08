//! The anchor between musical time and engine samples.

use peel_time::TempoMap;

/// Engine-sample position of musical time zero, as `whole + frac` samples.
///
/// A beat `b` falls on engine sample `whole + round(frac + seconds(b) * sr)`.
/// Keeping the fraction separate means a transport that has run for hours, or
/// looped thousands of times, is exactly where the unrounded timeline says it
/// is: rounding happens once per event, never accumulates, and does not
/// depend on how the stream was split into blocks. When `frac` is zero (play
/// from the start) a beat lands exactly on `TempoMap::beat_to_sample`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Origin {
    whole: i64,
    frac: f64,
}

impl Origin {
    pub const ZERO: Origin = Origin {
        whole: 0,
        frac: 0.0,
    };

    /// The origin that puts `beat` exactly on engine sample `at`.
    pub fn anchored(tempo: &TempoMap, beat: f64, at: i64, sr: f64) -> Origin {
        let x = tempo.beat_to_seconds(beat) * sr;
        let rounded = x.round_ties_even();
        Origin {
            whole: at.saturating_sub(rounded as i64),
            frac: rounded - x,
        }
    }

    /// Engine sample on which `beat` falls.
    #[inline]
    pub fn sample_of(&self, tempo: &TempoMap, beat: f64, sr: f64) -> i64 {
        let x = self.frac + tempo.beat_to_seconds(beat) * sr;
        self.whole.saturating_add(x.round_ties_even() as i64)
    }

    /// Musical position of an engine sample.
    pub fn beat_at(&self, tempo: &TempoMap, sample: i64, sr: f64) -> f64 {
        let samples = sample.saturating_sub(self.whole) as f64 - self.frac;
        tempo.seconds_to_beat(samples / sr)
    }

    /// Re-anchors for a loop wrap: afterwards `start` falls where `end` fell.
    /// The shift is the exact (unrounded) loop length.
    pub fn wrap(&mut self, tempo: &TempoMap, start: f64, end: f64, sr: f64) {
        let len = (tempo.beat_to_seconds(end) - tempo.beat_to_seconds(start)) * sr;
        let f = self.frac + len;
        let carry = f.round_ties_even();
        self.whole = self.whole.saturating_add(carry as i64);
        self.frac = f - carry;
    }
}

/// Exact loop length in samples, if the range is usable (at least one sample,
/// finite). Used to reject loops that could never advance.
pub(crate) fn loop_samples(tempo: &TempoMap, start: f64, end: f64, sr: f64) -> f64 {
    (tempo.beat_to_seconds(end) - tempo.beat_to_seconds(start)) * sr
}
