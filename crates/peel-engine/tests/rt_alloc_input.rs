//! Runs the audio input path under `assert_no_alloc`.
//!
//! Needs `cargo test -p peel-engine --features rt-check`. In a debug build any
//! allocation or deallocation inside the input callback (`InputInjector::push`
//! runs the very code a device callback runs) or inside the output callback
//! that mixes the monitored input aborts the test process.
#![cfg(all(feature = "rt-check", debug_assertions))]

mod common;

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use common::SR;
use peel_engine::{
    CallbackTiming, CaptureState, Engine, EngineCommand, EngineConfig, InputChannels, Retired,
    Telemetry,
};

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

#[test]
fn input_capture_and_monitoring_never_allocate_on_the_audio_threads() {
    let config = EngineConfig {
        max_block: 256,
        device_slots: 8,
        events_per_block: 64,
        ..EngineConfig::default()
    };
    let telemetry = Arc::new(Telemetry::new());
    let (retire_tx, mut retire_rx) = rtrb::RingBuffer::<Retired>::new(64);
    let mut engine = Engine::new(SR, &config, telemetry.clone(), retire_tx, None);
    let mut input = engine.open_injected_input(&telemetry, SR as u32, 4);
    let shared = engine.input_shared().clone();

    // Everything that allocates runs outside the guard, like the control
    // thread would.
    engine.handle_command(EngineCommand::Play);
    shared.set_monitoring(true, 0.5);
    assert!(shared.arm_capture());

    let mut left = vec![0.0f32; 256];
    let mut right = vec![0.0f32; 256];
    let mut f32_frames = vec![0.0f32; 4 * 512];
    let mut i16_frames = vec![0i16; 4 * 300];
    for (i, x) in f32_frames.iter_mut().enumerate() {
        *x = ((i / 4) as f32 * 0.02).sin() * 0.5 + (i % 4) as f32 * 0.01;
    }
    for (i, x) in i16_frames.iter_mut().enumerate() {
        *x = ((i * 37) % 20_000) as i16 - 10_000;
    }
    let mut drained = vec![0.0f32; 16_384];
    let mut total = 0usize;
    let mut monitored_peak = 0.0f32;
    for block in 0..800u32 {
        match block {
            100 => shared.set_channels(InputChannels::Stereo(2, 3)),
            200 => shared.set_channels(InputChannels::Mono(1)),
            300 => shared.set_monitoring(false, 1.0),
            400 => shared.set_monitoring(true, 2.0),
            _ => {}
        }
        let timing = CallbackTiming {
            callback_ns: 1_000_000 * u64::from(block),
            capture_ns: 1_000_000 * u64::from(block) - 400_000.min(1_000_000 * u64::from(block)),
        };
        input.set_timing(timing);
        // The input and the output callbacks run at different sizes.
        if block % 3 == 0 {
            assert_no_alloc(|| input.push(&i16_frames));
        } else {
            assert_no_alloc(|| input.push(&f32_frames));
        }
        let n = [256usize, 64, 100, 256][block as usize % 4];
        assert_no_alloc(|| engine.process(&mut left[..n], &mut right[..n]));
        monitored_peak = left[..n]
            .iter()
            .chain(right[..n].iter())
            .fold(monitored_peak, |p, x| p.max(x.abs()));
        while let Ok(item) = retire_rx.pop() {
            drop(item);
        }
        // The writer thread's side of the ring.
        loop {
            let got = shared.drain_capture(&mut drained);
            if got == 0 {
                break;
            }
            total += got;
        }
        let _ = shared.take_peak();
    }
    assert_eq!(shared.capture_state(), CaptureState::Capturing);
    assert_eq!(shared.dropped_frames(), 0);
    assert!(total > 800 * 300 * 2, "frames were captured: {total}");
    assert!(monitored_peak > 0.0, "the monitored input was audible");
    assert!(shared.monitor_stats().underruns < 20);
}
