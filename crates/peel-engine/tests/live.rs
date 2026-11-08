//! The live input path: queue, sustain pedal, target routing and recording.

mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use common::*;
use peel_dsp::EventKind;
use peel_engine::live::{LIVE_QUEUE_CAPACITY, MAX_SUSTAINED, RECORD_CAPACITY};
use peel_engine::{EngineCommand, LIVE_VOICE_BASE, LiveTarget, OfflineEngine};
use peel_time::TempoMap;

const BLOCK: usize = 256;

fn empty_plan(channels: usize) -> peel_engine::RenderPlan {
    plan(TempoMap::constant(120.0), &[], channels, None, 64.0)
}

fn new_log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

/// Master output of the next block, as the sum of held velocities.
fn level(off: &mut OfflineEngine) -> f32 {
    let (l, _) = render_blocks(off, BLOCK, BLOCK);
    assert!(
        l.iter().all(|&x| x == l[0]),
        "level changed inside the block"
    );
    l[0]
}

fn kinds(log: &Log) -> Vec<(u64, EventKind)> {
    log.lock()
        .unwrap()
        .iter()
        .map(|e| (e.abs(), e.kind))
        .collect()
}

#[test]
fn live_notes_from_the_queue_play_the_instrument() {
    let log = new_log();
    let mut off = rig(empty_plan(1), Some(log.clone()));
    let live = off.take_live_input().expect("live input");
    assert!(
        off.take_live_input().is_none(),
        "the producer is taken once"
    );

    assert!(live.note(0, 60, 0.8, true));
    render_blocks(&mut off, BLOCK, BLOCK);
    assert!(live.note(0, 60, 0.0, false));
    render_blocks(&mut off, BLOCK, BLOCK);

    let events = kinds(&log);
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(
        events[0],
        (
            0,
            EventKind::NoteOn {
                key: 60,
                velocity: 0.8,
                voice: LIVE_VOICE_BASE + 60
            }
        )
    );
    assert!(matches!(
        events[1],
        (
            256,
            EventKind::NoteOff {
                key: 60,
                voice: v,
                ..
            }
        ) if v == LIVE_VOICE_BASE + 60
    ));
}

#[test]
fn velocity_is_clamped_and_note_ons_with_no_instrument_are_ignored() {
    let mut off = rig(empty_plan(1), None);
    let live = off.take_live_input().unwrap();
    // Slot 9 has no device: dropped by the engine like any live note.
    assert!(live.note(9, 60, 1.0, true));
    live.note(0, 61, 7.5, true);
    live.note(0, 255, f32::NAN, true);
    assert_eq!(level(&mut off), 1.0, "61 is clamped to 1, 127 is silent");

    let target = live.target();
    assert_eq!(target.get(), None);
    assert!(!live.note_to_target(62, 1.0, true), "no target, no note");
    target.set(Some(0));
    assert!(live.note_to_target(62, 0.5, true));
    assert_eq!(level(&mut off), 1.5);
}

#[test]
fn the_sustain_pedal_holds_note_offs_until_it_lifts() {
    let mut off = rig(empty_plan(1), None);
    let live = off.take_live_input().unwrap();

    live.sustain(true);
    live.note(0, 60, 1.0, true);
    assert_eq!(level(&mut off), 1.0);
    live.note(0, 60, 0.0, false);
    assert_eq!(level(&mut off), 1.0, "released key keeps sounding");
    assert_eq!(level(&mut off), 1.0);
    live.sustain(false);
    assert_eq!(level(&mut off), 0.0, "lifting the pedal releases it");

    // Without the pedal a release is immediate.
    live.note(0, 60, 1.0, true);
    live.note(0, 60, 0.0, false);
    assert_eq!(level(&mut off), 0.0);
}

#[test]
fn sustained_notes_are_released_in_the_order_they_ended() {
    let log = new_log();
    let mut off = rig(empty_plan(1), Some(log.clone()));
    let live = off.take_live_input().unwrap();

    live.sustain(true);
    live.note(0, 60, 1.0, true);
    live.note(0, 64, 1.0, true);
    live.note(0, 67, 1.0, true);
    live.note(0, 64, 0.0, false);
    live.note(0, 60, 0.0, false);
    live.note(0, 64, 0.0, false); // repeated release: still one
    render_blocks(&mut off, BLOCK, BLOCK);
    assert_eq!(kinds(&log).len(), 3, "only the note-ons so far");

    live.sustain(false);
    render_blocks(&mut off, BLOCK, BLOCK);
    let released: Vec<u8> = kinds(&log)
        .into_iter()
        .filter_map(|(at, k)| match k {
            EventKind::NoteOff { key, .. } => {
                assert_eq!(at, BLOCK as u64, "all at the start of the block");
                Some(key)
            }
            _ => None,
        })
        .collect();
    assert_eq!(released, vec![64, 60], "67 is still held down");
    assert_eq!(level(&mut off), 1.0);
}

