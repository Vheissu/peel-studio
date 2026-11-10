mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use common::*;
use peel_dsp::EventKind;
use peel_engine::{
    ChannelRoute, Engine, EngineCommand, EngineConfig, OfflineEngine, RenderPlan, Retired,
    Telemetry, TrackRoute,
};
use peel_time::{TempoMap, TempoSegment, Time};

const SRF: f64 = SR as f64;

fn constant(bpm: f64) -> TempoMap {
    TempoMap::constant(bpm)
}

fn ramp_tempo() -> TempoMap {
    TempoMap::from_segments(&[
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
    ])
}

fn new_log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

// ---------------------------------------------------------------------
// Sample-accurate timing
// ---------------------------------------------------------------------

#[test]
fn events_land_on_exact_samples_for_every_block_size() {
    let tempo = ramp_tempo();
    // Non-overlapping, at awkward positions (thirds, sevenths, the tempo
    // ramp's end), so each is a separate gate pulse.
    let notes = [
        note(0.25, 0.0625, 1),
        note(1.0 / 3.0, 0.125, 2),
        note(2.5, 1.0 / 7.0, 3),
        note(7.75, 0.5, 4),
        note(15.99, 0.005, 5),
        note(16.0, 0.5, 6),
    ];
    let total = 700_000;

    let reference = {
        let mut off = rig(plan(tempo.clone(), &notes, 1, None, 64.0), None);
        off.send(EngineCommand::Play);
        render_blocks(&mut off, total, 256).0
    };

    let on: Vec<usize> = notes
        .iter()
        .map(|n| tempo.beat_to_sample(n.beat, SRF) as usize)
        .collect();
    let offs: Vec<usize> = notes
        .iter()
        .map(|n| tempo.beat_to_sample(n.beat + n.len, SRF) as usize)
        .collect();
    assert_eq!(rising_edges(&reference), on, "note-on samples");
    assert_eq!(falling_edges(&reference), offs, "note-off samples");

    for block in [1usize, 7, 64, 1000, 4096] {
        let mut off = rig(plan(tempo.clone(), &notes, 1, None, 64.0), None);
        off.send(EngineCommand::Play);
        let (l, r) = render_blocks(&mut off, total, block);
        assert!(l == reference, "left differs for block size {block}");
        assert!(r == reference, "right differs for block size {block}");
    }
}

#[test]
fn events_on_a_block_boundary_belong_to_the_next_block() {
    // 120 BPM at 48 kHz: beat 0.5 is exactly sample 12000.
    let notes = [note(0.5, 1.0, 1)];
    for (block, expect_start, expect_offset) in [(1000usize, 12_000u64, 0u32), (999, 11_988, 12)] {
        let log = new_log();
        let config = EngineConfig {
            max_block: 1000,
            ..config()
        };
        let mut off = rig_with(
            plan(constant(120.0), &notes, 1, None, 8.0),
            Some(log.clone()),
            config,
        );
        off.send(EngineCommand::Play);
        render_blocks(&mut off, 14_000, block);
        let log = log.lock().unwrap();
        let on = log
            .iter()
            .find(|e| matches!(e.kind, EventKind::NoteOn { .. }))
            .expect("note-on delivered");
        assert_eq!(on.block_start, expect_start, "block {block}");
        assert_eq!(on.offset, expect_offset, "block {block}");
        assert_eq!(on.abs(), 12_000);
    }
}

#[test]
fn offsets_are_always_inside_the_block() {
    let log = new_log();
    let notes: Vec<_> = (0..200)
        .map(|i| note(i as f64 * 0.0371, 0.02, i as u32))
        .collect();
    let mut off = rig(plan(ramp_tempo(), &notes, 1, None, 64.0), Some(log.clone()));
    off.send(EngineCommand::Play);
    render_blocks(&mut off, 100_000, 37);
    let log = log.lock().unwrap();
    assert!(!log.is_empty());
    assert!(log.iter().all(|e| (e.offset as usize) < 256));
    let mut prev = 0;
    for e in log.iter() {
        assert!(e.abs() >= prev, "events must arrive in time order");
        prev = e.abs();
    }
}

#[test]
fn a_note_at_beat_1000_lands_exactly_on_the_tempo_map_sample() {
    // Ramp for the first 16 beats, then 140 BPM: beat 1000 is far from 0 and
    // not a round sample count.
    let tempo = ramp_tempo();
    let expected = tempo.beat_to_sample(1000.0, SRF) as u64;
    let mut off = rig(plan(tempo, &[note(1000.0, 0.5, 1)], 1, None, 2000.0), None);
    off.send(EngineCommand::Play);
    let mut buf_l = vec![0.0; 4096];
    let mut buf_r = vec![0.0; 4096];
    let mut at = 0u64;
    let mut found = None;
    while at < expected + 10_000 {
        off.render(&mut buf_l, &mut buf_r);
        if let Some(i) = buf_l.iter().position(|&x| x != 0.0) {
            found = Some(at + i as u64);
            break;
        }
        at += 4096;
    }
    assert_eq!(found, Some(expected));
}

