//! Runs the live input path under `assert_no_alloc`.
//!
//! Needs `cargo test -p peel-engine --features rt-check`. In a debug build any
//! allocation or deallocation inside `process` aborts the test process.
#![cfg(all(feature = "rt-check", debug_assertions))]

mod common;

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use common::*;
use peel_dsp::EventKind;
use peel_engine::{Engine, EngineCommand, EngineConfig, LiveTiming, Retired, Telemetry};
use peel_time::TempoMap;

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

#[test]
fn live_input_never_allocates_on_the_audio_thread() {
    let config = EngineConfig {
        max_block: 256,
        device_slots: 8,
        events_per_block: 64,
        ..EngineConfig::default()
    };
    let telemetry = Arc::new(Telemetry::new());
    let (retire_tx, mut retire_rx) = rtrb::RingBuffer::<Retired>::new(64);
    let mut engine = Engine::new(SR, &config, telemetry.clone(), retire_tx, None);
    engine.set_live_timing(LiveTiming::Spread);
    let live = engine.take_live_input().expect("live input");
    let mut recorded = engine.take_recorded().expect("record ring");

    // Everything that allocates runs outside the guard, like the control and
    // MIDI threads would.
    for slot in 0..3u32 {
        engine.handle_command(EngineCommand::InstallDevice {
            slot,
            device: Box::new(Gate::new()),
        });
    }
    engine.handle_command(EngineCommand::SetPlan(Box::new(plan(
        TempoMap::constant(140.0),
        &[note(0.0, 100.0, 1)],
        3,
        None,
        200.0,
    ))));
    engine.handle_command(EngineCommand::Play);

    let mut left = vec![0.0f32; 256];
    let mut right = vec![0.0f32; 256];
    let mut peak = 0.0f32;
    let mut drained = Vec::with_capacity(8192);
    for block in 0..800u32 {
        match block % 40 {
            1 => {
                live.sustain(true);
            }
            2 => {
                for key in 48..60u8 {
                    live.note((key % 3) as u32, key, 0.6, true);
                }
            }
            5 => {
                for key in 48..60u8 {
                    live.note((key % 3) as u32, key, 0.0, false);
                }
            }
            10 => {
                live.sustain(false);
            }
            20 => {
                live.note(0, 72, 1.0, true);
                live.note(1, 72, 1.0, true); // same key, another instrument
                live.all_notes_off(0);
            }
            25 => {
                live.note(1, 72, 0.0, false);
                live.target().set(Some(2));
                live.note_to_target(70, 0.5, true);
                live.target().set(Some(1));
                live.note_to_target(70, 0.0, false);
            }
            30 if block == 390 => engine.handle_command(EngineCommand::Panic),
            _ => {}
        }
        live.target().set(Some(block % 3));
        live.expression(EventKind::PitchBend {
            semitones: (block % 24) as f32 - 12.0,
        });
        live.expression(EventKind::ChannelPressure {
            value: (block % 10) as f32 / 10.0,
        });
        live.expression(EventKind::PolyPressure {
            key: 60,
            value: 0.5,
        });
        if block % 17 == 0 {
            live.expression(EventKind::ResetExpression);
        }
        // The engine may be asked for callbacks of any size.
        let n = [256usize, 64, 100, 256][block as usize % 4];
        assert_no_alloc(|| engine.process(&mut left[..n], &mut right[..n]));
        peak = left
            .iter()
            .chain(right.iter())
            .fold(peak, |p, x| p.max(x.abs()));
        while let Ok(item) = retire_rx.pop() {
            drop(item);
        }
        recorded.drain_into(&mut drained);
        drained.clear();
    }
    assert!(peak > 0.0, "the live notes produced sound");
    assert!(live.stats().note_ons() > 100);
}