#[test]
fn striking_a_sustained_key_again_retriggers_and_keeps_the_new_note() {
    let mut off = rig(empty_plan(1), None);
    let live = off.take_live_input().unwrap();
    live.sustain(true);
    live.note(0, 60, 0.5, true);
    live.note(0, 60, 0.0, false);
    live.note(0, 60, 0.75, true);
    assert_eq!(level(&mut off), 0.75, "one voice, the new strike");
    live.sustain(false);
    assert_eq!(level(&mut off), 0.75, "the key is still down");
    live.note(0, 60, 0.0, false);
    assert_eq!(level(&mut off), 0.0);
}

#[test]
fn all_notes_off_releases_held_and_sustained_notes_of_the_instrument() {
    let mut off = rig(empty_plan(2), None);
    let live = off.take_live_input().unwrap();
    live.sustain(true);
    live.note(0, 60, 1.0, true);
    live.note(0, 62, 1.0, true);
    live.note(0, 62, 0.0, false); // sustained
    live.note(1, 61, 0.25, true);
    assert_eq!(level(&mut off), 2.25);
    live.all_notes_off(0);
    assert_eq!(level(&mut off), 0.25, "only the other instrument is left");
    // Nothing is left for the pedal to release on slot 0.
    live.sustain(false);
    live.note(1, 61, 0.0, false);
    assert_eq!(level(&mut off), 0.0);
}

#[test]
fn the_panic_button_works_with_no_target() {
    let mut off = rig(empty_plan(2), None);
    let live = off.take_live_input().unwrap();
    let target = live.target().clone();
    target.set(Some(0));
    live.sustain(true);
    live.note_to_target(60, 1.0, true);
    target.set(Some(1));
    live.note_to_target(62, 0.5, true);
    live.note_to_target(62, 0.0, false); // held by the pedal
    assert_eq!(level(&mut off), 1.5);
    target.set(None);
    assert!(live.all_notes_off_target());
    assert_eq!(level(&mut off), 0.0, "every instrument went quiet");
}

#[test]
fn a_note_off_follows_its_note_on_after_the_target_changes() {
    let log = new_log();
    let mut off = rig(empty_plan(2), Some(log.clone()));
    let live = off.take_live_input().unwrap();
    let target: LiveTarget = live.target().clone();

    target.set(Some(0));
    live.note_to_target(60, 1.0, true);
    render_blocks(&mut off, BLOCK, BLOCK);
    // The player selects another channel while holding the key.
    target.set(Some(1));
    live.note_to_target(60, 0.0, false);
    assert_eq!(level(&mut off), 0.0, "slot 0's note ended");
    assert!(
        kinds(&log)
            .iter()
            .any(|(_, k)| matches!(k, EventKind::NoteOff { key: 60, .. })),
        "the release reached slot 0"
    );

    // With no target at all, a release still ends the note.
    target.set(Some(0));
    live.note_to_target(61, 1.0, true);
    render_blocks(&mut off, BLOCK, BLOCK);
    target.set(None);
    assert!(live.note_to_target(61, 0.0, false));
    assert_eq!(level(&mut off), 0.0);
}

#[test]
fn panic_releases_everything_and_forgets_the_pedal() {
    let mut off = rig(empty_plan(1), None);
    let live = off.take_live_input().unwrap();
    live.sustain(true);
    live.note(0, 60, 1.0, true);
    live.note(0, 64, 1.0, true);
    live.note(0, 64, 0.0, false);
    assert_eq!(level(&mut off), 2.0, "64 is held by the pedal");
    off.send(EngineCommand::Panic);
    assert_eq!(level(&mut off), 0.0);
    // The pedal state is gone: a plain release now works immediately.
    live.note(0, 62, 1.0, true);
    live.note(0, 62, 0.0, false);
    assert_eq!(level(&mut off), 0.0);
}

#[test]
fn the_queue_is_bounded_and_keeps_room_for_releases() {
    let mut off = rig(empty_plan(1), None);
    let live = off.take_live_input().unwrap();

    let mut accepted = 0;
    for i in 0..LIVE_QUEUE_CAPACITY + 100 {
        if live.note(0, (i % 100) as u8, 0.5, true) {
            accepted += 1;
        }
    }
    assert!(accepted < LIVE_QUEUE_CAPACITY, "note-ons stop before full");
    assert!(accepted > LIVE_QUEUE_CAPACITY - 200);
    assert_eq!(
        live.stats().dropped.load(Ordering::Relaxed) as usize,
        LIVE_QUEUE_CAPACITY + 100 - accepted
    );
    // Releases and the pedal lift still fit in the reserve.
    assert!(live.note(0, 1, 0.0, false));
    assert!(live.sustain(false));
    assert!(live.all_notes_off(0));

    // The engine drains a bounded number per block and carries on.
    for _ in 0..8 {
        render_blocks(&mut off, BLOCK, BLOCK);
    }
    assert!(live.note(0, 2, 0.5, true), "room again");
    live.all_notes_off(0);
    assert_eq!(level(&mut off), 0.0);
}