#[test]
fn looping_does_not_accumulate_rounding_error() {
    // A loop whose length is not a whole number of samples: 4 beats at 133.3
    // BPM is 86421.6 samples. Rounding each pass would drift by 0.4 samples
    // per loop; the engine must stay on the exact timeline.
    let bpm = 133.3;
    let loop_len = 4.0 * 60.0 / bpm * SRF;
    let note_x = 60.0 / bpm * SRF;
    let mut off = rig(
        plan(
            constant(bpm),
            &[note(1.0, 0.25, 1)],
            1,
            Some((0.0, 4.0)),
            4.0,
        ),
        None,
    );
    off.send(EngineCommand::Play);
    let passes = 100;
    let total = (loop_len * passes as f64) as usize;
    let (l, _) = render_blocks(&mut off, total, 4096);
    let edges = rising_edges(&l);
    assert!(edges.len() >= passes - 1);
    for (k, &e) in edges.iter().enumerate() {
        let ideal = k as f64 * loop_len + note_x;
        assert!(
            (e as f64 - ideal).abs() <= 0.51,
            "pass {k}: sample {e}, ideal {ideal}"
        );
    }
}

// ---------------------------------------------------------------------
// Loops, stop, seek
// ---------------------------------------------------------------------

#[test]
fn loop_wrap_releases_notes_at_the_wrap_and_retriggers() {
    // 240 BPM: a beat is 12000 samples; the loop is 2 beats = 24000 samples.
    let log = new_log();
    let crossing = Note {
        voice: 1,
        ..note(1.5, 1.0, 1)
    };
    let starts_at_loop_start = Note {
        voice: 2,
        key: 64,
        ..note(0.0, 0.25, 2)
    };
    let mut off = rig(
        plan(
            constant(240.0),
            &[crossing, starts_at_loop_start],
            1,
            Some((0.0, 2.0)),
            2.0,
        ),
        Some(log.clone()),
    );
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 80_000, 333);

    // Voice 2 plays 0..3000 of each pass; voice 1 plays 18000..24000 (cut at
    // the wrap). Gate level is 1 per sounding voice.
    // The wrap itself is seamless in the gate signal (release then retrigger
    // at the same sample), so only the note ends and the crossing note's
    // starts show as edges.
    assert_eq!(rising_edges(&l), vec![0, 18_000, 42_000, 66_000]);
    assert_eq!(falling_edges(&l), vec![3_000, 27_000, 51_000, 75_000]);

    let log = log.lock().unwrap();
    let find = |pred: &dyn Fn(&Logged) -> bool| -> Vec<usize> {
        log.iter()
            .enumerate()
            .filter(|(_, e)| pred(e))
            .map(|(i, _)| i)
            .collect()
    };
    let voice1_off = find(&|e| matches!(e.kind, EventKind::NoteOff { key: 60, .. }));
    let abs: Vec<u64> = voice1_off.iter().map(|&i| log[i].abs()).collect();
    assert_eq!(
        &abs[..3],
        &[24_000, 48_000, 72_000],
        "cut exactly at the wrap"
    );
    let voice1_on = find(&|e| matches!(e.kind, EventKind::NoteOn { key: 60, .. }));
    let abs: Vec<u64> = voice1_on.iter().map(|&i| log[i].abs()).collect();
    assert_eq!(
        &abs[..3],
        &[18_000, 42_000, 66_000],
        "retriggered every pass"
    );

    let runtime_ids: Vec<_> = log
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::NoteOn { voice, .. } => Some(voice),
            _ => None,
        })
        .collect();
    let unique_ids: std::collections::HashSet<_> = runtime_ids.iter().copied().collect();
    assert_eq!(
        runtime_ids.len(),
        unique_ids.len(),
        "loop articulations need fresh ids for independent tails"
    );
    // At the wrap sample the release precedes the new note-on.
    let off_idx = voice1_off[0];
    let on_idx = log
        .iter()
        .position(|e| e.abs() == 24_000 && matches!(e.kind, EventKind::NoteOn { key: 64, .. }))
        .unwrap();
    assert!(
        off_idx < on_idx,
        "note-off must precede note-on at equal offsets"
    );
}

#[test]
fn stop_releases_every_sounding_voice() {
    let log = new_log();
    let notes = [
        note(0.0, 100.0, 1),
        note(0.0, 100.0, 2),
        note(1.0, 100.0, 3),
    ];
    let mut off = rig(
        plan(constant(120.0), &notes, 1, None, 200.0),
        Some(log.clone()),
    );
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 30_000, 256);
    assert_eq!(l[29_999], 3.0, "three voices sounding");

    off.send(EngineCommand::Stop { return_to: 0.0 });
    let (l, _) = render_blocks(&mut off, 512, 256);
    assert_eq!(l[0], 0.0, "released at the first sample after stop");
    assert!(l.iter().all(|&x| x == 0.0));
    assert!(!off.is_playing());
    assert_eq!(off.position_beats(), 0.0);

    let log = log.lock().unwrap();
    let offs = log
        .iter()
        .filter(|e| matches!(e.kind, EventKind::NoteOff { .. }))
        .count();
    assert_eq!(offs, 3);

    // Playing again starts the notes again: nothing was left latched.
    drop(log);
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 100, 100);
    assert_eq!(l[0], 2.0);
}

#[test]
fn pause_keeps_position_and_seek_releases_notes() {
    let notes = [note(0.0, 100.0, 1)];
    let mut off = rig(plan(constant(120.0), &notes, 1, None, 200.0), None);
    off.send(EngineCommand::Play);
    render_blocks(&mut off, 24_000, 256);
    off.send(EngineCommand::Pause);
    let (l, _) = render_blocks(&mut off, 256, 256);
    assert_eq!(l[0], 0.0);
    assert!((off.position_beats() - 1.0).abs() < 1e-6);
    // Paused: position does not move.
    render_blocks(&mut off, 5000, 256);
    assert!((off.position_beats() - 1.0).abs() < 1e-6);

    off.send(EngineCommand::Seek { beat: 10.0 });
    assert!((off.position_beats() - 10.0).abs() < 1e-12);
    off.send(EngineCommand::Play);
    render_blocks(&mut off, 12_000, 256);
    assert!((off.position_beats() - 10.5).abs() < 1e-6);
}

