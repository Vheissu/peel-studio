mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::*;
use peel_engine::{EngineCommand, EngineConfig, EngineHandle};
use peel_time::TempoMap;

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn send(h: &mut EngineHandle, cmd: EngineCommand) {
    assert!(h.send(cmd).is_ok(), "command queue full");
}

#[test]
fn the_null_backend_runs_the_transport_and_returns_garbage() {
    let mut h = EngineHandle::start_null(config()).unwrap();
    assert!(h.is_null_backend());
    assert!(h.is_healthy());
    assert_eq!(h.device_name(), "No audio device");
    assert_eq!(h.sample_rate(), 48_000.0);
    assert_eq!(h.max_block(), 256);
    assert_eq!(h.buffer_frames(), 256);

    send(
        &mut h,
        EngineCommand::InstallDevice {
            slot: 0,
            device: Box::new(Gate::new()),
        },
    );
    let p = plan(
        TempoMap::constant(240.0),
        &[note(0.0, 100.0, 1)],
        1,
        None,
        200.0,
    );
    send(&mut h, EngineCommand::SetPlan(Box::new(p)));
    send(&mut h, EngineCommand::Play);

    let t = h.telemetry().clone();
    wait_for("the playhead to move", || t.beat() > 1.0);
    assert!(t.playing.load(Ordering::Relaxed));
    wait_for("the note to sound", || t.take_master().0 >= 1.0);

    // Replacing the plan sends the old one back.
    let mut p2 = plan(TempoMap::constant(240.0), &[], 1, None, 200.0);
    p2.revision = 5;
    send(&mut h, EngineCommand::SetPlan(Box::new(p2)));
    let mut freed = 0;
    wait_for("the old plan to be retired", || {
        freed += h.collect_garbage();
        freed >= 2 // the initial empty plan and the first real one
    });
    assert_eq!(t.plan_revision.load(Ordering::Relaxed), 5);

    send(&mut h, EngineCommand::Stop { return_to: 0.0 });
    wait_for("the transport to stop", || {
        !t.playing.load(Ordering::Relaxed)
    });
    drop(h);
}

#[test]
fn the_command_queue_keeps_room_for_stop_and_panic() {
    // A long period keeps the audio thread from draining the queue while the
    // test fills it.
    let config = EngineConfig {
        command_capacity: 64,
        buffer_frames: Some(48_000),
        ..config()
    };
    let mut h = EngineHandle::start_null(config).unwrap();
    // Let the thread's immediate first block pass.
    std::thread::sleep(Duration::from_millis(50));
    let mut rejected = None;
    for i in 0..10_000u32 {
        let cmd = EngineCommand::SetParam {
            slot: 0,
            id: 0,
            value: i as f32,
        };
        if let Err(back) = h.send(cmd) {
            rejected = Some(back);
            break;
        }
    }
    assert!(rejected.is_some(), "queue never filled");
    // Ordinary commands bounce, the ones that silence the engine do not.
    assert!(
        h.send(EngineCommand::SetParam {
            slot: 0,
            id: 0,
            value: 0.0
        })
        .is_err()
    );
    assert!(h.send(EngineCommand::Pause).is_ok());
    assert!(h.send(EngineCommand::Panic).is_ok());
    assert!(h.send(EngineCommand::Stop { return_to: 0.0 }).is_ok());
}

#[test]
fn dropping_the_handle_frees_queued_devices() {
    let mut h = EngineHandle::start_null(EngineConfig {
        buffer_frames: Some(48_000),
        ..config()
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    for slot in 0..4 {
        send(
            &mut h,
            EngineCommand::InstallDevice {
                slot,
                device: Box::new(Gate::new()),
            },
        );
    }
    let started = Instant::now();
    drop(h);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "dropping must stop the audio thread promptly"
    );
}

/// Opens the real default device. Skipped in CI; run with `--ignored`.
#[test]
#[ignore = "needs an audio output device"]
fn the_default_device_opens_and_runs() {
    let mut h = EngineHandle::start(config()).unwrap();
    println!(
        "device: {} @ {} Hz, buffer {}, null: {}",
        h.device_name(),
        h.sample_rate(),
        h.buffer_frames(),
        h.is_null_backend()
    );
    send(&mut h, EngineCommand::Play);
    let t = h.telemetry().clone();
    wait_for("the playhead to move", || t.beat() > 0.1);
    assert!(h.is_healthy());
}
