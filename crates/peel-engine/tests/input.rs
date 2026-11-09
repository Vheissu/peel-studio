//! Audio input through a real engine: monitoring is mixed after the master,
//! takes start with the transport, and the handle opens and closes inputs.
//! No device is used: `open_injected_input` feeds frames through the same
//! processor a device callback runs.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::SR;
use peel_engine::{
    CaptureState, Engine, EngineCommand, EngineConfig, EngineHandle, InputChannels, Retired,
    Telemetry,
};

struct Rig {
    engine: Engine,
    telemetry: Arc<Telemetry>,
    retired: rtrb::Consumer<Retired>,
}

fn rig() -> Rig {
    let config = EngineConfig {
        max_block: 256,
        device_slots: 4,
        events_per_block: 16,
        ..EngineConfig::default()
    };
    let telemetry = Arc::new(Telemetry::new());
    let (retire_tx, retired) = rtrb::RingBuffer::<Retired>::new(64);
    let engine = Engine::new(SR, &config, telemetry.clone(), retire_tx, None);
    Rig {
        engine,
        telemetry,
        retired,
    }
}

fn render(engine: &mut Engine, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    engine.process(&mut l, &mut r);
    (l, r)
}

#[test]
fn monitoring_is_silent_until_switched_on_and_bypasses_the_master() {
    let mut rig = rig();
    let mut input = rig.engine.open_injected_input(&rig.telemetry, SR as u32, 1);
    let shared = rig.engine.input_shared().clone();

    // Off by default: the input is not heard.
    for _ in 0..6 {
        input.push(&[0.5f32; 256]);
        let (l, r) = render(&mut rig.engine, 256);
        assert!(l.iter().chain(&r).all(|x| *x == 0.0));
    }

    shared.set_monitoring(true, 0.5);
    let mut last = (vec![], vec![]);
    for _ in 0..12 {
        input.push(&[0.5f32; 256]);
        last = render(&mut rig.engine, 256);
    }
    assert!(
        last.0.iter().all(|x| (*x - 0.25).abs() < 1e-6),
        "{:?}",
        &last.0[..4]
    );
    assert_eq!(last.0, last.1, "a mono input is centred");
    assert_eq!(
        rig.telemetry.take_master(),
        (0.0, 0.0),
        "monitoring bypasses the master and its meter"
    );
    assert_eq!(shared.monitor_stats().underruns, 0);

    shared.set_monitoring(false, 0.5);
    input.push(&[0.5f32; 256]);
    let (l, _) = render(&mut rig.engine, 256);
    assert!(l.iter().all(|x| *x == 0.0), "off again");
    while let Ok(r) = rig.retired.pop() {
        drop(r);
    }
}

#[test]
fn closing_the_monitor_connection_stops_the_sound() {
    let mut rig = rig();
    let mut input = rig.engine.open_injected_input(&rig.telemetry, SR as u32, 1);
    let shared = rig.engine.input_shared().clone();
    shared.set_monitoring(true, 1.0);
    for _ in 0..8 {
        input.push(&[0.4f32; 256]);
        render(&mut rig.engine, 256);
    }
    rig.engine
        .handle_command(EngineCommand::SetInputMonitor(None));
    input.push(&[0.4f32; 256]);
    let (l, _) = render(&mut rig.engine, 256);
    assert!(l.iter().all(|x| *x == 0.0));
    assert!(
        matches!(rig.retired.pop(), Ok(Retired::Monitor(_))),
        "the old FIFO is freed by the control thread"
    );
}

#[test]
fn another_input_rate_is_monitored_at_the_right_pitch_and_captured_unchanged() {
    let mut rig = rig();
    let mut input = rig.engine.open_injected_input(&rig.telemetry, 44_100, 4);
    let shared = rig.engine.input_shared().clone();
    assert!(shared.monitor_supported());
    shared.set_channels(InputChannels::Stereo(2, 3));
    shared.set_monitoring(true, 1.0);
    rig.engine.handle_command(EngineCommand::Play);
    render(&mut rig.engine, 256);
    assert!(shared.arm_capture());
    let mut sample = 0usize;
    let mut fractional = 0.0;
    let mut raw = [0.0_f32; 2048];
    let mut raw_frames = 0;
    let mut output = Vec::new();
    let (mut left, mut right) = ([0.0_f32; 480], [0.0_f32; 480]);
    // Twenty seconds, with the input clock 300 ppm faster than the output.
    for callback in 0..2000 {
        fractional += 441.0 * 1.0003;
        let frames = fractional as usize;
        fractional -= frames as f64;
        let mut source = Vec::with_capacity(frames * 4);
        for i in 0..frames {
            let sine = (std::f32::consts::TAU * 1000.0 * (sample + i) as f32 / 44_100.0).sin();
            source.extend_from_slice(&[0.9, -0.9, 0.4 * sine, -0.2 * sine]);
        }
        input.push(&source);
        let got = shared.drain_capture(&mut raw);
        assert_eq!(got, frames * 2, "capture keeps input-clock frames");
        for (captured, original) in raw[..got]
            .as_chunks::<2>()
            .0
            .iter()
            .zip(source.as_chunks::<4>().0)
        {
            assert_eq!(captured, &original[2..4], "capture is never resampled");
        }
        raw_frames += got / 2;
        sample += frames;
        left.fill(0.0);
        right.fill(0.0);
        rig.engine.process(&mut left, &mut right);
        if callback > 50 {
            assert!(
                left.iter()
                    .zip(right)
                    .all(|(l, r)| (*l + r * 2.0).abs() < 1e-5),
                "stereo mapping and polarity are retained"
            );
            output.extend_from_slice(&left);
        }
    }
    assert_eq!(shared.input_sample_rate(), 44_100);
    assert_eq!(shared.captured_frames() as usize, raw_frames);
    assert_eq!(shared.dropped_frames(), 0);
    assert_eq!(shared.monitor_stats().underruns, 0);
    assert_eq!(shared.monitor_stats().dropped_frames, 0);
    let crossings = output
        .windows(2)
        .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
        .count();
    let hz = crossings as f64 * 48_000.0 / output.len() as f64;
    assert!(
        (hz - 1000.3).abs() < 2.0,
        "resampling preserves tone pitch: {hz} Hz"
    );
    assert!(output.iter().any(|v| *v > 0.35));
    assert!(
        shared.monitor_stats().fifo_frames < 2400.0,
        "latency remains bounded"
    );
}

