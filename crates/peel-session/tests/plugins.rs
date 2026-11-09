//! CLAP plugins inside a session: playback devices, export, saved state.

#[path = "../../peel-host-clap/tests/fixture/mod.rs"]
mod fixture;

use std::path::Path;
use std::time::{Duration, Instant};

use peel_document::{DeviceInstance, Id};
use peel_session::{ExportSettings, Session, template};

const GAIN_TYPE: &str = "clap:dev.peel.test.gain";

fn export(s: &mut Session, out: &Path) -> Vec<f32> {
    let job = s.export(ExportSettings {
        path: out.to_path_buf(),
        sample_rate: 48_000,
        bits: 32,
        tail_seconds: 0.0,
    });
    let start = Instant::now();
    loop {
        job.idle();
        if let Some(r) = job.take_result() {
            r.expect("export");
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "export timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    peel_media::decode_file(out)
        .expect("decode")
        .left()
        .to_vec()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn session_with_gain(gain: f32) -> (Session, Id) {
    let mut project = template::beat_and_melody();
    let mut inst = DeviceInstance::new(GAIN_TYPE, 1);
    inst.params.insert("7".into(), gain);
    let dev = inst.id;
    // After the master limiter, so the plugin scales the final mix.
    project.mixer.tracks[0].inserts.push(inst);
    let mut s = Session::new_silent(project);
    s.set_plugins(fixture::infos().to_vec());
    s.tick();
    assert!(s.missing_devices().is_empty(), "{:?}", s.missing_devices());
    (s, dev)
}

#[test]
fn a_clap_insert_renders_in_exports() {
    let dir = fixture::temp_dir("session-export");
    let mut plain = Session::new_silent(template::beat_and_melody());
    let reference = export(&mut plain, &dir.join("plain.wav"));
    let (mut s, _) = session_with_gain(0.5);
    let halved = export(&mut s, &dir.join("half.wav"));
    assert_eq!(reference.len(), halved.len());
    let a = &reference[48_000..48_000 * 8];
    let b = &halved[48_000..48_000 * 8];
    let ratio = rms(b) / rms(a);
    assert!((ratio - 0.5).abs() < 0.01, "plugin gain 0.5 gives {ratio}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn plugin_state_is_saved_and_restored() {
    let dir = fixture::temp_dir("session-state");
    let (mut s, dev) = session_with_gain(1.0);
    s.set_param(dev, "7", 2.0, 1);
    s.doc.end_gesture();
    s.tick();
    let path = s.save(Some(dir.join("song.peelproj"))).expect("save");
    let saved = peel_project::load(&path).expect("load").project;
    let inst = saved.mixer.tracks[0]
        .inserts
        .iter()
        .find(|d| d.id == dev)
        .expect("insert saved");
    assert!(
        inst.state.as_deref().is_some_and(|b| !b.is_empty()),
        "plugin state saved"
    );
    assert_eq!(inst.params.get("7"), Some(&2.0));

    let mut reopened = Session::new_silent(template::beat_and_melody());
    reopened.set_plugins(fixture::infos().to_vec());
    reopened.open(&path).expect("open");
    reopened.tick();
    assert!(
        reopened.missing_devices().is_empty(),
        "{:?}",
        reopened.missing_devices()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_plugin_is_reported_not_fatal() {
    let mut project = template::beat_and_melody();
    project.mixer.tracks[0]
        .inserts
        .push(DeviceInstance::new("clap:com.example.not-installed", 1));
    let mut s = Session::new_silent(project);
    s.set_plugins(fixture::infos().to_vec());
    s.tick();
    assert_eq!(s.missing_devices(), ["clap:com.example.not-installed"]);
}

#[test]
fn plugin_parameters_use_the_plugins_own_text() {
    let (s, dev) = session_with_gain(1.5);
    let desc = peel_devices::descriptor(GAIN_TYPE).expect("descriptor once loaded");
    let gain = desc.param_by_key("7").expect("gain param");
    assert_eq!(s.param_text(dev, gain, 1.5), "1.50x");
}

#[test]
fn requested_clap_restart_preserves_the_instance_and_refreshes_compensation() {
    use base64::Engine as _;
    let (mut session, device) = session_with_gain(1.0);
    let ready = Instant::now();
    while session.compensation_latency_samples() == 0 {
        session.tick();
        assert!(ready.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    }
    let initial_latency = session.compensation_latency_samples();
    session.set_param(device, "7", 3.5, 500);
    session.doc.end_gesture();
    session.play();
    // The fixture's main-thread callback changes its live gain to 0.75 and
    // latency to 64, then requests restart. Latency is deliberately absent
    // from its serialized state: recreating it would incorrectly report zero.
    let start = Instant::now();
    loop {
        session.tick();
        if session.compensation_latency_samples() == initial_latency + 64 {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "restart/compensation timed out: {:?}, slot reports {:?}",
            session.compensation_error(),
            session
                .telemetry()
                .slot_latencies
                .iter()
                .map(|s| s.load(std::sync::atomic::Ordering::Relaxed))
                .filter(|&s| s > 0)
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..8 {
        session.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(session.compensation_latency_samples(), initial_latency + 64);
    assert!(session.compensation_error().is_none());
    session.capture_plugin_states();
    let instance = session.doc.project().mixer.tracks[0]
        .inserts
        .iter()
        .find(|d| d.id == device)
        .unwrap();
    let state = base64::engine::general_purpose::STANDARD
        .decode(instance.state.as_deref().unwrap())
        .unwrap();
    let gain = f32::from_le_bytes(state[4..8].try_into().unwrap());
    assert_eq!(
        gain, 0.75,
        "restart must preserve the plugin's private parameter change"
    );
}

#[test]
fn a_template_keeps_live_plugin_state_without_touching_the_project() {
    let dir = fixture::temp_dir("session-template");
    let (mut s, dev) = session_with_gain(1.0);
    s.set_param(dev, "7", 2.0, 1);
    s.doc.end_gesture();
    s.tick();
    let doc_state = |s: &Session| {
        s.project().mixer.tracks[0]
            .inserts
            .iter()
            .find(|d| d.id == dev)
            .and_then(|d| d.state.clone())
    };
    assert_eq!(doc_state(&s), None, "live state is not in the document yet");
    let revision = s.doc.revision();
    let dirty = s.doc.is_dirty();
    let undo = s.doc.undo_label().map(str::to_string);

    let path = s
        .save_template(&dir.join("Templates"), "Gain rig")
        .expect("save template");
    let stored = peel_project::load_template(&path).expect("load").project;
    let inst = stored.mixer.tracks[0]
        .inserts
        .iter()
        .find(|d| d.id == dev)
        .expect("insert kept");
    assert!(inst.state.as_deref().is_some_and(|b| !b.is_empty()));
    assert_eq!(inst.params.get("7"), Some(&2.0));

    // The open project is exactly as it was.
    assert_eq!(doc_state(&s), None);
    assert_eq!(s.doc.revision(), revision);
    assert_eq!(s.doc.is_dirty(), dirty);
    assert_eq!(s.doc.undo_label().map(str::to_string), undo);
    assert!(s.path.is_none());

    // The template starts with the plugin loaded and its state applied.
    let mut fresh = Session::new_silent(template::empty());
    fresh.set_plugins(fixture::infos().to_vec());
    fresh.replace_project_with_media(stored, Some(path));
    fresh.tick();
    assert!(
        fresh.missing_devices().is_empty(),
        "{:?}",
        fresh.missing_devices()
    );
    std::fs::remove_dir_all(&dir).ok();
}