#[test]
fn seek_while_playing_reanchors_to_the_new_position() {
    // Note-on at beat 6 plays 12000 samples after a seek to beat 5.5.
    let mut off = rig(
        plan(constant(120.0), &[note(6.0, 1.0, 1)], 1, None, 20.0),
        None,
    );
    off.send(EngineCommand::Play);
    render_blocks(&mut off, 1000, 256);
    off.send(EngineCommand::Seek { beat: 5.5 });
    let (l, _) = render_blocks(&mut off, 20_000, 256);
    assert_eq!(rising_edges(&l), vec![12_000]);
}

#[test]
fn transport_stops_at_the_end_of_the_material_and_releases() {
    let log = new_log();
    // 120 BPM, material 1 beat long (24000 samples); the note runs past it.
    let mut off = rig(
        plan(constant(120.0), &[note(0.5, 10.0, 1)], 1, None, 1.0),
        Some(log.clone()),
    );
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 30_000, 256);
    assert_eq!(rising_edges(&l), vec![12_000]);
    assert_eq!(falling_edges(&l), vec![24_000]);
    assert!(!off.is_playing());
    assert!((off.position_beats() - 1.0).abs() < 1e-12);
    assert!(!off.telemetry().playing.load(Ordering::Relaxed));
}

// ---------------------------------------------------------------------
// Plan swap, devices, retirement
// ---------------------------------------------------------------------

#[test]
fn plan_swap_retires_the_old_plan() {
    let mut off = rig(plan(constant(120.0), &[], 1, None, 4.0), None);
    // The initial empty plan went back when the first plan was installed.
    let first: Vec<_> = off
        .drain_retired()
        .into_iter()
        .filter(|r| !matches!(r, Retired::Compensation(_)))
        .collect();
    assert_eq!(first.len(), 1);
    assert!(matches!(&first[0], Retired::Plan(p) if p.revision == 0));

    let mut second = plan(constant(120.0), &[], 1, None, 4.0);
    second.revision = 2;
    off.send(EngineCommand::SetPlan(Box::new(second)));
    let retired: Vec<_> = off
        .drain_retired()
        .into_iter()
        .filter(|r| !matches!(r, Retired::Compensation(_)))
        .collect();
    assert_eq!(retired.len(), 1);
    assert!(matches!(&retired[0], Retired::Plan(p) if p.revision == 1));
    assert_eq!(off.telemetry().plan_revision.load(Ordering::Relaxed), 2);
}

#[test]
fn plan_swap_keeps_notes_whose_release_is_still_ahead() {
    // The same material reissued mid-note (an edit elsewhere in the project)
    // must not cut the sounding note.
    let notes = [note(0.0, 4.0, 1)];
    let mut off = rig(plan(constant(120.0), &notes, 1, None, 8.0), None);
    off.send(EngineCommand::Play);
    render_blocks(&mut off, 24_000, 256);
    off.send(EngineCommand::SetPlan(Box::new(plan(
        constant(120.0),
        &notes,
        1,
        None,
        8.0,
    ))));
    let (l, _) = render_blocks(&mut off, 100_000, 256);
    assert_eq!(l[0], 1.0, "note keeps sounding across the swap");
    // Still released at its own note-off, beat 4 = sample 96000.
    assert_eq!(falling_edges(&l), vec![96_000 - 24_000]);
}

#[test]
fn a_plan_swap_exactly_at_an_event_loses_no_note() {
    // Swap the plan on the very sample a note starts, at beats that do not
    // convert to whole samples. The new plan's cursor must still include it.
    let tempo = constant(133.3);
    for beat in [0.5, 1.234_567, 3.0 / 7.0, 9.87654321, 20.1] {
        let target = tempo.beat_to_sample(beat, SRF) as usize;
        let notes = [note(beat, 0.25, 1)];
        let mut off = rig(plan(tempo.clone(), &notes, 1, None, 64.0), None);
        off.send(EngineCommand::Play);
        let (l, _) = render_blocks(&mut off, target, 256);
        assert!(l.iter().all(|&x| x == 0.0), "beat {beat}: early");
        off.send(EngineCommand::SetPlan(Box::new(plan(
            tempo.clone(),
            &notes,
            1,
            None,
            64.0,
        ))));
        let (l, _) = render_blocks(&mut off, 100, 100);
        assert_eq!(l[0], 1.0, "beat {beat}: note must start on sample {target}");
    }
}

#[test]
fn plan_swap_releases_notes_whose_release_disappeared() {
    let mut off = rig(
        plan(constant(120.0), &[note(0.0, 4.0, 1)], 1, None, 8.0),
        None,
    );
    off.send(EngineCommand::Play);
    render_blocks(&mut off, 24_000, 256);
    // The note was deleted from the new plan.
    off.send(EngineCommand::SetPlan(Box::new(plan(
        constant(120.0),
        &[],
        1,
        None,
        8.0,
    ))));
    let (l, _) = render_blocks(&mut off, 1000, 256);
    assert!(l.iter().all(|&x| x == 0.0), "released at the swap");
}

