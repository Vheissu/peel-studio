//! Runs the realtime path under `assert_no_alloc`.
//!
//! Needs `cargo test -p peel-engine --features rt-check`. In a debug build any
//! allocation or deallocation inside `process` aborts the test process.
#![cfg(all(feature = "rt-check", debug_assertions))]

mod common;

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use common::*;
use peel_dsp::SampleBuffer;
use peel_engine::{Engine, EngineCommand, EngineConfig, Retired, Telemetry};
use peel_time::TempoMap;

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

fn region(
    sample: &Arc<SampleBuffer>,
    track: u16,
    start: f64,
    end: f64,
    reversed: bool,
) -> peel_engine::AudioRegion {
    peel_engine::AudioRegion {
        track,
        sample: sample.clone(),
        start_beat: start,
        end_beat: end,
        source_start_seconds: 0.01,
        gain: 0.5,
        reversed,
        fade_in_beats: 0.25,
        fade_out_beats: 0.5,
    }
}

fn song(revision: u64, audio: &Arc<SampleBuffer>) -> Box<peel_engine::RenderPlan> {
    let mut notes = Vec::new();
    for i in 0..64u32 {
        notes.push(Note {
            channel: (i % 2) as u16,
            key: 40 + (i % 24) as u8,
            ..note(i as f64 * 0.25, 0.4, i)
        });
    }
    let mut p = plan(TempoMap::constant(140.0), &notes, 2, Some((0.0, 8.0)), 16.0);
    p.revision = revision;
    p.channels[1].track = 1;
    p.tracks = vec![
        peel_engine::TrackRoute::new(Some(2), 0),
        peel_engine::TrackRoute::new(Some(2), 1),
        peel_engine::TrackRoute::new(None, 2),
    ];
    p.tracks[0].inserts = vec![10];
    p.master = 2;
    // Regions that cross the loop end and the seek targets, one reversed,
    // one at another sample rate, plus many short ones.
    p.audio = vec![
        region(audio, 0, 1.0, 12.0, false),
        region(audio, 1, 3.0, 9.0, true),
        region(audio, 0, 6.5, 7.0, false),
    ];
    let slow = Arc::new(SampleBuffer::new(
        "slow",
        SR / 2.0,
        vec![vec![0.3; audio.frames() / 2]],
    ));
    p.audio.push(region(&slow, 1, 2.0, 7.5, false));
    for i in 0..40 {
        p.audio.push(region(
            audio,
            (i % 2) as u16,
            i as f64 * 0.2,
            i as f64 * 0.2 + 0.3,
            false,
        ));
    }
    Box::new(p)
}

