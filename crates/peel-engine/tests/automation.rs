//! Automation lanes drive track gain and device parameters while playing.

mod common;

use std::sync::Arc;

use common::*;
use peel_dsp::SampleBuffer;
use peel_engine::{AUTOMATION_UNSET, AudioRegion, AutomationLane, AutomationTarget, EngineCommand};
use peel_time::TempoMap;

/// One beat at 120 BPM and 48 kHz.
const BEAT: usize = 24_000;

fn dc_region(frames: usize) -> AudioRegion {
    AudioRegion {
        track: 0,
        sample: Arc::new(SampleBuffer::new("dc", SR, vec![vec![0.5; frames]])),
        start_beat: 0.0,
        end_beat: 16.0,
        source_start_seconds: 0.0,
        gain: 1.0,
        reversed: false,
        fade_in_beats: 0.0,
        fade_out_beats: 0.0,
    }
}

fn lane(target: AutomationTarget, points: Vec<(f64, f32)>, start: f64, end: f64) -> AutomationLane {
    AutomationLane {
        target,
        start,
        end,
        points,
        last: AUTOMATION_UNSET,
    }
}

#[test]
fn value_at_interpolates_and_holds() {
    let l = lane(
        AutomationTarget::TrackGain(0),
        vec![(1.0, 0.0), (3.0, 1.0)],
        0.0,
        4.0,
    );
    assert_eq!(l.value_at(0.5), Some(0.0), "holds before the first point");
    assert_eq!(l.value_at(2.0), Some(0.5));
    assert_eq!(l.value_at(3.5), Some(1.0), "holds after the last point");
    assert_eq!(l.value_at(4.0), None, "inactive at the end");
    assert_eq!(l.value_at(-0.1), None);
}

#[test]
fn track_gain_follows_the_envelope() {
    let mut p = plan(TempoMap::constant(120.0), &[], 0, None, 16.0);
    p.audio = vec![dc_region(BEAT * 20)];
    // Ramp the track's gain from 1 down to 0 over beats 2..4.
    p.automation = vec![lane(
        AutomationTarget::TrackGain(0),
        vec![(2.0, 1.0), (4.0, 0.0)],
        0.0,
        8.0,
    )];
    let mut off = rig(p, None);
    off.send(EngineCommand::Play);
    let (left, _) = render_blocks(&mut off, BEAT * 6, 256);
    let level = |beat: f64| left[(beat * BEAT as f64) as usize].abs();
    assert!(
        (level(1.0) - 0.5).abs() < 0.02,
        "full level before the ramp: {}",
        level(1.0)
    );
    assert!(
        (level(3.0) - 0.25).abs() < 0.03,
        "halfway down the ramp: {}",
        level(3.0)
    );
    assert!(level(5.0) < 0.01, "silent after the ramp: {}", level(5.0));
}

#[test]
fn many_lanes_render_cleanly() {
    let mut p = plan(TempoMap::constant(120.0), &[], 0, None, 16.0);
    p.automation = (0..32)
        .map(|i| {
            lane(
                AutomationTarget::TrackPan(0),
                vec![(0.0, -1.0), (8.0 + i as f64, 1.0)],
                0.0,
                16.0,
            )
        })
        .collect();
    let mut off = rig(p, None);
    off.send(EngineCommand::Play);
    let (left, _) = render_blocks(&mut off, BEAT * 2, 256);
    assert!(left.iter().all(|x| x.is_finite()));
}