#[test]
fn plan_swap_with_a_new_tempo_keeps_the_playhead_continuous() {
    let mut off = rig(plan(constant(120.0), &[], 1, None, 100.0), None);
    off.send(EngineCommand::Play);
    render_blocks(&mut off, 24_000, 256);
    assert!((off.position_beats() - 1.0).abs() < 1e-9);
    off.send(EngineCommand::SetPlan(Box::new(plan(
        constant(240.0),
        &[],
        1,
        None,
        100.0,
    ))));
    render_blocks(&mut off, 12_000, 256);
    // One more beat at the new, faster tempo.
    assert!(
        (off.position_beats() - 2.0).abs() < 1e-9,
        "{}",
        off.position_beats()
    );
}

#[test]
fn device_install_replace_and_remove_are_retired() {
    let mut off = OfflineEngine::new(SR, &config());
    off.send(EngineCommand::InstallDevice {
        slot: 3,
        device: Box::new(Gate::new()),
    });
    assert!(off.drain_retired().is_empty());

    off.send(EngineCommand::InstallDevice {
        slot: 3,
        device: Box::new(Gate::new()),
    });
    let r = off.drain_retired();
    assert_eq!(r.len(), 1);
    assert!(matches!(r[0], Retired::Device(_)));

    off.send(EngineCommand::RemoveDevice { slot: 3 });
    let r = off.drain_retired();
    assert_eq!(r.len(), 1);
    assert!(matches!(r[0], Retired::Device(_)));

    // Out-of-range slots hand the device straight back.
    off.send(EngineCommand::InstallDevice {
        slot: 10_000,
        device: Box::new(Gate::new()),
    });
    let r = off.drain_retired();
    assert!(matches!(r[0], Retired::Device(_)));

    // Samples replaced or refused are retired too.
    let sample = Arc::new(peel_dsp::SampleBuffer::new("s", SR, vec![vec![0.0; 4]]));
    off.send(EngineCommand::SetSample {
        slot: 7,
        sample: Some(sample),
    });
    assert!(matches!(off.drain_retired()[0], Retired::Sample(_)));
}

#[test]
fn oversized_plans_are_refused_and_returned() {
    let mut off = rig(plan(constant(120.0), &[], 1, None, 4.0), None);
    off.drain_retired();
    let mut huge = RenderPlan::empty();
    huge.revision = 99;
    huge.tracks = (0..600).map(|_| TrackRoute::new(None, 0)).collect();
    off.send(EngineCommand::SetPlan(Box::new(huge)));
    let r = off.drain_retired();
    assert!(matches!(&r[0], Retired::Plan(p) if p.revision == 99));
    assert_eq!(off.telemetry().plans_rejected.load(Ordering::Relaxed), 1);
    assert_eq!(off.telemetry().plan_revision.load(Ordering::Relaxed), 1);

    let mut bad_master = RenderPlan::empty();
    bad_master.master = 4;
    off.send(EngineCommand::SetPlan(Box::new(bad_master)));
    assert_eq!(off.telemetry().plans_rejected.load(Ordering::Relaxed), 2);
}

#[test]
fn a_full_retire_queue_parks_objects_instead_of_dropping_them() {
    let config = EngineConfig {
        retire_capacity: 16,
        ..config()
    };
    let mut off = OfflineEngine::new(SR, &config);
    for _ in 0..40 {
        off.send(EngineCommand::InstallDevice {
            slot: 0,
            device: Box::new(Gate::new()),
        });
    }
    // 39 replaced devices; the queue holds 16, the rest are parked.
    let first = off.drain_retired();
    assert_eq!(first.len(), 16);
    // Parked objects flow out on the next blocks.
    let mut l = [0.0; 64];
    let mut r = [0.0; 64];
    off.render(&mut l, &mut r);
    let second = off.drain_retired();
    assert_eq!(second.len(), 16);
    off.render(&mut l, &mut r);
    let third = off.drain_retired();
    assert_eq!(first.len() + second.len() + third.len(), 39);
    assert_eq!(off.telemetry().retire_leaks.load(Ordering::Relaxed), 0);
}

// ---------------------------------------------------------------------
// Mixer
// ---------------------------------------------------------------------

fn held_note_rig() -> OfflineEngine {
    let mut off = rig(
        plan(constant(120.0), &[note(0.0, 1000.0, 1)], 1, None, 2000.0),
        None,
    );
    off.send(EngineCommand::Play);
    off
}

