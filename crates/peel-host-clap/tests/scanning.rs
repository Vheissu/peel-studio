//! Discovery, bundle handling and the out-of-process scanner.

mod fixture;

use std::path::Path;
use std::time::Duration;

use fixture::*;
use peel_host_clap::{
    ClapError, ClapPluginInfo, ClapRole, candidate_files, scan_file, scan_file_isolated,
    scan_isolated, scan_paths,
};

const SCANNER: &str = env!("CARGO_BIN_EXE_peel-clap-scan");

fn by_id<'a>(plugins: &'a [ClapPluginInfo], id: &str) -> &'a ClapPluginInfo {
    plugins
        .iter()
        .find(|p| p.id == id)
        .unwrap_or_else(|| panic!("{id} not found in {} plugins", plugins.len()))
}

#[test]
fn a_directory_scan_finds_every_plugin_in_the_file() {
    let dir = temp_dir("scan-dir");
    let clap = install(&dir.join("vendor"), "peel-test");
    let report = scan_paths(std::slice::from_ref(&dir));
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.plugins.len(), 4);

    let gain = by_id(&report.plugins, GAIN);
    assert_eq!(gain.name, "Test Gain");
    assert_eq!(gain.vendor, "Peel Studio");
    assert_eq!(gain.version, "1.2.3");
    assert_eq!(gain.description, "Multiplies the signal");
    assert_eq!(gain.url, "https://github.com/Vheissu/peel-studio");
    assert_eq!(gain.features, ["audio-effect", "utility", "stereo"]);
    assert_eq!(gain.role, ClapRole::AudioEffect);
    assert!(gain.is_effect() && !gain.is_instrument() && gain.is_hostable());
    assert_eq!(gain.index, 0);
    assert_eq!(gain.path, clap);
    assert_eq!(gain.type_id(), "clap:dev.peel.test.gain");
    assert!(gain.has_gui, "the gain plugin exposes clap.gui");
    assert!(gain.has_state);
    assert_eq!((gain.audio_inputs, gain.audio_outputs), (1, 1));
    assert_eq!((gain.note_inputs, gain.note_outputs), (0, 0));
    assert_eq!(gain.param_count, 2);
    assert_eq!(gain.probe_error, None);
    assert!(gain.clap_version.starts_with("1."));

    let sine = by_id(&report.plugins, SINE);
    assert_eq!(sine.role, ClapRole::Instrument);
    assert!(sine.is_instrument());
    assert_eq!(sine.index, 2);
    assert!(!sine.has_gui);
    assert_eq!((sine.audio_inputs, sine.audio_outputs), (0, 2));
    assert_eq!((sine.note_inputs, sine.note_outputs), (1, 0));
    assert_eq!(
        sine.param_count, 1,
        "hidden and read-only parameters are not counted"
    );

    assert_eq!(by_id(&report.plugins, MONO_GAIN).features[2], "mono");
    assert_eq!(by_id(&report.plugins, MIDI_SINE).index, 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_file_reads_a_bare_library_path_too() {
    let plugins = scan_file(library()).expect("scan");
    assert_eq!(plugins.len(), 4);
    let ids: Vec<&str> = plugins.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, [GAIN, MONO_GAIN, SINE, MIDI_SINE]);
}