#[test]
fn the_sustain_store_is_bounded_across_instruments() {
    // The test gate holds 64 voices, so 9 instruments x 64 keys is the most
    // that can be sounding, and more than the store can hold back.
    let (channels, keys) = (9usize, 64u8);
    assert!(channels * keys as usize > MAX_SUSTAINED);
    let mut off = rig(empty_plan(channels), None);
    let live = off.take_live_input().unwrap();
    live.sustain(true);
    for slot in 0..channels as u32 {
        for key in 0..keys {
            live.note(slot, key, 0.01, true);
            live.note(slot, key, 0.0, false);
            if key % 32 == 31 {
                render_blocks(&mut off, BLOCK, BLOCK);
            }
        }
    }
    // The first MAX_SUSTAINED releases are held; the rest were not stored,
    // so those notes ended at once.
    let held = level(&mut off);
    let expected = MAX_SUSTAINED as f32 * 0.01;
    assert!((held - expected).abs() < 0.01, "{held} vs {expected}");
    live.sustain(false);
    assert_eq!(level(&mut off), 0.0, "every one is released on lift");
    assert_eq!(off.telemetry().dropped_notes.load(Ordering::Relaxed), 0);
}

#[test]
fn recorded_notes_carry_the_position_and_state_at_arrival() {
    let mut off = rig(
        plan(TempoMap::constant(120.0), &[], 1, Some((0.0, 64.0)), 64.0),
        None,
    );
    let live = off.take_live_input().unwrap();
    let mut recorded = off.take_recorded().unwrap();
    assert!(off.take_recorded().is_none());

    live.note(0, 60, 0.5, true);
    render_blocks(&mut off, BLOCK, BLOCK);
    off.send(EngineCommand::Play);
    render_blocks(&mut off, 24_000 * 2, BLOCK); // two beats at 120 bpm
    live.sustain(true);
    live.note(0, 60, 0.0, false);
    live.note(0, 62, 1.0, true);

    let mut out = Vec::new();
    assert_eq!(recorded.drain_into(&mut out), 3);
    assert_eq!(recorded.drain_into(&mut out), 0);
    let [first, second, third] = out[..] else {
        panic!("{out:?}")
    };

    assert!(first.on && first.key == 60 && first.velocity == 0.5);
    assert!(!first.playing && first.beat == 0.0 && !first.sustain);
    assert!(!second.on && second.key == 60 && second.velocity == 0.0);
    assert!(second.playing && second.sustain);
    assert!((second.beat - 2.0).abs() < 0.02, "{}", second.beat);
    assert_eq!(third.slot, 0);
    assert!(third.time_ns >= second.time_ns && second.time_ns >= first.time_ns);
}

#[test]
fn the_record_ring_is_bounded() {
    let mut off = rig(empty_plan(1), None);
    let live = off.take_live_input().unwrap();
    let mut recorded = off.take_recorded().unwrap();
    // Nobody drains the ring while the engine plays notes.
    for i in 0..RECORD_CAPACITY + 500 {
        assert!(live.note(0, (i % 100) as u8, 0.5, i % 2 == 0));
        if i % 200 == 199 {
            render_blocks(&mut off, BLOCK, BLOCK);
        }
    }
    assert_eq!(live.stats().record_dropped.load(Ordering::Relaxed), 500);
    let mut out = Vec::new();
    assert_eq!(recorded.drain_into(&mut out), RECORD_CAPACITY);
    // Room again afterwards.
    assert!(live.note(0, 1, 0.5, true));
    assert!(recorded.pop().is_some());
}

#[test]
fn input_activity_is_counted() {
    let mut off = rig(empty_plan(1), None);
    let live = off.take_live_input().unwrap();
    assert_eq!(live.stats().note_ons(), 0);
    live.note(0, 60, 0.5, true);
    live.note(0, 60, 0.0, false);
    live.sustain(true);
    assert_eq!(live.stats().note_ons(), 1);
    assert_eq!(live.stats().note_offs.load(Ordering::Relaxed), 1);
    assert_eq!(live.stats().events(), 3);
    drop(off);
    assert!(live.is_closed(), "the engine is gone");
}

#[test]
fn live_notes_and_the_command_queue_coexist() {
    // The computer keyboard still plays through LiveNote commands, and the
    // two sources end each other's notes on the same key (same voice id).
    let mut off = rig(empty_plan(1), None);
    let live = off.take_live_input().unwrap();
    off.send(EngineCommand::LiveNote {
        slot: 0,
        key: 60,
        velocity: 0.5,
        on: true,
    });
    assert_eq!(level(&mut off), 0.5);
    live.note(0, 60, 0.0, false);
    assert_eq!(level(&mut off), 0.0);
}