fn max_step(buf: &[f32]) -> f32 {
    buf.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn mute_ramps_without_a_step_discontinuity() {
    let ramp = 0.005 * SR;
    let run = |block: usize| {
        let mut off = held_note_rig();
        let mut out = render_blocks(&mut off, 1000, block).0;
        off.send(EngineCommand::SetChannelMute {
            channel: 0,
            muted: true,
        });
        out.extend(render_blocks(&mut off, 1000, block).0);
        off.send(EngineCommand::SetChannelMute {
            channel: 0,
            muted: false,
        });
        out.extend(render_blocks(&mut off, 1000, block).0);
        out
    };
    let a = run(256);
    assert_eq!(a[999], 1.0);
    assert_eq!(a[1999], 0.0, "fully muted after the ramp");
    assert_eq!(a[2999], 1.0, "fully restored");
    let step = max_step(&a);
    assert!(step <= 1.0 / ramp * 1.01, "largest step {step}");
    assert!(step > 0.0);
    // The ramp reaches silence in about 5 ms.
    let silent_at = a[1000..].iter().position(|&x| x == 0.0).unwrap();
    assert!(
        (silent_at as f32 - ramp).abs() <= 2.0,
        "silent after {silent_at}"
    );
    // Per-sample ramps do not depend on block size.
    assert!(run(1) == a);
    assert!(run(77) == a);
}

#[test]
fn gain_and_pan_changes_are_smoothed() {
    let mut off = held_note_rig();
    render_blocks(&mut off, 100, 100);
    off.send(EngineCommand::SetChannelGain {
        channel: 0,
        gain: 0.25,
    });
    off.send(EngineCommand::SetTrackGain {
        track: 1,
        gain: 0.5,
    });
    let (l, _) = render_blocks(&mut off, 1000, 100);
    // Channel and master ramp together, so the product moves up to twice as fast.
    assert!(max_step(&l) <= 2.0 / (0.005 * SR) * 1.01);
    assert!((l[999] - 0.125).abs() < 1e-6);

    off.send(EngineCommand::SetChannelGain {
        channel: 0,
        gain: 1.0,
    });
    off.send(EngineCommand::SetTrackGain {
        track: 1,
        gain: 1.0,
    });
    off.send(EngineCommand::SetChannelPan {
        channel: 0,
        pan: -1.0,
    });
    let (l, r) = render_blocks(&mut off, 1000, 100);
    // Hard left: the equal-power law puts sqrt(2) on the left, nothing right.
    assert!((l[999] - std::f32::consts::SQRT_2).abs() < 1e-5);
    assert_eq!(r[999], 0.0);
    assert!(max_step(&l) <= 3.0 / (0.005 * SR));
}

#[test]
fn non_finite_output_is_contained_and_counted() {
    let mut off = held_note_rig();
    render_blocks(&mut off, 300, 100);
    off.send(EngineCommand::SetParam {
        slot: 0,
        id: P_POISON,
        value: 1.0,
    });
    let (l, r) = render_blocks(&mut off, 2000, 256);
    assert!(l.iter().chain(r.iter()).all(|x| x.is_finite()));
    assert!(off.telemetry().non_finite.load(Ordering::Relaxed) >= 2);
}

#[test]
fn inserts_process_in_place_and_bypass_crossfades() {
    let mut p = plan(constant(120.0), &[note(0.0, 1000.0, 1)], 1, None, 2000.0);
    p.tracks[0].inserts = vec![10];
    let mut off = rig(p, None);
    off.send(EngineCommand::InstallDevice {
        slot: 10,
        device: Box::new(Half),
    });
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 500, 256);
    assert_eq!(l[499], 0.5, "insert halves the signal");

    off.send(EngineCommand::SetBypass {
        slot: 10,
        bypass: true,
    });
    let (l, _) = render_blocks(&mut off, 1000, 256);
    assert_eq!(l[999], 1.0, "bypassed");
    assert!(
        max_step(&l) <= 0.5 / (0.005 * SR) * 1.05,
        "bypass is crossfaded"
    );

    off.send(EngineCommand::SetBypass {
        slot: 10,
        bypass: false,
    });
    let (l, _) = render_blocks(&mut off, 1000, 256);
    assert_eq!(l[999], 0.5);
}

#[test]
fn tracks_route_in_plan_order_into_the_master() {
    // Two instrument channels -> tracks 0 and 1 -> track 2 -> master (3).
    let mut p = plan(constant(120.0), &[], 2, None, 8.0);
    p.channels[1].track = 1;
    p.channels[1].gain = 0.5;
    p.tracks = vec![
        TrackRoute::new(Some(2), 0),
        TrackRoute::new(Some(2), 1),
        TrackRoute::new(Some(3), 2),
        TrackRoute::new(None, 3),
    ];
    p.tracks[2].gain = 0.5;
    p.master = 3;
    p.sequence = plan(
        constant(120.0),
        &[
            note(0.0, 100.0, 1),
            Note {
                channel: 1,
                ..note(0.0, 100.0, 2)
            },
        ],
        2,
        None,
        8.0,
    )
    .sequence;
    let mut off = rig(p, None);
    off.send(EngineCommand::Play);
    let (l, r) = render_blocks(&mut off, 400, 64);
    // (1.0 + 1.0 * 0.5) * 0.5
    assert_eq!(l[399], 0.75);
    assert_eq!(r[399], 0.75);
}

#[test]
fn a_track_that_routes_backwards_is_dropped_not_aliased() {
    let mut p = plan(constant(120.0), &[note(0.0, 100.0, 1)], 1, None, 8.0);
    // Track 0 routes to itself: invalid, must not feed back or panic.
    p.tracks[0] = TrackRoute::new(Some(0), 0);
    let mut off = rig(p, None);
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 400, 64);
    assert!(l.iter().all(|&x| x == 0.0));
}

#[test]
fn many_idle_tracks_do_not_disturb_the_mix() {
    let mut p = plan(constant(120.0), &[note(0.0, 100.0, 1)], 1, None, 8.0);
    // 100 feeder tracks into the master (index 100); the channel uses one.
    p.tracks = (0..100)
        .map(|i| TrackRoute::new(Some(100), i))
        .chain([TrackRoute::new(None, 100)])
        .collect();
    p.channels[0].track = 7;
    p.master = 100;
    let mut off = rig(p, None);
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 300, 100);
    assert_eq!(l[299], 1.0);
    assert_eq!(off.telemetry().take_meter(7), (1.0, 1.0));
    assert_eq!(off.telemetry().take_meter(8), (0.0, 0.0));
}

// ---------------------------------------------------------------------
// Telemetry
// ---------------------------------------------------------------------