#[test]
fn switching_monitoring_off_and_on_between_callbacks_drops_old_audio() {
    let mut rig = rig();
    let mut input = rig.engine.open_injected_input(&rig.telemetry, 44_100, 1);
    let shared = rig.engine.input_shared().clone();
    shared.set_monitoring(true, 1.0);
    for _ in 0..4 {
        input.push(&[0.9_f32; 441]);
    }
    // Output never observes the off state. Generation tracking must still
    // discard these old input frames before the next monitored callback.
    shared.set_monitoring(false, 1.0);
    shared.set_monitoring(true, 1.0);
    for _ in 0..8 {
        input.push(&[0.0_f32; 441]);
        let (left, right) = render(&mut rig.engine, 480);
        assert!(left.iter().chain(&right).all(|v| *v == 0.0));
    }
}

#[test]
fn a_take_begins_with_the_transport_and_remembers_where_it_was() {
    let mut rig = rig();
    let mut input = rig.engine.open_injected_input(&rig.telemetry, SR as u32, 2);
    let shared = rig.engine.input_shared().clone();
    shared.set_channels(InputChannels::Stereo(0, 1));
    assert!(shared.arm_capture());

    // Armed, transport stopped: frames go nowhere.
    input.push(&[0.1f32; 2 * 256]);
    render(&mut rig.engine, 256);
    assert_eq!(shared.capture_state(), CaptureState::Armed);
    assert_eq!(shared.capture_pending(), 0);

    rig.engine.handle_command(EngineCommand::Play);
    // The transport advances; the take starts at the first callback that
    // sees it playing, with the position telemetry showed then.
    for _ in 0..40 {
        render(&mut rig.engine, 256);
    }
    let beat_then = rig.telemetry.beat();
    assert!(rig.telemetry.playing.load(Ordering::Relaxed) && beat_then > 0.0);
    input.push(&[0.2f32; 2 * 256]);
    assert_eq!(shared.capture_state(), CaptureState::Capturing);
    let start = shared.capture_start().expect("snapshot");
    assert_eq!(start.playhead_beat, beat_then);
    // The null clock reports no timing: a block each way is assumed.
    assert!((start.output_block_s - 256.0 / f64::from(SR)).abs() < 1e-9);
    assert_eq!(shared.captured_frames(), 256);

    // Later frames are appended; the snapshot does not move.
    render(&mut rig.engine, 256);
    input.push(&[0.3f32; 2 * 256]);
    assert_eq!(shared.captured_frames(), 512);
    assert_eq!(shared.capture_start().unwrap().playhead_beat, beat_then);
    let mut out = vec![0.0f32; 4096];
    let n = shared.drain_capture(&mut out);
    assert_eq!(n, 2 * 512);
    assert!(out[..2 * 256].iter().all(|x| *x == 0.2));
    assert!(out[2 * 256..n].iter().all(|x| *x == 0.3));
}

#[test]
fn the_handle_opens_an_injected_input_closes_it_and_reports_it() {
    let mut h = EngineHandle::start_null(EngineConfig::default()).unwrap();
    assert!(!h.input().is_open() && h.input_info().is_none());

    let mut input = h.open_injected_input(48_000, 2);
    assert!(h.input().is_open());
    let info = h.input_info().expect("info");
    assert_eq!((info.sample_rate, info.channels), (48_000, 2));
    assert!(h.input().monitor_supported());

    // Frames show on the meter.
    input.push(&[0.6f32; 2 * 128]);
    let (l, r) = h.input().take_peak();
    assert!((l - 0.6).abs() < 1e-6 && (r - 0.6).abs() < 1e-6);

    // Reopening replaces it; stopping the audio closes it.
    let _again = h.open_injected_input(44_100, 1);
    assert_eq!(h.input().input_sample_rate(), 44_100);
    assert!(h.input().monitor_supported());
    h.stop_audio();
    assert!(!h.input().is_open());
    assert!(h.input_info().is_none());
}

#[test]
fn the_null_engine_reports_the_output_block_for_latency() {
    let h = EngineHandle::start_null(EngineConfig {
        buffer_frames: Some(128),
        ..EngineConfig::default()
    })
    .unwrap();
    let t0 = Instant::now();
    loop {
        let lat = h.input().latency();
        if lat.output_s > 0.0 {
            assert!((lat.output_s - 128.0 / 48_000.0).abs() < 1e-9, "{lat:?}");
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "no timing arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
}
