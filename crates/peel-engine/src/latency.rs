//! Where a recorded take belongs on the timeline.
//!
//! A performer plays along with what they *hear*, not with what the engine
//! has just rendered, and the microphone delivers their sound later than it
//! happened. Both delays must be undone for a take to line up with the music
//! it was played to. All of it lives in [`take_start_beat`], so there is one
//! formula to read, test and correct.
//!
//! # The model
//!
//! The input callback that delivers the take's first frame reads the
//! transport position from telemetry, `P`: the position of the *next* sample
//! the output will render (telemetry is updated at the end of every output
//! callback). At that moment:
//!
//! * `P` reaches the speakers `lead + block` after the output callback began
//!   (`lead` is the delay until the first sample of that callback plays,
//!   `block` the length of the callback), and that callback began `age`
//!   before the input callback. So the sound leaving the speakers at the
//!   instant of the input callback is `lead + block - age` *behind* `P`.
//! * The first frame of the input callback was captured by the converter
//!   `input` before the callback (the device buffer plus converter and
//!   driver latency; cpal reports it as `callback - capture`).
//! * The performer reacted to what was playing when their sound was made, so
//!   the first frame belongs at
//!
//! ```text
//! start = P - (lead + block - age + input)      (seconds of music time)
//!         - manual                              (user correction)
//! ```
//!
//! `age` is how long ago the output callback began, known when both streams
//! stamp their callbacks on one clock (Core Audio, WASAPI, ALSA do); when it
//! is not known the best guess is half a block, the average staleness of the
//! telemetry position.
//!
//! The result is in seconds of music time, so it is converted with the tempo
//! map: beat to seconds, subtract, seconds to beat. It can be negative when a
//! take starts at the very top of the song; callers place the clip at zero
//! and trim the head.
//!
//! # Accuracy
//!
//! The position and the input timing come from two different threads, so the
//! estimate is good to about one output block (a few milliseconds). Devices
//! that report no latency get one buffer assumed. The `manual` offset is for
//! the rest: a loopback measurement or a nudge by ear.

use peel_time::TempoMap;

/// What the input callback measured at the first frame of a take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureStart {
    /// Transport position (quarter notes) from telemetry when the first
    /// input callback of the take ran: the next sample the output renders.
    pub playhead_beat: f64,
    /// Seconds from the start of an output callback until its first sample
    /// plays (cpal `playback - callback`; one buffer if the host reports
    /// nothing).
    pub output_lead_s: f64,
    /// Length of that output callback in seconds.
    pub output_block_s: f64,
    /// Seconds between the start of that output callback and the input
    /// callback, if both clocks are comparable.
    pub output_age_s: Option<f64>,
    /// Seconds from capturing the first frame to the input callback (cpal
    /// `callback - capture`; one buffer if the host reports nothing).
    pub input_latency_s: f64,
}

impl CaptureStart {
    /// How far behind [`playhead_beat`](CaptureStart::playhead_beat), in
    /// seconds of music time, the first captured frame belongs (before any
    /// manual correction).
    pub fn lag_seconds(&self) -> f64 {
        let age = self
            .output_age_s
            .unwrap_or(self.output_block_s * 0.5)
            .clamp(0.0, self.output_block_s.max(0.0) * 2.0);
        self.output_lead_s + self.output_block_s - age + self.input_latency_s
    }
}

/// Position, in quarter notes, of the first frame of a take. See the module
/// documentation for the formula.
///
/// `manual_offset_s` is the user's alignment correction: positive moves the
/// take earlier (its sound arrives late). The result may be negative.
pub fn take_start_beat(tempo: &TempoMap, start: &CaptureStart, manual_offset_s: f64) -> f64 {
    let seconds =
        tempo.beat_to_seconds(start.playhead_beat) - start.lag_seconds() - manual_offset_s;
    tempo.seconds_to_beat(seconds)
}

/// Round-trip figures for display, in seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LatencyReport {
    /// Capture to callback on the input device.
    pub input_s: f64,
    /// Output callback to speaker.
    pub output_s: f64,
}

