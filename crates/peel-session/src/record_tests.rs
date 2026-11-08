//! Session-level tests for audio recording. Frames are injected through the
//! same input path a microphone uses (`InputInjector`); no device is opened.

use std::sync::atomic::{AtomicUsize, Ordering};

use peel_document::{ClipContent, Project};
use peel_engine::take_start_beat;
use peel_media::decode_file;

use super::*;
use crate::template;

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "peel-session-record-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `data[i] = i * 1e-5`: every frame is distinct, and float WAV stores it
/// exactly, so a recording can be compared sample for sample.
fn ramp(frames: usize) -> Vec<f32> {
    (0..frames).map(|i| i as f32 * 1e-5).collect()
}

/// A session in Song mode that records mono 48 kHz into `dir`.
fn rig(dir: &Scratch) -> (Session, InputInjector, Id) {
    let mut s = Session::new_silent(Project::empty("rec"));
    s.set_recordings_dir(Some(dir.path("takes")));
    s.set_mode(PlayMode::Song);
    let input = s.open_injected_input(48_000, 1);
    let lane = s.project().arrangement().tracks[0].id;
    (s, input, lane)
}

fn wait(s: &mut Session, what: &str, mut done: impl FnMut(&mut Session) -> bool) {
    let t0 = Instant::now();
    loop {
        s.tick();
        if done(s) {
            return;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(3));
    }
}

fn transport_running(s: &mut Session) -> bool {
    s.telemetry().playing.load(Ordering::Relaxed)
}