#[test]
fn telemetry_reports_peaks_beat_and_state() {
    let mut p = plan(
        constant(120.0),
        &[Note {
            velocity: 0.5,
            ..note(0.0, 100.0, 1)
        }],
        1,
        None,
        200.0,
    );
    p.revision = 7;
    let mut off = rig(p, None);
    off.send(EngineCommand::Play);
    render_blocks(&mut off, 24_000, 256);
    let t = off.telemetry().clone();
    assert!(t.playing.load(Ordering::Relaxed));
    assert_eq!(t.plan_revision.load(Ordering::Relaxed), 7);
    assert_eq!(t.sample_rate.load(Ordering::Relaxed), 48_000);
    assert!((t.beat() - 1.0).abs() < 1e-9);
    assert_eq!(t.take_slot(0), 0.5);
    assert_eq!(t.take_meter(0), (0.5, 0.5));
    assert_eq!(t.take_meter(1), (0.5, 0.5));
    assert_eq!(t.take_master(), (0.5, 0.5));
    // Reading clears the running maximum.
    assert_eq!(t.take_master(), (0.0, 0.0));
    off.send(EngineCommand::Stop { return_to: 0.0 });
    render_blocks(&mut off, 256, 256);
    assert_eq!(
        t.take_master().0,
        0.0,
        "released note leaves the next block silent"
    );
}

#[test]
fn cpu_load_is_measured_when_metering_is_on() {
    let config = config();
    let telemetry = Arc::new(Telemetry::new());
    let (retire_tx, _retire_rx) = rtrb::RingBuffer::new(64);
    let mut engine = Engine::new(SR, &config, telemetry.clone(), retire_tx, None);
    let mut l = vec![0.0; 256];
    let mut r = vec![0.0; 256];
    for _ in 0..50 {
        engine.process(&mut l, &mut r);
    }
    let load = telemetry.cpu_load();
    assert!(load > 0.0 && load < 10.0, "load {load}");
}

// ---------------------------------------------------------------------
// Overflow policy
// ---------------------------------------------------------------------

fn overflow_config() -> EngineConfig {
    EngineConfig {
        max_block: 16_384,
        events_per_block: 32,
        ..config()
    }
}

#[test]
fn note_ons_beyond_capacity_are_dropped_and_their_offs_skipped() {
    let log = new_log();
    // 40 simultaneous notes into a 32-event buffer.
    let notes: Vec<_> = (0..40).map(|i| note(0.0, 1.0, i)).collect();
    let mut off = rig_with(
        plan(constant(480.0), &notes, 1, None, 8.0),
        Some(log.clone()),
        overflow_config(),
    );
    off.send(EngineCommand::Play);
    // Note-ons and note-offs in separate blocks.
    render_blocks(&mut off, 3_000, 3_000);
    render_blocks(&mut off, 9_000, 9_000);
    let log = log.lock().unwrap();
    let ons = log
        .iter()
        .filter(|e| matches!(e.kind, EventKind::NoteOn { .. }))
        .count();
    let offs = log
        .iter()
        .filter(|e| matches!(e.kind, EventKind::NoteOff { .. }))
        .count();
    assert_eq!(ons, 32);
    assert_eq!(offs, 32, "every started note is released, none else");
    assert_eq!(off.telemetry().dropped_notes.load(Ordering::Relaxed), 8);
}

#[test]
fn note_offs_are_never_dropped_in_favor_of_note_ons() {
    let log = new_log();
    // 480 BPM: a beat is 6000 samples. Thirty notes start at beat 0 and end
    // at beat 1.5; thirty-two more start at beat 1. Both fall in one block,
    // so the buffer fills with note-ons before the note-offs arrive.
    let mut notes: Vec<_> = (0..30).map(|i| note(0.0, 1.5, i)).collect();
    notes.extend((0..32).map(|i| note(1.0, 10.0, 100 + i)));
    let mut off = rig_with(
        plan(constant(480.0), &notes, 1, None, 40.0),
        Some(log.clone()),
        overflow_config(),
    );
    off.send(EngineCommand::Play);
    render_blocks(&mut off, 100, 100);
    render_blocks(&mut off, 12_000, 12_000);
    {
        let log = log.lock().unwrap();
        let released: Vec<u32> = log
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::NoteOff { voice, .. } => Some(voice),
                _ => None,
            })
            .collect();
        let original_voices: Vec<_> = log
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::NoteOn { voice, .. } if e.abs() == 0 => Some(voice),
                _ => None,
            })
            .collect();
        assert_eq!(original_voices.len(), 30);
        for v in original_voices {
            assert!(released.contains(&v), "note-off for voice {v} was lost");
        }
        let started_new = log
            .iter()
            .filter(|e| e.abs() == 6000 && matches!(e.kind, EventKind::NoteOn { .. }))
            .count();
        assert_eq!(started_new, 10, "22 note-ons made room for the note-offs");
    }
    assert_eq!(off.telemetry().dropped_notes.load(Ordering::Relaxed), 22);

    // The ten surviving notes are tracked and released by Stop; nothing hangs.
    off.send(EngineCommand::Stop { return_to: 0.0 });
    let (l, _) = render_blocks(&mut off, 256, 256);
    assert!(l.iter().all(|&x| x == 0.0));
    let log = log.lock().unwrap();
    let total_offs = log
        .iter()
        .filter(|e| matches!(e.kind, EventKind::NoteOff { .. }))
        .count();
    assert_eq!(total_offs, 30 + 10);
}