#[test]
fn a_bundle_without_an_info_plist_is_found_by_its_name() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let dir = temp_dir("scan-bundle");
    let clap = install(&dir, "NoPlist");
    std::fs::remove_file(clap.join("Contents").join("Info.plist")).unwrap();
    let plugins = scan_file(&clap).expect("scan by bundle name");
    assert_eq!(plugins.len(), 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_clap_file_without_the_entry_symbol_is_an_error() {
    let dir = temp_dir("scan-noentry");
    // Any real shared library that is not a CLAP plugin.
    let some_lib = if cfg!(target_os = "macos") {
        "/usr/lib/libSystem.B.dylib"
    } else if cfg!(windows) {
        "C:\\Windows\\System32\\kernel32.dll"
    } else {
        "libm.so.6"
    };
    let fake = dir.join("fake.clap");
    if Path::new(some_lib).is_absolute() && Path::new(some_lib).exists() {
        std::fs::copy(some_lib, &fake).unwrap();
    } else {
        // The dynamic loader can find it, but there is nothing to copy.
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    match scan_file(&fake) {
        Err(ClapError::NoEntry(_)) | Err(ClapError::Load { .. }) => {}
        other => panic!("expected a load error, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn candidates_skip_non_plugins() {
    let dir = temp_dir("scan-cands");
    install(&dir, "a");
    std::fs::write(dir.join("notes.txt"), b"x").unwrap();
    let files = candidate_files(std::slice::from_ref(&dir));
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("a.clap"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- out of process --------------------------------------------------------

#[test]
fn the_child_process_reports_the_same_plugins() {
    let dir = temp_dir("iso-ok");
    let clap = install(&dir, "peel-test");
    let isolated = scan_file_isolated(Path::new(SCANNER), &clap, Duration::from_secs(30))
        .expect("isolated scan");
    let direct = scan_file(&clap).expect("direct scan");
    assert_eq!(isolated, direct);
    assert_eq!(isolated.len(), 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_crashing_plugin_is_contained() {
    let dir = temp_dir("iso-crash");
    let clap = install(&dir, "crash-me");
    let err = scan_file_isolated(Path::new(SCANNER), &clap, Duration::from_secs(30))
        .expect_err("must fail");
    assert!(matches!(err, ClapError::ScannerCrashed { .. }), "{err}");
    // The test process is still here to check it.
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_hanging_plugin_is_killed_at_the_timeout() {
    let dir = temp_dir("iso-hang");
    let clap = install(&dir, "hang-me");
    let started = std::time::Instant::now();
    let err = scan_file_isolated(Path::new(SCANNER), &clap, Duration::from_millis(800))
        .expect_err("must fail");
    assert!(matches!(err, ClapError::ScannerTimedOut(_)), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_plugin_that_refuses_to_init_is_reported_by_the_worker() {
    let dir = temp_dir("iso-refuse");
    let clap = install(&dir, "refuse-me");
    let err = scan_file_isolated(Path::new(SCANNER), &clap, Duration::from_secs(30))
        .expect_err("must fail");
    match err {
        ClapError::Scanner(message) => assert!(message.contains("init failed"), "{message}"),
        other => panic!("unexpected {other}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_bad_plugin_does_not_stop_the_rest_of_the_scan() {
    let dir = temp_dir("iso-mixed");
    install(&dir, "good");
    install(&dir, "crash-bundle");
    install(&dir, "hang-bundle");
    install(&dir, "refuse-bundle");
    let report = scan_isolated(
        Path::new(SCANNER),
        std::slice::from_ref(&dir),
        Duration::from_secs(2),
    );
    assert_eq!(report.plugins.len(), 4, "only the good file loads");
    assert_eq!(report.failures.len(), 3);
    for failure in &report.failures {
        assert!(!failure.error.is_empty());
    }
    let named = |needle: &str| {
        report
            .failures
            .iter()
            .find(|f| f.path.to_string_lossy().contains(needle))
            .unwrap_or_else(|| panic!("no failure for {needle}"))
    };
    assert!(named("crash-bundle").error.contains("crashed"));
    assert!(named("hang-bundle").error.contains("timed out"));
    assert!(named("refuse-bundle").error.contains("init failed"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_scanner_is_a_clean_error() {
    let err = scan_file_isolated(
        Path::new("/definitely/not/a/scanner"),
        library(),
        Duration::from_secs(1),
    )
    .expect_err("must fail");
    assert!(matches!(err, ClapError::Scanner(_)), "{err}");
}