/// Pushes `signal` in 256-frame callbacks at roughly real-time pace.
fn feed(input: &mut InputInjector, signal: &[f32]) {
    for chunk in signal.chunks(256) {
        input.push(chunk);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn recorded_clips(s: &Session) -> Vec<peel_document::Clip> {
    s.project()
        .arrangement()
        .clips
        .iter()
        .filter(|c| matches!(c.content, ClipContent::Audio { .. }))
        .cloned()
        .collect()
}

fn asset_of(clip: &peel_document::Clip) -> Id {
    match clip.content {
        ClipContent::Audio { asset, .. } => asset,
        _ => panic!("not an audio clip"),
    }
}

fn tempo(s: &Session) -> TempoMap {
    TempoMap::from_segments(&s.project().tempo)
}

// ---------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------

#[test]
fn a_take_is_placed_at_its_beat_with_the_length_of_its_audio() {
    let tempo = TempoMap::constant(120.0);
    let (start, length, offset) = placement_for_take(&tempo, 8.25, 1.5).unwrap();
    assert_eq!(start, Time::new(33, 4));
    assert_eq!(length, Time::beats(3));
    assert_eq!(offset, Time::ZERO);
    // At 60 BPM a second is a beat.
    let slow = TempoMap::constant(60.0);
    let (_, length, _) = placement_for_take(&slow, 0.0, 1.5).unwrap();
    assert_eq!(length, Time::new(3, 2));
}

#[test]
fn a_take_that_starts_before_the_song_is_trimmed_through_the_offset() {
    let tempo = TempoMap::constant(120.0);
    // 0.25 s before the top of the song = half a beat of audio to skip.
    let (start, length, offset) = placement_for_take(&tempo, -0.5, 2.0).unwrap();
    assert_eq!(start, Time::ZERO);
    assert_eq!(offset, Time::new(1, 2));
    assert_eq!(length, Time::new(7, 2), "1.75 s of audio left = 3.5 beats");
    // Entirely before the song: nothing to place.
    assert_eq!(placement_for_take(&tempo, -10.0, 2.0), None);
    assert_eq!(placement_for_take(&tempo, 1.0, 0.0), None);
    assert_eq!(placement_for_take(&tempo, f64::NAN, 1.0), None);
}

#[test]
fn the_length_follows_the_tempo_map() {
    use peel_document::TempoSegment;
    // 60 BPM for four beats, then 120: starting at beat 3, one second of
    // audio is one beat at 60 BPM, then 0.5 s at 120 BPM is one more beat.
    let tempo = TempoMap::from_segments(&[
        TempoSegment {
            start: Time::ZERO,
            bpm: 60.0,
            ramp_to: None,
        },
        TempoSegment {
            start: Time::beats(4),
            bpm: 120.0,
            ramp_to: None,
        },
    ]);
    let (start, length, _) = placement_for_take(&tempo, 3.0, 1.5).unwrap();
    assert_eq!(start, Time::beats(3));
    assert_eq!(length, Time::beats(2));
}

// ---------------------------------------------------------------------
// The whole path
// ---------------------------------------------------------------------

#[test]
fn a_take_becomes_a_clip_holding_the_injected_audio_at_the_right_beat() {
    let dir = Scratch::new();
    let (mut s, mut input, lane) = rig(&dir);
    s.arm_lane(lane, true);
    s.seek(8.0);
    s.start_recording().expect("start");
    assert!(s.is_recording());
    wait(&mut s, "the transport", transport_running);

    let signal = ramp(48_000);
    feed(&mut input, &signal);
    let t0 = Instant::now();
    let status = loop {
        let status = s.recording_status().expect("status");
        if status.frames > 0 || t0.elapsed() > Duration::from_secs(5) {
            break status;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(!status.waiting_for_transport);
    assert!(status.frames > 0 && status.error.is_none(), "{status:?}");

    let ids = s.stop_recording();
    assert!(!s.is_recording());
    assert_eq!(ids.len(), 1);
    let clips = recorded_clips(&s);
    assert_eq!(clips.len(), 1);
    let clip = &clips[0];
    assert_eq!((clip.id, clip.track), (ids[0], lane));
    assert_eq!(clip.offset, Time::ZERO);

    // Where: the playhead at the first input callback, moved back by the
    // latency, by exactly the formula in one place.
    let snap = s.engine.input().capture_start().expect("snapshot");
    let expected = take_start_beat(&tempo(&s), &snap, 0.0);
    assert!(
        (clip.start.as_f64() - expected).abs() <= 1.0 / TIME_GRID as f64,
        "{} vs {expected}",
        clip.start.as_f64()
    );
    assert!(
        snap.playhead_beat >= 8.0 && snap.playhead_beat < 12.0,
        "the playhead when the take began: {}",
        snap.playhead_beat
    );
    let lag_beats = snap.lag_seconds() * 2.0;
    assert!(
        lag_beats > 0.0 && (clip.start.as_f64() - (snap.playhead_beat - lag_beats)).abs() < 1e-3,
        "start {} playhead {} lag {lag_beats}",
        clip.start.as_f64(),
        snap.playhead_beat
    );
    // How long: one second at 120 BPM.
    assert!((clip.length.as_f64() - 2.0).abs() <= 1.0 / TIME_GRID as f64);

    // What: the asset is the file the writer produced, and it is exactly
    // what was injected.
    let asset = s.project().asset(asset_of(clip)).expect("asset").clone();
    assert_eq!(
        (asset.sample_rate, asset.channels, asset.frames),
        (48_000, 1, 48_000)
    );
    assert!(!asset.managed);
    assert!(Path::new(&asset.path).starts_with(dir.path("takes")));
    let file = decode_file(&asset.path).expect("decode");
    assert_eq!(file.frames(), 48_000);
    assert_eq!(file.left(), &signal[..]);

    // The engine gets the same audio at the clip's beat, with a waveform.
    wait(&mut s, "the waveform", |s| {
        s.audio_peaks(asset.id).is_some()
    });
    let loaded = s.sample(asset.id).expect("decoded");
    assert_eq!(loaded.left(), &signal[..]);
    let plan = s.plan.as_ref().expect("plan");
    assert_eq!(plan.audio.len(), 1);
    assert_eq!(plan.audio[0].start_beat, clip.start.as_f64());
    assert_eq!(plan.audio[0].source_start_seconds, 0.0);

    // One undoable step; the recording itself survives the undo.
    assert_eq!(s.doc.undo_label(), Some("Record audio"));
    s.doc.undo();
    assert!(recorded_clips(&s).is_empty());
    assert!(s.project().assets.is_empty());
    assert!(Path::new(&asset.path).is_file());
    s.doc.redo();
    assert_eq!(recorded_clips(&s).len(), 1);
}

#[test]
fn every_armed_lane_gets_a_clip_of_one_asset_in_one_undo_step() {
    let dir = Scratch::new();
    let (mut s, mut input, l0) = rig(&dir);
    let l2 = s.project().arrangement().tracks[2].id;
    s.arm_lane(l0, true);
    s.arm_lane(l2, true);
    assert_eq!(s.armed_lanes(), vec![l0, l2]);
    s.start_recording().expect("start");
    wait(&mut s, "the transport", transport_running);
    feed(&mut input, &ramp(9_600));
    let ids = s.stop_recording();

    assert_eq!(ids.len(), 2);
    let clips = recorded_clips(&s);
    assert_eq!(
        clips.iter().map(|c| c.track).collect::<Vec<_>>(),
        vec![l0, l2]
    );
    assert_eq!(asset_of(&clips[0]), asset_of(&clips[1]));
    assert_eq!(
        (clips[0].start, clips[0].length),
        (clips[1].start, clips[1].length)
    );
    assert_eq!(s.project().assets.len(), 1);
    // Each lane plays through a mixer track of its own.
    assert_eq!(s.project().mixer.tracks.len(), 3);
    assert!(s.doc.validate().iter().all(|i| !i.is_error()));

    s.doc.undo();
    assert!(recorded_clips(&s).is_empty());
    assert_eq!(s.project().mixer.tracks.len(), 1);
    assert!(s.stop_recording().is_empty(), "the ids are handed out once");
}

#[test]
fn the_file_is_on_disk_while_recording_even_if_the_ui_stalls() {
    let dir = Scratch::new();
    let (mut s, mut input, lane) = rig(&dir);
    s.arm_lane(lane, true);
    s.start_recording().expect("start");
    wait(&mut s, "the transport", transport_running);

    // No tick, no stop: the UI thread is stuck. The writer thread alone keeps
    // the file growing and valid.
    let signal = ramp(24_000);
    feed(&mut input, &signal);
    let path = s.rec.active.as_ref().expect("active").path.clone();
    // The writer checkpoints every 100 ms; give it a few seconds of slack.
    let t0 = Instant::now();
    let mid_take = loop {
        let mid_take = decode_file(&path).expect("a valid WAV before stop");
        if mid_take.frames() >= 20_000 || t0.elapsed() > Duration::from_secs(5) {
            break mid_take;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        mid_take.frames() >= 20_000,
        "frames on disk before stop: {}",
        mid_take.frames()
    );
    assert_eq!(mid_take.left(), &signal[..mid_take.frames()]);
    assert!(s.recording_status().unwrap().seconds > 0.4);

    assert_eq!(s.stop_recording().len(), 1);
    assert_eq!(decode_file(&path).unwrap().frames(), 24_000);
}

#[test]
fn stopping_the_transport_ends_the_take_and_keeps_its_clips_for_stop_recording() {
    let dir = Scratch::new();
    let (mut s, mut input, lane) = rig(&dir);
    s.arm_lane(lane, true);
    s.start_recording().unwrap();
    wait(&mut s, "the transport", transport_running);
    feed(&mut input, &ramp(4_800));

    s.stop();
    assert!(!s.is_recording());
    assert_eq!(recorded_clips(&s).len(), 1, "stop placed the clip");
    let ids = s.stop_recording();
    assert_eq!(ids.len(), 1);
    assert!(s.stop_recording().is_empty());
}

#[test]
fn restarting_audio_ends_a_take_without_losing_it() {
    let dir = Scratch::new();
    let (mut s, mut input, lane) = rig(&dir);
    s.arm_lane(lane, true);
    s.start_recording().unwrap();
    wait(&mut s, "the transport", transport_running);
    feed(&mut input, &ramp(4_800));

    s.restart_audio(None, Some(128)).expect("restart");
    assert!(!s.is_recording());
    assert_eq!(s.stop_recording().len(), 1);
    assert_eq!(recorded_clips(&s).len(), 1);
    assert!(
        !s.input_is_open(),
        "the injected input went with the engine"
    );
}

#[test]
fn a_take_that_starts_before_the_song_keeps_its_clip_at_zero_and_trims_the_head() {
    let dir = Scratch::new();
    let (mut s, mut input, lane) = rig(&dir);
    s.arm_lane(lane, true);
    s.set_record_offset_ms(500.0); // one beat earlier at 120 BPM
    s.start_recording().unwrap();
    wait(&mut s, "the transport", transport_running);
    feed(&mut input, &ramp(48_000));
    assert_eq!(s.stop_recording().len(), 1);

    let clip = recorded_clips(&s)[0].clone();
    let snap = s.engine.input().capture_start().unwrap();
    let start = take_start_beat(&tempo(&s), &snap, 0.5);
    assert!(start < 0.0, "{start}");
    assert_eq!(clip.start, Time::ZERO);
    assert!(
        (clip.offset.as_f64() + start).abs() <= 1.0 / TIME_GRID as f64,
        "trimmed {} beats for a start at {start}",
        clip.offset.as_f64()
    );
    // What is left plays for the rest of the second.
    assert!((clip.length.as_f64() + clip.offset.as_f64() - 2.0).abs() <= 2.0 / TIME_GRID as f64);
    wait(&mut s, "the plan", |s| {
        s.plan.as_ref().is_some_and(|p| p.audio.len() == 1)
    });
    let region = &s.plan.as_ref().unwrap().audio[0];
    assert!((region.source_start_seconds - (-start * 0.5)).abs() < 1e-3);
}

#[test]
fn a_saved_project_keeps_its_recording_as_managed_media() {
    let dir = Scratch::new();
    let (mut s, mut input, lane) = rig(&dir);
    s.set_recordings_dir(None);
    let project = dir.path("Song.peelproj");
    s.save(Some(project.clone())).expect("save");
    s.arm_lane(lane, true);
    s.start_recording().unwrap();
    wait(&mut s, "the transport", transport_running);
    let signal = ramp(12_000);
    feed(&mut input, &signal);
    assert_eq!(s.stop_recording().len(), 1);

    let asset = s.project().assets[0].clone();
    assert!(
        asset.managed && asset.path.starts_with("media/"),
        "{asset:?}"
    );
    assert!(asset.name.starts_with("Recording "));
    let managed = peel_project::resolve_asset_path(&project, &asset);
    assert_eq!(decode_file(&managed).unwrap().left(), &signal[..]);
    assert_eq!(hash_file(&managed).unwrap(), asset.hash);
    // The original stays in the recovery folder next to the project.
    let originals: Vec<_> = std::fs::read_dir(dir.path("Song.peeldata/recordings"))
        .unwrap()
        .collect();
    assert_eq!(originals.len(), 1);

    // The saved project opens with its media in place.
    s.save(None).expect("save again");
    let loaded = peel_project::load(&project).expect("load");
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.project.assets.len(), 1);
}

#[test]
fn a_cancelled_take_adds_no_clip_but_keeps_the_audio() {
    let dir = Scratch::new();
    let (mut s, mut input, lane) = rig(&dir);
    s.arm_lane(lane, true);
    s.start_recording().unwrap();
    wait(&mut s, "the transport", transport_running);
    feed(&mut input, &ramp(4_800));
    let path = s.rec.active.as_ref().unwrap().path.clone();
    s.cancel_recording();
    assert!(!s.is_recording());
    assert!(recorded_clips(&s).is_empty());
    assert!(s.project().assets.is_empty());
    assert_eq!(decode_file(&path).unwrap().frames(), 4_800);
    assert!(s.stop_recording().is_empty());
}

#[test]
fn a_take_where_nothing_arrived_leaves_no_file_and_says_so() {
    let dir = Scratch::new();
    let (mut s, _input, lane) = rig(&dir);
    s.arm_lane(lane, true);
    s.start_recording().unwrap();
    wait(&mut s, "the transport", transport_running);
    assert!(s.stop_recording().is_empty());
    assert!(recorded_clips(&s).is_empty());
    assert!(
        s.notice()
            .is_some_and(|n| n.text.contains("Nothing was recorded"))
    );
    let files = std::fs::read_dir(dir.path("takes")).unwrap().count();
    assert_eq!(files, 0, "the empty file was removed");
}

#[test]
fn a_lost_device_ends_the_take_and_keeps_what_was_captured() {
    let dir = Scratch::new();
    let (mut s, mut input, lane) = rig(&dir);
    s.arm_lane(lane, true);
    s.start_recording().unwrap();
    wait(&mut s, "the transport", transport_running);
    feed(&mut input, &ramp(9_600));
    input.fail_device("The device was unplugged");
    s.tick();
    assert!(!s.is_recording(), "a failed input never keeps recording");
    let notice = s.notice().expect("notice").clone();
    assert!(notice.error);
    assert!(notice.text.contains("unplugged"), "{}", notice.text);
    assert_eq!(s.stop_recording().len(), 1);
    assert_eq!(
        s.project().assets[0].frames,
        9_600,
        "the captured audio is kept"
    );
    assert!(s.input_error().is_some());
    assert!(s.record_blocker().unwrap().contains("unplugged"));
}

// ---------------------------------------------------------------------
// Refusals say what to do
// ---------------------------------------------------------------------

#[test]
fn recording_audio_needs_song_mode_an_armed_lane_and_an_input() {
    let dir = Scratch::new();
    let mut s = Session::new_silent(template::beat_and_melody());
    s.set_recordings_dir(Some(dir.path("takes")));
    let lane = s.project().arrangement().tracks[0].id;

    // No input yet.
    s.set_mode(PlayMode::Song);
    s.arm_lane(lane, true);
    let why = s.start_recording().unwrap_err();
    assert!(why.contains("input"), "{why}");

    let _input = s.open_injected_input(48_000, 2);
    s.set_mode(PlayMode::Pattern(s.selected_pattern()));
    let why = s.start_recording().unwrap_err();
    assert!(why.contains("Song mode"), "{why}");

    s.set_mode(PlayMode::Song);
    s.arm_lane(lane, false);
    let why = s.start_recording().unwrap_err();
    assert!(why.contains("Arm"), "{why}");
    assert!(
        !s.is_playing(),
        "a refused take does not start the transport"
    );
    assert!(!s.is_recording());

    // A lane that was deleted is no longer armed.
    s.arm_lane(lane, true);
    assert_eq!(s.armed_lanes(), vec![lane]);
    s.doc.edit("Remove lane", |p| {
        p.arrangement_mut().clips.retain(|c| c.track != lane);
        p.arrangement_mut().tracks.retain(|t| t.id != lane);
    });
    assert!(s.armed_lanes().is_empty());
    s.arm_lane(Id::new(), true);
    assert!(s.armed_lanes().is_empty(), "unknown lanes cannot be armed");

    s.arm_lane(s.project().arrangement().tracks[0].id, true);
    assert_eq!(s.record_blocker(), None);
}

#[test]
fn an_unwritable_folder_is_reported_before_anything_starts() {
    let dir = Scratch::new();
    let (mut s, _input, lane) = rig(&dir);
    std::fs::write(dir.path("file"), b"not a folder").unwrap();
    s.set_recordings_dir(Some(dir.path("file/takes")));
    s.arm_lane(lane, true);
    let why = s.start_recording().unwrap_err();
    assert!(why.contains("Could not start recording"), "{why}");
    assert!(!s.is_recording() && !s.is_playing());
    assert!(s.notice().is_some_and(|n| n.error));
}

#[test]
fn a_silent_session_has_no_hardware_input() {
    let mut s = Session::new_silent(Project::empty("rec"));
    assert!(s.set_input_device(None).is_err());
    assert!(!s.input_is_open());
    let lane = s.project().arrangement().tracks[0].id;
    s.arm_lane(lane, true);
    assert!(!s.input_is_open(), "arming must not open hardware in tests");
}

// ---------------------------------------------------------------------
// Meter, channels and monitoring
// ---------------------------------------------------------------------

#[test]
fn the_input_meter_reads_the_chosen_channels() {
    let dir = Scratch::new();
    let (mut s, _, _) = rig(&dir);
    let mut input = s.open_injected_input(48_000, 2);
    s.set_input_channels(InputChannels::Stereo(0, 1));
    assert_eq!(s.input_level(), (0.0, 0.0));
    // Left quiet, right loud.
    let frames: Vec<f32> = (0..256).flat_map(|_| [0.1, 0.8]).collect();
    input.push(&frames);
    let (l, r) = s.input_level();
    assert!((l - 0.1).abs() < 1e-6 && (r - 0.8).abs() < 1e-6, "{l} {r}");
    assert_eq!(s.input_level(), (0.0, 0.0), "reading clears the peak");

    s.set_input_channels(InputChannels::Mono(1));
    input.push(&frames);
    assert_eq!(s.input_level(), (0.8, 0.8));
    s.set_input_channels(InputChannels::Mono(9));
    assert_eq!(
        s.input_channels(),
        InputChannels::Mono(1),
        "clamped to the device"
    );
    assert_eq!(s.input_info().unwrap().channels, 2);
}

#[test]
fn a_stereo_selection_records_a_stereo_file() {
    let dir = Scratch::new();
    let (mut s, _, lane) = rig(&dir);
    let mut input = s.open_injected_input(48_000, 4);
    s.set_input_channels(InputChannels::Stereo(3, 1));
    s.arm_lane(lane, true);
    s.start_recording().unwrap();
    wait(&mut s, "the transport", transport_running);
    let frames: Vec<f32> = (0..4800)
        .flat_map(|i| {
            let x = i as f32 * 1e-5;
            [x, x + 0.1, x + 0.2, x + 0.3]
        })
        .collect();
    for chunk in frames.chunks(4 * 256) {
        input.push(chunk);
        std::thread::sleep(Duration::from_millis(2));
    }
    // The channel count cannot change under an open file.
    s.set_input_channels(InputChannels::Mono(0));
    assert_eq!(s.input_channels().count(), 2);
    assert_eq!(s.stop_recording().len(), 1);
    let asset = &s.project().assets[0];
    assert_eq!((asset.channels, asset.frames), (2, 4_800));
    let file = decode_file(&asset.path).unwrap();
    for i in 0..4_800 {
        let x = i as f32 * 1e-5;
        assert_eq!((file.left()[i], file.right()[i]), (x + 0.3, x + 0.1));
    }
}

#[test]
fn monitoring_is_off_by_default_and_explains_when_it_cannot_run() {
    let dir = Scratch::new();
    let (mut s, mut input, _) = rig(&dir);
    let status = s.monitor_status();
    assert!(!status.on && status.available, "{status:?}");
    assert_eq!(s.monitoring(), (false, 1.0));

    s.set_monitoring(true, 0.5);
    assert_eq!(s.monitoring(), (true, 0.5));
    for _ in 0..8 {
        input.push(&[0.3f32; 256]);
    }
    let status = s.monitor_status();
    assert!(status.on && status.available);
    assert!(status.latency_ms >= 0.0);

    // An input at another rate than the output cannot be monitored in
    // software; recording it is unaffected.
    let _slow = s.open_injected_input(44_100, 1);
    let status = s.monitor_status();
    assert!(!status.available);
    let why = status.unavailable_reason.unwrap();
    assert!(why.contains("44100") && why.contains("48000"), "{why}");
    s.set_monitoring(true, 1.0);
    assert!(s.notice().is_some_and(|n| n.error));

    s.close_input();
    assert!(!s.monitor_status().available);
}

#[test]
fn a_take_runs_straight_through_the_loop_and_the_end_of_the_song() {
    let dir = Scratch::new();
    let (mut s, mut input, lane) = rig(&dir);
    // Fast, so that a few beats pass in a fraction of a second.
    s.doc.edit("Tempo", |p| p.set_bpm(600.0));
    s.arm_lane(lane, true);
    s.tick();
    let before = s.plan.as_ref().expect("plan");
    assert_eq!(
        before.loop_range,
        Some((0.0, 4.0)),
        "an empty song is a bar"
    );
    assert_eq!(before.length_beats, 4.0);

    s.start_recording().unwrap();
    wait(&mut s, "the transport", transport_running);
    let during = s.plan.as_ref().expect("plan");
    assert_eq!(during.loop_range, None);
    assert!(during.length_beats.is_infinite());

    // Half a second at 600 BPM is five beats: past the end of the one-bar song.
    feed(&mut input, &ramp(24_000));
    // With the loop in force the playhead would wrap at four beats.
    wait(&mut s, "the playhead past the end", |s| s.position() > 4.5);
    assert!(s.is_playing());
    assert_eq!(s.stop_recording().len(), 1);

    let clip = recorded_clips(&s)[0].clone();
    // The whole take is five beats; a few milliseconds of it may fall before
    // the top of the song and be trimmed off.
    let whole = clip.length.as_f64() + clip.offset.as_f64();
    assert!((whole - 5.0).abs() < 1e-3, "{clip:?}");
    // The song's own loop and end are back, now covering the recording.
    s.tick();
    let after = s.plan.as_ref().expect("plan");
    assert_eq!(after.loop_range, Some((0.0, after.length_beats)));
    assert!(after.length_beats >= 8.0 && after.length_beats.is_finite());
}

/// Records one second from the default microphone onto a lane. Run with
/// `--ignored` on a machine with a microphone; macOS asks for permission the
/// first time.
#[test]
#[ignore = "needs an audio input device and microphone permission"]
fn a_real_microphone_take_becomes_a_clip() {
    let dir = Scratch::new();
    let mut s = Session::new(Project::empty("real"));
    s.set_recordings_dir(Some(dir.path("takes")));
    s.set_mode(PlayMode::Song);
    s.set_input_device(None).expect("open the default input");
    assert!(s.input_is_open());
    let lane = s.project().arrangement().tracks[0].id;
    s.arm_lane(lane, true);
    s.start_recording().expect("start");
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(1200) {
        s.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    let ids = s.stop_recording();
    assert_eq!(ids.len(), 1, "{:?}", s.notice());
    let asset = &s.project().assets[0];
    let seconds = asset.frames as f64 / f64::from(asset.sample_rate);
    assert!((0.9..1.4).contains(&seconds), "recorded {seconds} s");
    assert!(decode_file(&asset.path).is_ok());
}

#[test]
fn a_silent_input_is_flagged_until_a_sound_arrives() {
    let dir = Scratch::new();
    let (s, mut input, _) = rig(&dir);
    assert_eq!(s.input_warning(), None, "nothing to say yet");
    // Three and a half seconds of digital silence.
    for _ in 0..660 {
        input.push(&[0.0f32; 256]);
    }
    let hint = s.input_warning().expect("a hint");
    assert!(
        hint.contains("silent") && hint.contains("Microphone"),
        "{hint}"
    );
    input.push(&[0.0f32, 0.001, 0.0, 0.0]);
    assert_eq!(s.input_warning(), None);
    // A failed device is reported in its own words.
    input.fail_device("The device was unplugged");
    assert!(s.input_warning().unwrap().contains("unplugged"));
}

#[test]
fn opening_another_project_forgets_the_armed_lanes() {
    let dir = Scratch::new();
    let (mut s, mut input, lane) = rig(&dir);
    s.arm_lane(lane, true);
    s.start_recording().unwrap();
    wait(&mut s, "the transport", transport_running);
    feed(&mut input, &ramp(4_800));
    s.replace_project(template::beat_and_melody());
    assert!(!s.is_recording());
    assert!(s.armed_lanes().is_empty());
    assert!(
        s.stop_recording().is_empty(),
        "those clips belonged to the old project"
    );
}