#[test]
fn releases_that_cannot_fit_fall_back_to_all_notes_off() {
    let log = new_log();
    // events_per_block 32 => at most 40 note-offs fit in one block. Hold 64
    // notes... the engine tracks at most 32 per block, so start them across
    // several blocks, then stop: 64 releases cannot fit in one buffer.
    let config = EngineConfig {
        max_block: 256,
        events_per_block: 32,
        ..config()
    };
    let notes: Vec<_> = (0..60).map(|i| note(i as f64 * 0.01, 100.0, i)).collect();
    let mut off = rig_with(
        plan(constant(120.0), &notes, 1, None, 400.0),
        Some(log.clone()),
        config,
    );
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 24_000, 256);
    assert_eq!(l[23_999], 60.0);
    off.send(EngineCommand::Stop { return_to: 0.0 });
    let (l, _) = render_blocks(&mut off, 256, 256);
    assert!(l.iter().all(|&x| x == 0.0), "everything released");
    assert!(off.telemetry().note_overflows.load(Ordering::Relaxed) >= 1);
    let log = log.lock().unwrap();
    assert!(log.iter().any(|e| matches!(e.kind, EventKind::AllNotesOff)));
}

// ---------------------------------------------------------------------
// Live input, panic, monitoring
// ---------------------------------------------------------------------

#[test]
fn live_notes_play_while_stopped_and_survive_stop() {
    let mut off = rig(plan(constant(120.0), &[], 1, None, 8.0), None);
    off.send(EngineCommand::LiveNote {
        slot: 0,
        key: 64,
        velocity: 0.5,
        on: true,
    });
    let (l, _) = render_blocks(&mut off, 300, 100);
    assert_eq!(l[0], 0.5);
    assert_eq!(l[299], 0.5);
    // Stop only affects sequence notes.
    off.send(EngineCommand::Stop { return_to: 0.0 });
    let (l, _) = render_blocks(&mut off, 100, 100);
    assert_eq!(l[99], 0.5);
    off.send(EngineCommand::LiveNote {
        slot: 0,
        key: 64,
        velocity: 0.0,
        on: false,
    });
    let (l, _) = render_blocks(&mut off, 100, 100);
    assert_eq!(l[0], 0.0);
}

#[test]
fn panic_silences_every_device_and_clears_tracking() {
    let mut off = rig(
        plan(constant(120.0), &[note(0.0, 100.0, 1)], 1, None, 200.0),
        None,
    );
    off.send(EngineCommand::Play);
    off.send(EngineCommand::LiveNote {
        slot: 0,
        key: 70,
        velocity: 1.0,
        on: true,
    });
    let (l, _) = render_blocks(&mut off, 1000, 256);
    assert_eq!(l[999], 2.0);
    off.send(EngineCommand::Panic);
    let (l, _) = render_blocks(&mut off, 256, 256);
    assert_eq!(l[0], 0.0);
    // A stray note-off for a note the panic already killed is harmless.
    off.send(EngineCommand::LiveNote {
        slot: 0,
        key: 70,
        velocity: 0.0,
        on: false,
    });
    off.send(EngineCommand::Stop { return_to: 0.0 });
    let (l, _) = render_blocks(&mut off, 256, 256);
    assert!(l.iter().all(|&x| x == 0.0));
}

fn peak_in(buf: &[f32], range: std::ops::Range<usize>) -> f32 {
    buf[range].iter().fold(0.0f32, |p, x| p.max(x.abs()))
}

#[test]
fn metronome_clicks_on_beats_with_an_accented_downbeat() {
    let mut off = rig(plan(constant(120.0), &[], 0, None, 16.0), None);
    off.set_monitoring(true);
    off.send(EngineCommand::Metronome { enabled: true });
    off.send(EngineCommand::Play);
    let (l, r) = render_blocks(&mut off, 24_000 * 5, 300);
    assert_eq!(l, r);
    let bar = peak_in(&l, 0..1500);
    let beat = peak_in(&l, 24_000..25_500);
    let next_bar_gap = peak_in(&l, 10_000..23_000);
    assert!(bar > beat && beat > 0.1, "accent {bar} vs beat {beat}");
    assert_eq!(next_bar_gap, 0.0, "silent between clicks");
    // Beats 0..4 click at 0, 24000, ...; beat 4 (sample 96000) is the next bar.
    let bar2 = peak_in(&l, 96_000..97_500);
    assert!((bar2 - bar).abs() < 1e-6);
    // Clicks start on the beat sample itself (the waveform opens at zero).
    assert!(l[1] != 0.0);
    assert_eq!(l[24_000 - 1], 0.0);
    assert!(l[24_000 + 1] != 0.0);
    // The click is not on the master meter.
    assert_eq!(off.telemetry().take_master(), (0.0, 0.0));
}

#[test]
fn offline_renders_exclude_the_metronome_and_preview_by_default() {
    let mut off = rig(plan(constant(120.0), &[], 0, None, 16.0), None);
    off.send(EngineCommand::Metronome { enabled: true });
    off.send(EngineCommand::Play);
    let sample = Arc::new(peel_dsp::SampleBuffer::new("s", SR, vec![vec![1.0; 4800]]));
    off.send(EngineCommand::Preview {
        sample: Some(sample),
        gain: 1.0,
    });
    let (l, r) = render_blocks(&mut off, 30_000, 256);
    assert!(l.iter().chain(r.iter()).all(|&x| x == 0.0));
    // The refused preview sample is handed back for the control side to free.
    assert!(
        off.drain_retired()
            .iter()
            .any(|r| matches!(r, Retired::Sample(_)))
    );
}

