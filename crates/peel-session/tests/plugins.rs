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