impl LatencyReport {
    pub fn round_trip_s(&self) -> f64 {
        self.input_s + self.output_s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn frames(n: f64) -> f64 {
        n / SR
    }

    fn start(playhead_beat: f64) -> CaptureStart {
        CaptureStart {
            playhead_beat,
            output_lead_s: frames(480.0),
            output_block_s: frames(256.0),
            output_age_s: Some(frames(100.0)),
            input_latency_s: frames(384.0),
        }
    }

    #[test]
    fn the_lag_adds_both_devices_and_removes_the_callback_age() {
        let s = start(8.0);
        // 480 + 256 - 100 + 384 frames.
        assert!((s.lag_seconds() - frames(1020.0)).abs() < 1e-12);
    }

    #[test]
    fn an_unknown_age_assumes_half_a_block() {
        let mut s = start(8.0);
        s.output_age_s = None;
        assert!((s.lag_seconds() - frames(480.0 + 256.0 - 128.0 + 384.0)).abs() < 1e-12);
        // An absurd age (clocks that cannot be compared) cannot make the lag
        // negative or grow it without bound.
        s.output_age_s = Some(30.0);
        assert!((s.lag_seconds() - frames(480.0 + 256.0 - 512.0 + 384.0)).abs() < 1e-12);
        s.output_age_s = Some(-5.0);
        assert!((s.lag_seconds() - frames(480.0 + 256.0 + 384.0)).abs() < 1e-12);
    }

    #[test]
    fn the_start_is_the_playhead_moved_back_by_the_lag_in_music_time() {
        // 120 BPM: a beat is half a second.
        let tempo = TempoMap::constant(120.0);
        let s = start(8.0);
        let lag_beats = s.lag_seconds() * 2.0;
        let beat = take_start_beat(&tempo, &s, 0.0);
        assert!((beat - (8.0 - lag_beats)).abs() < 1e-9, "{beat}");
        // At 60 BPM the same delay is half as many beats.
        let slow = TempoMap::constant(60.0);
        let beat = take_start_beat(&slow, &s, 0.0);
        assert!((beat - (8.0 - s.lag_seconds())).abs() < 1e-9, "{beat}");
    }

    #[test]
    fn the_manual_offset_moves_the_take_earlier_or_later() {
        let tempo = TempoMap::constant(120.0);
        let s = start(8.0);
        let base = take_start_beat(&tempo, &s, 0.0);
        // 10 ms late recordings move 0.02 beats earlier at 120 BPM.
        assert!((take_start_beat(&tempo, &s, 0.010) - (base - 0.02)).abs() < 1e-9);
        assert!((take_start_beat(&tempo, &s, -0.010) - (base + 0.02)).abs() < 1e-9);
    }

    #[test]
    fn a_take_at_the_top_of_the_song_starts_before_zero() {
        let tempo = TempoMap::constant(120.0);
        let s = start(0.0);
        let beat = take_start_beat(&tempo, &s, 0.0);
        assert!((beat + s.lag_seconds() * 2.0).abs() < 1e-9, "{beat}");
        assert!(beat < 0.0);
    }

    #[test]
    fn the_delay_is_in_music_time_across_a_tempo_change() {
        use peel_time::{TempoSegment, Time};
        // 60 BPM for four beats (4 s), then 120 BPM.
        let tempo = TempoMap::from_segments(&[
            TempoSegment {
                start: Time::ZERO,
                bpm: 60.0,
                ramp_to: None,
            },
            TempoSegment {
                start: Time::beats(4),
                bpm: 120.0,
                ramp_to: None,
            },
        ]);
        // 0.5 s of delay, playhead 0.1 s into the 120 BPM section (beat 4.2).
        let s = CaptureStart {
            playhead_beat: 4.2,
            output_lead_s: 0.25,
            output_block_s: 0.0,
            output_age_s: Some(0.0),
            input_latency_s: 0.25,
        };
        // Back 0.5 s of music time: 0.1 s in the fast section (0.2 beats)
        // and 0.4 s in the slow one (0.4 beats).
        let beat = take_start_beat(&tempo, &s, 0.0);
        assert!((beat - 3.6).abs() < 1e-9, "{beat}");
    }

    #[test]
    fn the_report_adds_up() {
        let r = LatencyReport {
            input_s: 0.008,
            output_s: 0.012,
        };
        assert!((r.round_trip_s() - 0.020).abs() < 1e-12);
    }
}
