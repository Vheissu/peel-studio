//! Actual selected-insert boundaries and signal-based spectrum checks.
mod common;
use common::*;
use peel_engine::{
    EngineCommand,
    analysis::{AnalysisFrame, FFT_SIZE, SpectrumAnalyzer},
};
use peel_time::TempoMap;

#[test]
fn selected_insert_captures_its_own_pre_post_not_the_master() {
    let mut p = plan(
        TempoMap::constant(120.0),
        &[note(0.0, 10.0, 1)],
        1,
        None,
        20.0,
    );
    p.tracks[0].inserts = vec![10, 11];
    let mut engine = rig(p, None);
    for slot in [10, 11] {
        engine.send(EngineCommand::InstallDevice {
            slot,
            device: Box::new(Half),
        });
    }
    engine.telemetry().analysis.set_slot(Some(10));
    engine.send(EngineCommand::Play);
    let (master, _) = render_blocks(&mut engine, 4096, 256);
    let a = engine.telemetry().analysis.snapshot().unwrap();
    assert_eq!(a.valid_frames, FFT_SIZE);
    assert!((a.output_left[FFT_SIZE - 1] * 2.0 - a.input_left[FFT_SIZE - 1]).abs() < 1e-6);
    assert!((a.output_left[FFT_SIZE - 1] * 0.5 - master[master.len() - 1]).abs() < 1e-6);
    let key = a.key;
    engine.telemetry().analysis.set_slot(Some(11));
    assert!(
        engine.telemetry().analysis.snapshot().is_none(),
        "old slot cannot leak after selection"
    );
    render_blocks(&mut engine, 256, 256);
    let b = engine.telemetry().analysis.snapshot().unwrap();
    assert_ne!(b.key, key);
    assert_eq!(b.valid_frames, 256);
    assert!((b.input_left[FFT_SIZE - 1] - a.output_left[FFT_SIZE - 1]).abs() < 1e-6);
    engine.send(EngineCommand::Seek { beat: 1.0 });
    assert!(engine.telemetry().analysis.snapshot().is_none());
    engine.telemetry().analysis.set_slot(None);
    render_blocks(&mut engine, 256, 256);
    assert!(engine.telemetry().analysis.snapshot().is_none());
}

#[test]
fn bypass_capture_shows_dry_output_and_silence_clears() {
    let mut p = plan(
        TempoMap::constant(120.0),
        &[note(0.0, 0.1, 1)],
        1,
        None,
        20.0,
    );
    p.tracks[0].inserts = vec![10];
    let mut engine = rig(p, None);
    engine.send(EngineCommand::InstallDevice {
        slot: 10,
        device: Box::new(Half),
    });
    engine.telemetry().analysis.set_slot(Some(10));
    engine.send(EngineCommand::SetBypass {
        slot: 10,
        bypass: true,
    });
    engine.send(EngineCommand::Play);
    render_blocks(&mut engine, 1024, 256);
    let a = engine.telemetry().analysis.snapshot().unwrap();
    assert!((a.input_left[FFT_SIZE - 1] - a.output_left[FFT_SIZE - 1]).abs() < 1e-6);
    render_blocks(&mut engine, 8192, 256);
    let a = engine.telemetry().analysis.snapshot().unwrap();
    assert!(a.input_left.iter().chain(&a.output_left).all(|&x| x == 0.0));
}

fn tone_frame(sequence: u64, anti_phase: bool) -> AnalysisFrame {
    let samples = std::array::from_fn(|i| {
        (2.0 * std::f32::consts::PI * 64.0 * i as f32 / FFT_SIZE as f32).sin() * 0.5
    });
    AnalysisFrame {
        key: 1,
        sequence,
        sample_rate: 48_000.0,
        valid_frames: FFT_SIZE,
        total_frames: sequence * FFT_SIZE as u64,
        input_left: samples,
        input_right: samples.map(|x| if anti_phase { -x } else { x }),
        output_left: samples.map(|x| x * 0.5),
        output_right: samples.map(|x| if anti_phase { -x * 0.5 } else { x * 0.5 }),
    }
}

#[test]
fn calibrated_fft_and_persistent_candidates_preserve_antiphase_stereo() {
    for anti_phase in [false, true] {
        let mut analyzer = SpectrumAnalyzer::new();
        for sequence in 1..=60 {
            assert!(analyzer.update(&tone_frame(sequence, anti_phase)));
        }
        let s = analyzer.snapshot();
        assert!(
            (s.input_db[64] - -6.0206).abs() < 0.05,
            "{}",
            s.input_db[64]
        );
        assert!((s.output_db[64] - -12.0412).abs() < 0.05);
        assert!((s.input_rms_db - -9.0309).abs() < 0.05);
        assert_eq!(s.frequency(64), 1500.0);
        assert!((s.resonances[0].frequency_hz - 1500.0).abs() < 25.0);
        assert!(s.resonances[0].cut_db < 0.0);
        assert!(
            !analyzer.update(&tone_frame(60, anti_phase)),
            "duplicate UI reads do not accumulate analysis"
        );
        let mut silence = tone_frame(61, anti_phase);
        silence.input_left.fill(0.0);
        silence.input_right.fill(0.0);
        silence.output_left.fill(0.0);
        silence.output_right.fill(0.0);
        analyzer.update(&silence);
        assert!(analyzer.snapshot().resonances.is_empty());
        silence.key = 2;
        silence.sequence = 62;
        silence.total_frames = FFT_SIZE as u64;
        analyzer.update(&silence);
        assert_eq!(analyzer.snapshot().input_peak_db, -120.0);
        assert!(analyzer.snapshot().analyzed_seconds == 0.0);
    }
}

#[test]
fn broadband_noise_does_not_suggest_notches() {
    let mut analyzer = SpectrumAnalyzer::new();
    let mut seed = 12345u32;
    for sequence in 1..=80 {
        let mut frame = tone_frame(sequence, false);
        for i in 0..FFT_SIZE {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let x = (seed as f32 / u32::MAX as f32 - 0.5) * 0.2;
            frame.input_left[i] = x;
            frame.input_right[i] = x;
            frame.output_left[i] = x;
            frame.output_right[i] = x;
        }
        analyzer.update(&frame);
    }
    assert!(analyzer.snapshot().resonances.is_empty());
}