#[test]
fn the_realtime_path_never_allocates() {
    let config = EngineConfig {
        max_block: 256,
        device_slots: 32,
        events_per_block: 64,
        ..EngineConfig::default()
    };
    let telemetry = Arc::new(Telemetry::new());
    let (retire_tx, mut retire_rx) = rtrb::RingBuffer::<Retired>::new(256);
    let (mut cmd_tx, cmd_rx) = rtrb::RingBuffer::<EngineCommand>::new(1024);
    let mut engine = Engine::new(SR, &config, telemetry.clone(), retire_tx, Some(cmd_rx));

    // Everything below that allocates runs outside the guard, like the
    // control thread would.
    let mut send = |cmd: EngineCommand| assert!(cmd_tx.push(cmd).is_ok());
    for slot in 0..2u32 {
        send(EngineCommand::InstallDevice {
            slot,
            device: Box::new(Gate::new()),
        });
    }
    send(EngineCommand::InstallDevice {
        slot: 10,
        device: Box::new(Half),
    });
    let sample = Arc::new(SampleBuffer::new("click", SR, vec![vec![0.25; 9_600]]));
    let audio = Arc::new(SampleBuffer::new(
        "audio",
        SR,
        vec![
            (0..200_000)
                .map(|i| (i as f32 * 0.01).sin() * 0.4)
                .collect(),
            (0..200_000)
                .map(|i| (i as f32 * 0.013).sin() * 0.4)
                .collect(),
        ],
    ));
    send(EngineCommand::SetPlan(song(1, &audio)));

    let mut left = vec![0.0f32; 256];
    let mut right = vec![0.0f32; 256];
    let mut peak = 0.0f32;
    let mut tails_retired = 0;
    for block in 0..600u32 {
        match block {
            1 => send(EngineCommand::Play),
            2 => send(EngineCommand::Metronome { enabled: true }),
            40 => send(EngineCommand::SetPlan(song(2, &audio))),
            160 => send(EngineCommand::Seek { beat: 4.2 }),
            170 => send(EngineCommand::Seek { beat: 10.5 }),
            180 => send(EngineCommand::SetPlan(song(3, &audio))),
            190 => send(EngineCommand::SetTrackMute {
                track: 1,
                muted: true,
            }),
            195 => send(EngineCommand::SetTrackMute {
                track: 1,
                muted: false,
            }),
            50 => send(EngineCommand::SetChannelMute {
                channel: 0,
                muted: true,
            }),
            55 => send(EngineCommand::SetChannelMute {
                channel: 0,
                muted: false,
            }),
            60 => send(EngineCommand::SetBypass {
                slot: 10,
                bypass: true,
            }),
            70 => send(EngineCommand::SetBypass {
                slot: 10,
                bypass: false,
            }),
            80 => send(EngineCommand::Preview {
                sample: Some(sample.clone()),
                gain: 0.8,
            }),
            100 => send(EngineCommand::LiveNote {
                slot: 1,
                key: 72,
                velocity: 0.9,
                on: true,
            }),
            110 => send(EngineCommand::LiveNote {
                slot: 1,
                key: 72,
                velocity: 0.0,
                on: false,
            }),
            150 => send(EngineCommand::Seek { beat: 3.3 }),
            200 => send(EngineCommand::SetParam {
                slot: 0,
                id: P_GAIN,
                value: 0.5,
            }),
            210 => send(EngineCommand::SetTrackGain {
                track: 2,
                gain: 0.7,
            }),
            220 => send(EngineCommand::SetTrackPan {
                track: 0,
                pan: -0.5,
            }),
            300 => send(EngineCommand::Pause),
            310 => send(EngineCommand::Play),
            330 => send(EngineCommand::SetPlan(Box::new(
                peel_engine::RenderPlan::empty(),
            ))),
            340 => send(EngineCommand::SetPlan(song(4, &audio))),
            400 => send(EngineCommand::InstallDevice {
                slot: 0,
                device: Box::new(Gate::new()),
            }),
            450 => send(EngineCommand::SetParam {
                slot: 1,
                id: P_POISON,
                value: 1.0,
            }),
            500 => send(EngineCommand::Panic),
            520 => send(EngineCommand::Stop { return_to: 0.0 }),
            530 => send(EngineCommand::Play),
            _ => {}
        }
        assert_no_alloc(|| engine.process(&mut left, &mut right));
        peak = left
            .iter()
            .chain(right.iter())
            .fold(peak, |p, x| p.max(x.abs()));
        // The control thread frees what the engine retired.
        while let Ok(item) = retire_rx.pop() {
            if let Retired::Sample(s) = &item
                && Arc::ptr_eq(s, &audio)
            {
                tails_retired += 1;
            }
            drop(item);
        }
    }
    assert!(peak > 0.0, "the song produced sound");
    assert!(
        tails_retired > 10,
        "audio regions were cut and their tails retired ({tails_retired})"
    );
    assert!(
        telemetry
            .non_finite
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0,
        "the non-finite path ran too"
    );
}

#[test]
fn guard_reports_enabled() {
    assert!(std::hint::black_box(peel_engine::rt::ENABLED));
    assert_eq!(peel_engine::rt::guard(|| 7), 7);
}
