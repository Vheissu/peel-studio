//! Changing tempo during playback keeps the musical position continuous.

mod common;

use common::*;
use peel_engine::EngineCommand;
use peel_time::TempoMap;

#[test]
fn tempo_change_mid_play_keeps_position() {
    let mut off = rig(plan(TempoMap::constant(120.0), &[], 0, None, 64.0), None);
    off.send(EngineCommand::Play);
    // Two beats at 120 BPM.
    render_blocks(&mut off, 48_000, 256);
    let before = off.position_beats();
    assert!((before - 2.0).abs() < 0.02, "{before}");
    // Swap in the same plan at 60 BPM.
    off.send(EngineCommand::SetPlan(Box::new(plan(
        TempoMap::constant(60.0),
        &[],
        0,
        None,
        64.0,
    ))));
    render_blocks(&mut off, 256, 256);
    let after_swap = off.position_beats();
    assert!(
        (after_swap - before).abs() < 0.02,
        "no jump at the swap: {before} -> {after_swap}"
    );
    // One more second at 60 BPM is one beat.
    render_blocks(&mut off, 48_000, 256);
    let later = off.position_beats();
    assert!(
        (later - after_swap - 1.0).abs() < 0.02,
        "{after_swap} -> {later}"
    );
}
