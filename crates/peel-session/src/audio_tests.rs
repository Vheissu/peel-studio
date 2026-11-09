//! Session-level tests for audio clips, using WAV files written on the fly.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use peel_document::{AudioAsset, Project, TempoSegment};
use peel_dsp::util::db_to_gain;
use peel_media::{WavFormat, write_wav};

use super::*;
use crate::{ExportSettings, PlayMode};

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "peel-session-audio-{}-{}",
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

/// `data[i] = i * 1e-5`, which float WAV stores exactly.
fn ramp_data(frames: usize) -> Vec<f32> {
    (0..frames).map(|i| i as f32 * 1e-5).collect()
}

fn write_ramp(path: &Path, rate: u32, frames: usize) {
    write_wav(path, &[&ramp_data(frames)], rate, WavFormat::FLOAT32).unwrap();
}

fn session() -> Session {
    Session::new_silent(Project::empty("audio"))
}

fn lane(s: &Session, i: usize) -> Id {
    s.project().arrangement().tracks[i].id
}

/// Ticks the session until `done` holds, failing after ten seconds.
fn wait(s: &mut Session, what: &str, mut done: impl FnMut(&mut Session) -> bool) {
    let t0 = Instant::now();
    loop {
        s.tick();
        if done(s) {
            return;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn export_to(s: &mut Session, path: &Path, rate: u32) -> Result<PathBuf, String> {
    let job = s.export(ExportSettings {
        path: path.to_path_buf(),
        sample_rate: rate,
        bits: 32,
        tail_seconds: 0.1,
    });
    let t0 = Instant::now();
    loop {
        if let Some(result) = job.take_result() {
            return result;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "export timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn first_clip(s: &Session) -> Clip {
    s.project().arrangement().clips[0].clone()
}

fn clip_parts(clip: &Clip) -> (Id, Id) {
    match clip.content {
        ClipContent::Audio { asset, track, .. } => (asset, track),
        _ => panic!("not an audio clip"),
    }
}

// ---------------------------------------------------------------------
// Adding clips
// ---------------------------------------------------------------------

#[test]
fn adding_a_clip_is_one_undoable_step() {
    let dir = Scratch::new();
    let wav = dir.path("hit.wav");
    write_ramp(&wav, 48_000, 14_400); // 0.3 s = 0.6 beats at 120 BPM
    let mut s = session();
    let lane0 = lane(&s, 0);
    let before = s.project().clone();

    let id = s.add_audio_clip(lane0, Time::beats(2), &wav).unwrap();

    let p = s.project();
    assert_eq!(p.assets.len(), 1);
    let asset = &p.assets[0];
    assert_eq!(asset.name, "hit.wav");
    assert_eq!(
        (asset.sample_rate, asset.channels, asset.frames),
        (48_000, 1, 14_400)
    );
    assert!(!asset.managed);
    // A mixer track named after the lane, feeding the master.
    assert_eq!(p.mixer.tracks.len(), 2);
    let mixer = &p.mixer.tracks[1];
    assert_eq!(mixer.name, "Track 1");
    assert_eq!(mixer.output, None);
    // One clip: starts where asked, covers the file rounded up to a 16th.
    let clip = &p.arrangement().clips[0];
    assert_eq!(clip.id, id);
    assert_eq!(clip.track, lane0);
    assert_eq!(clip.start, Time::beats(2));
    assert_eq!(clip.length, Time::new(3, 4));
    assert_eq!(clip.offset, Time::ZERO);
    assert!(!clip.muted);
    assert_eq!(clip.content, ClipContent::audio(asset.id, mixer.id));
    assert!(s.doc.validate().iter().all(|i| !i.is_error()));
    assert_eq!(s.doc.undo_label(), Some("Add audio clip"));

    // Undo removes the clip, the asset and the mixer track together.
    s.doc.undo();
    assert_eq!(s.project().assets, before.assets);
    assert_eq!(s.project().mixer, before.mixer);
    assert!(s.project().arrangement().clips.is_empty());
    s.doc.redo();
    assert_eq!(s.project().assets.len(), 1);
    assert_eq!(s.project().mixer.tracks.len(), 2);
    assert_eq!(s.project().arrangement().clips.len(), 1);
}

#[test]
fn a_lane_keeps_one_mixer_track_and_a_file_is_one_asset() {
    let dir = Scratch::new();
    let (a, b) = (dir.path("a.wav"), dir.path("b.wav"));
    write_ramp(&a, 48_000, 24_000);
    write_ramp(&b, 48_000, 30_000);
    let mut s = session();
    let (l0, l1) = (lane(&s, 0), lane(&s, 1));

    s.add_audio_clip(l0, Time::ZERO, &a).unwrap();
    s.add_audio_clip(l0, Time::beats(4), &a).unwrap();
    s.add_audio_clip(l0, Time::beats(8), &b).unwrap();
    assert_eq!(s.project().assets.len(), 2, "the same file is one asset");
    assert_eq!(
        s.project().mixer.tracks.len(),
        2,
        "one mixer track per lane"
    );
    let tracks: Vec<Id> = s
        .project()
        .arrangement()
        .clips
        .iter()
        .map(|c| clip_parts(c).1)
        .collect();
    assert!(tracks.iter().all(|t| *t == tracks[0]));

    s.add_audio_clip(l1, Time::ZERO, &a).unwrap();
    assert_eq!(s.project().mixer.tracks.len(), 3);
    assert_eq!(s.project().mixer.tracks[2].name, "Track 2");
    assert_ne!(clip_parts(&s.project().arrangement().clips[3]).1, tracks[0]);
}

#[test]
fn a_lane_adopts_an_unused_mixer_track_of_the_same_name() {
    let dir = Scratch::new();
    let wav = dir.path("a.wav");
    write_ramp(&wav, 48_000, 24_000);
    let mut s = session();
    let l0 = lane(&s, 0);
    let clip = s.add_audio_clip(l0, Time::ZERO, &wav).unwrap();
    // Deleting the clip leaves the lane's track behind; the next clip on the
    // lane reuses it instead of piling up tracks.
    s.doc.edit("Delete clip", |p| {
        p.arrangement_mut().clips.retain(|c| c.id != clip);
    });
    s.add_audio_clip(l0, Time::ZERO, &wav).unwrap();
    assert_eq!(s.project().mixer.tracks.len(), 2);
}

#[test]
fn bad_requests_fail_without_touching_the_project() {
    let dir = Scratch::new();
    let wav = dir.path("a.wav");
    write_ramp(&wav, 48_000, 24_000);
    let text = dir.path("notes.wav");
    std::fs::write(&text, "this is not audio").unwrap();
    let mut s = session();
    let l0 = lane(&s, 0);
    let revision = s.doc.revision();

    assert!(s.add_audio_clip(Id::new(), Time::ZERO, &wav).is_err());
    assert!(s.add_audio_clip(l0, Time::ZERO, &text).is_err());
    assert!(
        s.add_audio_clip(l0, Time::ZERO, &dir.path("missing.wav"))
            .is_err()
    );
    assert_eq!(s.doc.revision(), revision);
    assert!(!s.doc.can_undo());
    assert!(s.project().assets.is_empty());
}

#[test]
fn clip_length_follows_the_tempo_and_rounds_up_to_a_sixteenth() {
    let t120 = TempoMap::constant(120.0);
    assert_eq!(clip_length(&t120, Time::ZERO, 1.0), Time::beats(2));
    assert_eq!(clip_length(&t120, Time::ZERO, 1.01), Time::new(9, 4));
    assert_eq!(clip_length(&t120, Time::ZERO, 0.001), Time::new(1, 4));
    assert_eq!(
        clip_length(&TempoMap::constant(90.0), Time::ZERO, 1.0),
        Time::new(3, 2)
    );
    // The tempo at the clip's own position decides.
    let changing = TempoMap::from_segments(&[
        TempoSegment {
            start: Time::ZERO,
            bpm: 120.0,
            ramp_to: None,
        },
        TempoSegment {
            start: Time::beats(8),
            bpm: 60.0,
            ramp_to: None,
        },
    ]);
    assert_eq!(clip_length(&changing, Time::beats(8), 1.0), Time::beats(1));
    assert_eq!(clip_length(&changing, Time::beats(4), 1.0), Time::beats(2));
}

// ---------------------------------------------------------------------
// Loading and the plan
// ---------------------------------------------------------------------

#[test]
fn audio_reaches_the_plan_and_peaks_when_decoding_finishes() {
    let dir = Scratch::new();
    let wav = dir.path("loop.wav");
    write_ramp(&wav, 48_000, 48_000);
    let mut s = session();
    s.set_mode(PlayMode::Song);
    let l0 = lane(&s, 0);
    let id = s.add_audio_clip(l0, Time::beats(1), &wav).unwrap();
    let (asset, _) = clip_parts(&s.project().arrangement().clips[0]);
    assert_eq!(s.project().arrangement().clips[0].id, id);

    // Not decoded yet: the clip is silent and nothing is cached.
    assert!(s.audio_peaks(asset).is_none());
    assert!(s.plan.as_ref().unwrap().audio.is_empty());
    assert!(s.audio_loading(asset));
    assert_eq!(s.asset_duration_seconds(asset), Some(1.0));
    let revision = s.doc.revision();

    wait(&mut s, "audio decoded", |s| s.audio_peaks(asset).is_some());

    assert_eq!(s.doc.revision(), revision, "no document edit was needed");
    assert!(!s.audio_loading(asset));
    let peaks = s.audio_peaks(asset).unwrap();
    assert_eq!(peaks.frames(), 48_000);
    // Frame 0 is silent and the ramp climbs: the last column peaks highest.
    let cols: Vec<(f32, f32)> = peaks.query(0, 48_000, 4).collect();
    assert!(cols[3].1 > cols[0].1);

    let plan = s.plan.as_ref().unwrap();
    assert_eq!(plan.audio.len(), 1);
    let r = &plan.audio[0];
    assert_eq!((r.start_beat, r.end_beat), (1.0, 3.0));
    assert!(Arc::ptr_eq(&r.sample, &s.sample(asset).unwrap()));
    assert_eq!(r.track as usize, plan.tracks.len() - 2, "the lane's track");
    assert_eq!(s.asset_duration_seconds(asset), Some(1.0));
}

#[test]
fn pattern_mode_ignores_clips_but_still_decodes_them() {
    let dir = Scratch::new();
    let wav = dir.path("loop.wav");
    write_ramp(&wav, 48_000, 24_000);
    let mut s = session();
    let l0 = lane(&s, 0);
    s.add_audio_clip(l0, Time::ZERO, &wav).unwrap();
    let (asset, _) = clip_parts(&first_clip(&s));
    // Waveforms are needed whatever the transport plays.
    wait(&mut s, "audio decoded", |s| s.audio_peaks(asset).is_some());
    assert!(matches!(s.mode(), PlayMode::Pattern(_)));
    assert!(s.plan.as_ref().unwrap().audio.is_empty());
    s.set_mode(PlayMode::Song);
    s.tick();
    assert_eq!(s.plan.as_ref().unwrap().audio.len(), 1);
}

#[test]
fn a_file_that_cannot_be_decoded_is_reported_once_and_not_retried() {
    let dir = Scratch::new();
    let broken = dir.path("broken.wav");
    std::fs::write(&broken, "garbage").unwrap();
    let mut s = session();
    s.set_mode(PlayMode::Song);
    // A project that already references a damaged file.
    let asset = Id::new();
    let (lane0, master) = (lane(&s, 0), s.project().mixer.tracks[0].id);
    s.doc.edit("Add broken clip", |p| {
        p.assets.push(AudioAsset {
            id: asset,
            hash: "00".into(),
            name: "broken.wav".into(),
            path: broken.to_string_lossy().into_owned(),
            managed: false,
            sample_rate: 48_000,
            channels: 1,
            frames: 48_000,
        });
        p.arrangement_mut().clips.push(Clip {
            id: Id::new(),
            track: lane0,
            start: Time::ZERO,
            length: Time::beats(2),
            offset: Time::ZERO,
            muted: false,
            content: ClipContent::audio(asset, master),
        });
    });
    s.tick();
    assert!(s.audio_loading(asset));

    wait(&mut s, "failure reported", |s| {
        s.audio.failed.contains(&asset)
    });
    assert!(s.notice().is_some_and(|n| n.error));
    assert!(s.audio_peaks(asset).is_none());
    assert!(!s.audio_loading(asset));
    assert!(s.plan.as_ref().unwrap().audio.is_empty());
    // Editing does not queue the broken file again.
    s.doc.edit("Move", |p| {
        p.arrangement_mut().clips[0].start = Time::beats(1);
    });
    s.tick();
    assert!(!s.audio_loading(asset));
}

// ---------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------

/// Project with one audio clip at beat 2, one beat long, starting half a
/// beat into a one second ramp file.
fn clip_session(dir: &Scratch) -> (Session, Id) {
    let wav = dir.path("ramp.wav");
    write_ramp(&wav, 48_000, 48_000);
    let mut s = session();
    let l0 = lane(&s, 0);
    let id = s.add_audio_clip(l0, Time::beats(2), &wav).unwrap();
    s.doc.edit("Trim", |p| {
        let clip = &mut p.arrangement_mut().clips[0];
        clip.length = Time::beats(1);
        clip.offset = Time::new(1, 2);
    });
    (s, id)
}

fn read_wav(path: &Path) -> Vec<f32> {
    peel_media::decode_file(path).unwrap().channels.remove(0)
}

#[test]
fn export_places_trimmed_audio_at_the_right_samples() {
    let dir = Scratch::new();
    // Not decoded when the export starts: the export loads it itself.
    let (mut s, _) = clip_session(&dir);
    let out = dir.path("mix.wav");
    export_to(&mut s, &out, 48_000).unwrap();
    let mix = read_wav(&out);

    let ramp = ramp_data(48_000);
    // Beat 2 at 120 BPM is sample 48000; half a beat of offset is 12000
    // frames into the file; the clip is one beat (24000 frames) long.
    let (start, offset, len) = (48_000usize, 12_000usize, 24_000usize);
    assert!(mix[..start].iter().all(|&x| x == 0.0), "silent before");
    for k in 64..len - 64 {
        assert_eq!(mix[start + k], ramp[offset + k], "frame {k}");
    }
    assert!(mix[start] != 0.0, "the first frame is already audible");
    assert!(mix[start + len - 1] != 0.0);
    assert!(mix[start + len..].iter().all(|&x| x == 0.0), "silent after");
}

#[test]
fn export_resamples_decoded_audio_to_the_export_rate() {
    let dir = Scratch::new();
    let (mut s, id) = clip_session(&dir);
    let (asset, _) = clip_parts(&s.project().arrangement().clips[0]);
    assert_eq!(s.project().arrangement().clips[0].id, id);
    // Decoded at the engine's 48 kHz, exported at 44.1 kHz.
    wait(&mut s, "audio decoded", |s| s.audio_peaks(asset).is_some());
    let out = dir.path("mix.wav");
    export_to(&mut s, &out, 44_100).unwrap();
    let mix = read_wav(&out);

    // Beat 2 is sample 44100 at this rate.
    let start = 44_100usize;
    assert!(mix[..start].iter().all(|&x| x == 0.0));
    assert!(mix[start] != 0.0);
    for k in [200usize, 5_000, 12_000, 20_000] {
        // Output frame k is k * 48000/44100 source frames past the offset.
        let want = (12_000.0 + k as f64 * 48_000.0 / 44_100.0) * 1e-5;
        assert!(
            (mix[start + k] as f64 - want).abs() < 1e-3,
            "frame {k}: {} vs {want}",
            mix[start + k]
        );
    }
    let len = 22_050usize;
    assert!(mix[start + len + 10..].iter().all(|&x| x == 0.0));
}

#[test]
fn a_muted_clip_is_not_exported_and_needs_no_file() {
    let dir = Scratch::new();
    let (mut s, _) = clip_session(&dir);
    s.doc
        .edit("Mute", |p| p.arrangement_mut().clips[0].muted = true);
    std::fs::remove_file(dir.path("ramp.wav")).unwrap();
    let out = dir.path("mix.wav");
    export_to(&mut s, &out, 48_000).unwrap();
    assert!(read_wav(&out).iter().all(|&x| x == 0.0));
}

#[test]
fn a_clip_whose_file_is_missing_fails_the_export_by_name() {
    let dir = Scratch::new();
    let (mut s, _) = clip_session(&dir);
    std::fs::remove_file(dir.path("ramp.wav")).unwrap();
    let err = export_to(&mut s, &dir.path("mix.wav"), 48_000).unwrap_err();
    assert!(err.contains("ramp.wav"), "{err}");
    assert!(!dir.path("mix.wav").exists());
}

#[test]
fn the_cli_style_render_includes_clips_too() {
    // `render` is the entry the command-line tool uses: it gets a decoded
    // sample map and no session.
    let dir = Scratch::new();
    let wav = dir.path("ramp.wav");
    write_ramp(&wav, 48_000, 48_000);
    let mut p = Project::empty("cli");
    let master = p.mixer.tracks[0].id;
    let lane = p.arrangement().tracks[0].id;
    let asset = Id::new();
    p.assets.push(AudioAsset {
        id: asset,
        hash: "00".into(),
        name: "ramp.wav".into(),
        path: wav.to_string_lossy().into_owned(),
        managed: false,
        sample_rate: 48_000,
        channels: 1,
        frames: 48_000,
    });
    p.arrangement_mut().clips.push(Clip {
        id: Id::new(),
        track: lane,
        start: Time::beats(1),
        length: Time::beats(2),
        offset: Time::ZERO,
        muted: false,
        content: ClipContent::Audio {
            asset,
            gain_db: -6.0,
            reversed: true,
            track: master,
            fade_in: Time::ZERO,
            fade_out: Time::ZERO,
            source_bpm: None,
        },
    });
    let samples = HashMap::from([(
        asset,
        Arc::new(peel_media::load_for_engine(&wav, 48_000.0).unwrap()),
    )]);
    let out = dir.path("mix.wav");
    let settings = ExportSettings {
        path: out.clone(),
        sample_rate: 48_000,
        bits: 32,
        tail_seconds: 0.0,
    };
    crate::render_to_wav(
        &p,
        &samples,
        &settings,
        &AtomicU32::new(0),
        &AtomicBool::new(false),
    )
    .unwrap();
    let mix = read_wav(&out);
    // Reversed: the first output frame of the clip is the last frame of the
    // two-beat (48000 frame) window, which here is the whole file.
    let gain = db_to_gain(-6.0);
    let ramp = ramp_data(48_000);
    let start = 24_000usize;
    for k in [100usize, 1_000, 30_000, 47_000] {
        let want = ramp[48_000 - 1 - k] * gain;
        assert!(
            (mix[start + k] - want).abs() < 1e-6,
            "frame {k}: {} vs {want}",
            mix[start + k]
        );
    }
}
