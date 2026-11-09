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

#[test]
fn palm_exports_write_only_audio_and_preserve_existing_credit_metadata() {
    use peel_document::{Clip, ClipContent, Id, Note, Time};
    use std::sync::Arc;

    let dir = Dir::new();
    let mut project = template::empty();
    let ch = template::add_channel(
        &mut project,
        "Chug",
        0,
        template::device("peel.guitar.palm_muted", Some("Metal Chugs")),
        40,
    );
    let pattern = Arc::make_mut(&mut project.patterns[0]);
    pattern.length = Time::beats(1);
    pattern
        .notes
        .push(Note::new(ch, Time::ZERO, Time::new(1, 4), 40, 0.8));
    let id = pattern.id;
    let lane = project.arrangement().tracks[0].id;
    project.arrangement_mut().clips.push(Clip {
        id: Id::new(),
        track: lane,
        start: Time::ZERO,
        length: Time::beats(1),
        offset: Time::ZERO,
        muted: false,
        content: ClipContent::Pattern { pattern: id },
    });
    for format in [
        AudioFormat::Wav,
        AudioFormat::Mp3,
        AudioFormat::Flac,
        AudioFormat::Aiff,
    ] {
        let output = dir.file(&format!("chug.{}", format.extension()));
        render_audio(
            &project,
            &HashMap::new(),
            &settings(&output, format),
            &AtomicU32::new(0),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            decode_file(&output).unwrap().channels[0]
                .iter()
                .any(|s| s.abs() > 0.01)
        );
        assert!(
            !dir.file(&format!("chug.{}.credits.txt", format.extension()))
                .exists()
        );
    }
    let output = dir.file("chug.wav");
    let credit = dir.file("chug.wav.credits.txt");
    // An existing sidecar must neither be replaced nor prevent audio export.
    std::fs::write(&credit, "My existing credits").unwrap();
    render_audio(
        &project,
        &HashMap::new(),
        &settings(&output, AudioFormat::Wav),
        &AtomicU32::new(0),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&credit).unwrap(),
        "My existing credits"
    );
    std::fs::remove_file(&credit).unwrap();

    let stem_dir = dir.file("stems");
    std::fs::create_dir_all(&stem_dir).unwrap();
    let mut stems = Session::new_silent(project.clone());
    let job = stems.export_stems(settings(&stem_dir.join("Song.wav"), AudioFormat::Wav));
    let started = Instant::now();
    loop {
        if let Some(result) = job.take_result() {
            assert_eq!(result.unwrap(), stem_dir);
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(5));
    }
    let stem_files: Vec<_> = std::fs::read_dir(&stem_dir)
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect();
    assert_eq!(stem_files.len(), 1);
    assert_eq!(stem_files[0].extension().unwrap(), "wav");

    // Preserve attribution metadata when importing an older export whose
    // sidecar was supplied later, including on content-hash dedupe.
    let notice = include_str!("../../../assets/strings/palm-muted/NOTICE.txt");
    let mut deduped = Session::new_silent(template::empty());
    let lane = deduped.project().arrangement().tracks[0].id;
    deduped.add_audio_clip(lane, Time::ZERO, &output).unwrap();
    assert!(deduped.project().extensions.is_empty());
    let duplicate = dir.file("same-bytes.wav");
    std::fs::copy(&output, &duplicate).unwrap();
    let revised = notice
        .replace(
            "Guitar-TECHS palm-muted guitar recordings",
            "Guitar-TECHS sample credits, revised",
        )
        .replace("\n", "\r\n");
    std::fs::write(dir.file("same-bytes.wav.credits.txt"), revised).unwrap();
    deduped
        .add_audio_clip(lane, Time::beats(1), &duplicate)
        .unwrap();
    assert_eq!(deduped.project().assets.len(), 1);
    assert!(!deduped.project().extensions.is_empty());
    deduped.doc.undo();
    assert!(deduped.project().extensions.is_empty());
    std::fs::write(&credit, notice).unwrap();

    let mut session = Session::new_silent(template::empty());
    let lane = session.project().arrangement().tracks[0].id;
    session.add_audio_clip(lane, Time::ZERO, &output).unwrap();
    let extensions = session.project().extensions.clone();
    assert!(!extensions.is_empty());
    session.doc.undo();
    assert!(
        session.project().extensions.is_empty(),
        "credit import is one undo step"
    );
    session.doc.redo();
    let tpl = session
        .save_template(&dir.file("Templates"), "Chugs")
        .unwrap();
    let loaded = peel_project::load_template(&tpl).unwrap().project;
    assert_eq!(loaded.extensions, extensions);
    assert_eq!(loaded.assets[0].hash, session.project().assets[0].hash);
    assert_ne!(loaded.assets[0].path, session.project().assets[0].path);

    let legacy = dir.file("legacy.wav");
    project.channels[0].device = template::device("peel.guitar.recorded", Some("Palm Mute"));
    render_audio(
        &project,
        &HashMap::new(),
        &settings(&legacy, AudioFormat::Wav),
        &AtomicU32::new(0),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!dir.file("legacy.wav.credits.txt").exists());
}
