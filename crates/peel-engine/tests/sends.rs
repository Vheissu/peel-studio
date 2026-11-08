//! Post-fader sends mix a track into a later bus.

mod common;

use std::sync::Arc;

use common::*;
use peel_dsp::SampleBuffer;
use peel_engine::{AUTOMATION_UNSET, AudioRegion, EngineCommand, TrackRoute, TrackSend};
use peel_time::TempoMap;

const BEAT: usize = 24_000;

#[test]
fn send_adds_a_scaled_copy_through_the_bus() {
    let mut p = plan(TempoMap::constant(120.0), &[], 0, None, 16.0);
    // Track 0 -> master (2), sending half to bus 1 which also feeds master.
    let mut source = TrackRoute::new(Some(2), 0);
    source.sends.push(TrackSend {
        dest: 1,
        gain: 0.5,
        last: AUTOMATION_UNSET,
    });
    p.tracks = vec![
        source,
        TrackRoute::new(Some(2), 1),
        TrackRoute::new(None, 2),
    ];
    p.master = 2;
    p.audio = vec![AudioRegion {
        track: 0,
        sample: Arc::new(SampleBuffer::new("dc", SR, vec![vec![0.4; BEAT * 8]])),
        start_beat: 0.0,
        end_beat: 8.0,
        source_start_seconds: 0.0,
        gain: 1.0,
        reversed: false,
        fade_in_beats: 0.0,
        fade_out_beats: 0.0,
    }];
    let mut off = rig(p, None);
    off.send(EngineCommand::Play);
    let (left, _) = render_blocks(&mut off, BEAT * 2, 256);
    let v = left[BEAT];
    assert!(
        (v - 0.6).abs() < 1e-4,
        "0.4 direct + 0.2 through the bus = {v}"
    );
}

#[test]
fn backward_sends_are_ignored() {
    let mut p = plan(TempoMap::constant(120.0), &[], 0, None, 16.0);
    let mut later = TrackRoute::new(Some(1), 0);
    // A send to itself or an earlier track would alias buffers; it is dropped.
    later.sends.push(TrackSend {
        dest: 0,
        gain: 1.0,
        last: AUTOMATION_UNSET,
    });
    p.tracks = vec![later, TrackRoute::new(None, 1)];
    let mut off = rig(p, None);
    off.send(EngineCommand::Play);
    let (left, _) = render_blocks(&mut off, 4096, 256);
    assert!(left.iter().all(|x| x.is_finite()));
}
