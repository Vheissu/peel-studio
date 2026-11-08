//! Builds the `peel-clap-test-plugin` library and finds its plugins.
//!
//! `cargo test -p peel-host-clap` does not build other workspace members, so
//! the first test to need the library runs `cargo build -p
//! peel-clap-test-plugin` with the same profile as the test binary and reads
//! the artifact path from cargo's JSON output. Later calls reuse the result.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use peel_host_clap::{ClapPluginInfo, scan_file};

pub const GAIN: &str = "dev.peel.test.gain";
pub const MONO_GAIN: &str = "dev.peel.test.mono-gain";
pub const SINE: &str = "dev.peel.test.sine";
pub const MIDI_SINE: &str = "dev.peel.test.midi-sine";

pub const SR: f32 = 48_000.0;
pub const BLOCK: usize = 512;

/// Path of the built fixture library (`.dylib`, `.so` or `.dll`).
pub fn library() -> &'static Path {
    static LIB: OnceLock<PathBuf> = OnceLock::new();
    LIB.get_or_init(build_library)
}

fn build_library() -> PathBuf {
    // The test binary lives in `<target>/<profile>/deps/`.
    let exe = std::env::current_exe().expect("test executable path");
    let profile = exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .expect("profile directory")
        .to_string();

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args([
        "build",
        "-p",
        "peel-clap-test-plugin",
        "--message-format=json",
    ])
    .current_dir(env!("CARGO_MANIFEST_DIR"))
    .stdout(Stdio::piped())
    .stderr(Stdio::inherit());
    match profile.as_str() {
        "debug" => {}
        "release" => {
            cmd.arg("--release");
        }
        other => {
            cmd.args(["--profile", other]);
        }
    }
    let output = cmd.output().expect("run cargo build for the test plugin");
    assert!(
        output.status.success(),
        "building peel-clap-test-plugin failed"
    );

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if msg["reason"] != "compiler-artifact" || msg["target"]["name"] != "peel_clap_test_plugin"
        {
            continue;
        }
        let dylib = msg["filenames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f.as_str())
            .find(|f| [".dylib", ".so", ".dll"].iter().any(|ext| f.ends_with(ext)));
        if let Some(path) = dylib {
            return PathBuf::from(path);
        }
    }
    panic!("cargo did not report a cdylib artifact for peel-clap-test-plugin");
}

/// Every plugin in the fixture library, scanned in this process.
pub fn infos() -> &'static [ClapPluginInfo] {
    static INFOS: OnceLock<Vec<ClapPluginInfo>> = OnceLock::new();
    INFOS.get_or_init(|| scan_file(library()).expect("scan the test library"))
}

pub fn info(id: &str) -> ClapPluginInfo {
    infos()
        .iter()
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("fixture plugin {id} not found"))
        .clone()
}

/// A scratch directory unique to this test process and `tag`.
pub fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("peel-clap-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Copies the fixture library into `dir` as `<stem>.clap`: a bundle with an
/// `Info.plist` on macOS, a plain file elsewhere. Returns the `.clap` path.
pub fn install(dir: &Path, stem: &str) -> PathBuf {
    let clap = dir.join(format!("{stem}.clap"));
    if cfg!(target_os = "macos") {
        let macos = clap.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).expect("create bundle");
        std::fs::copy(library(), macos.join(stem)).expect("copy binary");
        std::fs::write(
            clap.join("Contents").join("Info.plist"),
            format!(
                "<?xml version=\"1.0\"?>\n<plist version=\"1.0\"><dict>\n\
                 <key>CFBundleExecutable</key><string>{stem}</string>\n\
                 </dict></plist>\n"
            ),
        )
        .expect("write Info.plist");
    } else {
        std::fs::copy(library(), &clap).expect("copy library");
    }
    clap
}

/// A deterministic test signal whose samples are exactly representable, so
/// gain by a power of two stays exact.
pub fn ramp(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| ((i % 64) as f32 - 31.5) / 32.0)
        .collect()
}
