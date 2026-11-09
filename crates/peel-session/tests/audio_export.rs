use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::time::{Duration, Instant};

use peel_media::{AudioFormat, decode_file};
use peel_session::{AudioExportSettings, ExportSettings, Session, render_audio, template};

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("peel-export-{}", peel_document::Id::new()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn settings(path: &Path, format: AudioFormat) -> AudioExportSettings {
    AudioExportSettings {
        render: ExportSettings {
            path: path.to_owned(),
            sample_rate: 44_100,
            bits: 24,
            tail_seconds: 0.0,
        },
        format,
        dither: false,
        ..AudioExportSettings::default()
    }
}

#[test]
fn all_formats_render_real_audio_with_rate_channels_and_normalization() {
    let dir = Dir::new();
    let project = template::beat_and_melody();
    let expected_frames = (peel_time::TempoMap::from_segments(&project.tempo)
        .beat_to_seconds(project.arrangement().end().as_f64())
        * 44_100.0)
        .ceil() as usize;
    for format in [
        AudioFormat::Wav,
        AudioFormat::Mp3,
        AudioFormat::Flac,
        AudioFormat::Aiff,
    ] {
        let path = dir.file(&format!("mix.{}", format.extension()));
        let mut options = settings(&path, format);
        options.mono = true;
        options.normalize = true;
        let progress = AtomicU32::new(0);
        render_audio(
            &project,
            &HashMap::new(),
            &options,
            &progress,
            &AtomicBool::new(false),
        )
        .unwrap();
        let decoded = decode_file(&path).unwrap();
        assert_eq!(decoded.channels.len(), 1, "{format:?}");
        assert_eq!(decoded.sample_rate, 44_100.0);
        let frames = decoded.channels[0].len();
        assert!(
            frames.abs_diff(expected_frames) <= if format == AudioFormat::Mp3 { 2304 } else { 0 },
            "{format:?}: {frames} != {expected_frames}"
        );
        let peak = decoded.channels[0]
            .iter()
            .fold(0.0f32, |peak, v| peak.max(v.abs()));
        if format == AudioFormat::Mp3 {
            assert!(peak > 0.5 && peak < 1.3, "{peak}");
        } else {
            assert!(
                (peak - peel_dsp::util::db_to_gain(-1.0)).abs() < 0.00001,
                "{format:?}: {peak}"
            );
        }
        assert_eq!(
            f32::from_bits(progress.load(std::sync::atomic::Ordering::Relaxed)),
            1.0
        );
    }
}

#[test]
fn invalid_settings_and_cancellation_preserve_existing_output() {
    let dir = Dir::new();
    let path = dir.file("mix.wav");
    std::fs::write(&path, b"original").unwrap();
    let project = template::beat_and_melody();
    let mut options = settings(&path, AudioFormat::Wav);
    for tail in [f32::NAN, f32::INFINITY, -1.0, 61.0] {
        options.render.tail_seconds = tail;
        assert!(
            render_audio(
                &project,
                &HashMap::new(),
                &options,
                &AtomicU32::new(0),
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    options.render.tail_seconds = 0.0;
    options.format = AudioFormat::Mp3;
    assert!(options.validate().is_err(), "extension must match format");
    options.format = AudioFormat::Wav;
    let error = render_audio(
        &project,
        &HashMap::new(),
        &options,
        &AtomicU32::new(0),
        &AtomicBool::new(true),
    )
    .unwrap_err();
    assert!(error.to_lowercase().contains("cancel"));
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
}

#[test]
fn stems_use_selected_format_and_disambiguate_track_names() {
    let dir = Dir::new();
    let mut project = template::beat_and_melody();
    for track in project.mixer.tracks.iter_mut().skip(1) {
        track.name = "Same / name".into();
    }
    let count = peel_session::stem_tracks(&project).len();
    let mut session = Session::new_silent(project);
    let job = session.export_stems(settings(&dir.file("Song.flac"), AudioFormat::Flac));
    let start = Instant::now();
    loop {
        if let Some(result) = job.take_result() {
            assert_eq!(result.unwrap(), dir.0);
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(5));
    }
    let paths: Vec<_> = std::fs::read_dir(&dir.0)
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect();
    assert_eq!(paths.len(), count);
    for path in paths {
        assert_eq!(path.extension().unwrap(), "flac");
        assert!(decode_file(path).is_ok());
    }
}