#[test]
fn preview_plays_after_the_master_and_is_retired_when_done() {
    let mut off = rig(plan(constant(120.0), &[], 0, None, 16.0), None);
    off.set_monitoring(true);
    off.drain_retired();
    let sample = Arc::new(peel_dsp::SampleBuffer::new("s", SR, vec![vec![0.5; 4800]]));
    off.send(EngineCommand::Preview {
        sample: Some(sample),
        gain: 1.0,
    });
    let (l, _) = render_blocks(&mut off, 9600, 256);
    assert_eq!(l[4000], 0.5);
    assert!(l[0] > 0.0 && l[0] < 0.01, "fades in instead of clicking on");
    assert_eq!(l[5000], 0.0, "ended");
    assert!(
        off.drain_retired()
            .iter()
            .any(|r| matches!(r, Retired::Sample(_)))
    );
    // The preview never reaches the master meter.
    assert_eq!(off.telemetry().take_master(), (0.0, 0.0));
}

// ---------------------------------------------------------------------
// Command queue
// ---------------------------------------------------------------------

#[test]
fn queued_commands_apply_at_the_next_block_and_are_bounded_per_block() {
    let config = config();
    let telemetry = Arc::new(Telemetry::new());
    let (retire_tx, _retire_rx) = rtrb::RingBuffer::new(64);
    let (mut cmd_tx, cmd_rx) = rtrb::RingBuffer::new(4096);
    let mut engine = Engine::new(SR, &config, telemetry.clone(), retire_tx, Some(cmd_rx));
    let mut l = vec![0.0; 64];
    let mut r = vec![0.0; 64];

    cmd_tx
        .push(EngineCommand::InstallDevice {
            slot: 0,
            device: Box::new(Gate::new()),
        })
        .ok()
        .unwrap();
    cmd_tx
        .push(EngineCommand::SetPlan(Box::new(plan(
            constant(120.0),
            &[note(0.0, 100.0, 1)],
            1,
            None,
            8.0,
        ))))
        .ok()
        .unwrap();
    cmd_tx.push(EngineCommand::Play).ok().unwrap();
    engine.process(&mut l, &mut r);
    assert_eq!(l[0], 1.0, "commands applied before the block rendered");

    // A burst larger than the per-block bound drains over several blocks.
    for _ in 0..3000 {
        cmd_tx
            .push(EngineCommand::SetChannelGain {
                channel: 0,
                gain: 1.0,
            })
            .ok()
            .unwrap();
    }
    cmd_tx
        .push(EngineCommand::SetChannelMute {
            channel: 0,
            muted: true,
        })
        .ok()
        .unwrap();
    engine.process(&mut l, &mut r);
    assert!(l[63] > 0.0, "mute is behind the first 1024 commands");
    engine.process(&mut l, &mut r);
    engine.process(&mut l, &mut r);
    engine.process(&mut l, &mut r);
    // Fully drained and ramped to silence.
    let mut big = vec![0.0; 1000];
    let mut big_r = vec![0.0; 1000];
    engine.process(&mut big, &mut big_r);
    assert_eq!(big[999], 0.0);
}

#[test]
fn process_accepts_any_frame_count_and_zero_fills_a_mismatch() {
    let config = config();
    let telemetry = Arc::new(Telemetry::new());
    let (retire_tx, _rx) = rtrb::RingBuffer::new(8);
    let mut engine = Engine::new(SR, &config, telemetry, retire_tx, None);
    let mut l = vec![1.0; 1000];
    let mut r = vec![1.0; 700];
    engine.process(&mut l, &mut r);
    assert!(l.iter().all(|&x| x == 0.0));
    assert!(r.iter().all(|&x| x == 0.0));
    engine.process(&mut [], &mut []);
    assert_eq!(engine.engine_sample(), 700);
}

#[test]
fn channel_routes_with_bad_indices_are_ignored() {
    let mut p = plan(constant(120.0), &[note(0.0, 100.0, 1)], 1, None, 8.0);
    p.channels.push(ChannelRoute {
        slot: 9_999,
        track: 0,
        gain: 1.0,
        pan: 0.0,
        muted: false,
    });
    p.channels.push(ChannelRoute {
        slot: 1,
        track: 77,
        gain: 1.0,
        pan: 0.0,
        muted: false,
    });
    let mut off = rig(p, None);
    off.send(EngineCommand::InstallDevice {
        slot: 1,
        device: Box::new(Gate::new()),
    });
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 300, 100);
    assert_eq!(l[299], 1.0);
}

#[test]
fn play_past_the_loop_end_starts_at_the_loop_start() {
    // 240 BPM: a beat is 12000 samples. The playhead sits beyond a two-beat
    // loop, as it does after a longer pattern or a song.
    let mut off = rig(
        plan(
            constant(240.0),
            &[note(0.5, 0.25, 1)],
            1,
            Some((0.0, 2.0)),
            2.0,
        ),
        None,
    );
    off.send(EngineCommand::Seek { beat: 9.0 });
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 48_000, 256);
    assert_eq!(rising_edges(&l), vec![6_000, 30_000]);
}

#[test]
fn play_after_the_end_starts_over() {
    let mut off = rig(
        plan(constant(240.0), &[note(0.5, 0.25, 1)], 1, None, 2.0),
        None,
    );
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 30_000, 256);
    assert_eq!(rising_edges(&l), vec![6_000]);
    assert!(!off.engine().is_playing());
    off.send(EngineCommand::Play);
    let (l, _) = render_blocks(&mut off, 12_000, 256);
    assert_eq!(rising_edges(&l), vec![6_000]);
}
