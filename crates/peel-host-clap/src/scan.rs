//! Finding CLAP plugins on disk and describing them.
//!
//! [`scan`] walks the standard search paths and reads every plugin's
//! descriptor, then briefly creates each plugin (without activating it) to
//! record its ports, parameter count and GUI/state support.
//!
//! **Scanning runs plugin code.** Loading a library runs its initialisers and
//! creating a plugin runs its constructor. A plugin that crashes in either
//! takes down the process doing the scan, and `catch_unwind` cannot help with
//! a segfault or abort. The `catch_unwind` in [`scan_file`] only contains
//! panics in this crate's own Rust code. To survive a bad plugin, run the scan
//! in a child process: see [`crate::scan_file_isolated`] and the
//! `peel-clap-scan` binary, which call [`scan_file`] on the other side.

use std::collections::HashSet;
use std::ffi::{OsString, c_char};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use clap_sys::plugin::clap_plugin_descriptor;
use clap_sys::version::clap_version_is_compatible;
use serde::{Deserialize, Serialize};

use crate::error::ClapError;
use crate::info::{ClapPluginInfo, ClapRole, classify};
use crate::instance::PluginShared;
use crate::module::{ModuleHandle, version_string};

/// Directory depth searched below each search path.
const MAX_DEPTH: usize = 6;
/// A file reporting more plugins than this is treated as broken.
const MAX_PLUGINS_PER_FILE: u32 = 1024;
const MAX_FEATURES: usize = 64;

/// One file that could not be scanned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanFailure {
    pub path: PathBuf,
    pub error: String,
}

/// Everything a scan found and everything it could not read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanReport {
    pub plugins: Vec<ClapPluginInfo>,
    pub failures: Vec<ScanFailure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Os {
    Mac,
    Windows,
    Other,
}

fn current_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Other
    }
}

/// The standard CLAP locations for `os`, then `CLAP_PATH`.
fn standard_paths(os: Os, var: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(list) = var("CLAP_PATH") {
        paths.extend(std::env::split_paths(&list));
    }
    let home = var("HOME").map(PathBuf::from);
    match os {
        Os::Mac => {
            if let Some(home) = &home {
                paths.push(home.join("Library/Audio/Plug-Ins/CLAP"));
            }
            paths.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        }
        Os::Windows => {
            if let Some(common) = var("COMMONPROGRAMFILES") {
                paths.push(PathBuf::from(common).join("CLAP"));
            }
            if let Some(local) = var("LOCALAPPDATA") {
                paths.push(
                    PathBuf::from(local)
                        .join("Programs")
                        .join("Common")
                        .join("CLAP"),
                );
            }
        }
        Os::Other => {
            if let Some(home) = &home {
                paths.push(home.join(".clap"));
            }
            paths.push(PathBuf::from("/usr/lib/clap"));
        }
    }
    let mut seen = HashSet::new();
    paths.retain(|p| !p.as_os_str().is_empty() && seen.insert(p.clone()));
    paths
}

/// Where CLAP plugins are looked for on this machine: `CLAP_PATH` entries
/// first, then the platform's standard folders. Folders need not exist.
///
/// - macOS: `~/Library/Audio/Plug-Ins/CLAP`, `/Library/Audio/Plug-Ins/CLAP`
/// - Windows: `%COMMONPROGRAMFILES%\CLAP`, `%LOCALAPPDATA%\Programs\Common\CLAP`
/// - Linux and others: `~/.clap`, `/usr/lib/clap`
pub fn search_paths() -> Vec<PathBuf> {
    standard_paths(current_os(), &|name| std::env::var_os(name))
}

fn is_clap_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("clap"))
}

fn collect_candidates(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if is_clap_path(&path) {
            // A macOS bundle is a directory; never look inside it for more.
            out.push(path);
        } else if depth < MAX_DEPTH && path.is_dir() {
            collect_candidates(&path, depth + 1, out);
        }
    }
}

/// Every `.clap` file or bundle under `dirs`, sorted and de-duplicated.
/// An entry of `dirs` that is itself a `.clap` path is included as is.
pub fn candidate_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in dirs {
        if is_clap_path(dir) && dir.exists() {
            found.push(dir.clone());
        } else {
            collect_candidates(dir, 0, &mut found);
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Scans the standard search paths in this process and returns what loaded.
///
/// Files that fail are logged and skipped. Run it off the UI thread, and see
/// the module docs for why a production scan should use
/// [`crate::scan_isolated`] instead.
pub fn scan() -> Vec<ClapPluginInfo> {
    scan_paths(&search_paths()).plugins
}

/// Scans `dirs` (and any `.clap` paths among them) in this process.
pub fn scan_paths(dirs: &[PathBuf]) -> ScanReport {
    let mut report = ScanReport::default();
    for file in candidate_files(dirs) {
        match scan_file(&file) {
            Ok(mut plugins) => report.plugins.append(&mut plugins),
            Err(e) => {
                log::warn!("CLAP scan: {e}");
                report.failures.push(ScanFailure {
                    path: file,
                    error: e.to_string(),
                });
            }
        }
    }
    report
}

/// Reads every plugin in one `.clap` file or bundle.
///
/// This is the unit of work for the out-of-process scanner. Panics in the
/// host's own code are caught and returned as [`ClapError::Panic`]; a crash
/// inside the plugin is not recoverable in-process.
pub fn scan_file(path: &Path) -> Result<Vec<ClapPluginInfo>, ClapError> {
    std::panic::catch_unwind(AssertUnwindSafe(|| probe_file(path))).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        Err(ClapError::Panic {
            what: path.display().to_string(),
            message,
        })
    })
}

fn probe_file(path: &Path) -> Result<Vec<ClapPluginInfo>, ClapError> {
    let module = ModuleHandle::open(path)?;
    let count = module.plugin_count();
    if count > MAX_PLUGINS_PER_FILE {
        return Err(ClapError::Unsupported(format!(
            "{} reports {count} plugins",
            path.display()
        )));
    }
    let mut plugins = Vec::new();
    for index in 0..count {
        let Some(desc) = module.descriptor(index) else {
            continue;
        };
        let Some(mut info) = info_from_descriptor(desc, path, index) else {
            continue;
        };
        probe_instance(
            &module,
            &mut info,
            clap_version_is_compatible(desc.clap_version),
        );
        plugins.push(info);
    }
    Ok(plugins)
}

/// Copies a string out of a descriptor. Null becomes empty.
fn c_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: descriptor strings are NUL-terminated and live as long as the
    // library, which the caller keeps loaded.
    unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

fn features_of(desc: &clap_plugin_descriptor) -> Vec<String> {
    let mut features = Vec::new();
    if desc.features.is_null() {
        return features;
    }
    for i in 0..MAX_FEATURES {
        // SAFETY: the array is NULL-terminated; we stop at the terminator
        // or after `MAX_FEATURES` entries.
        let ptr = unsafe { *desc.features.add(i) };
        if ptr.is_null() {
            break;
        }
        features.push(c_string(ptr));
    }
    features
}

fn info_from_descriptor(
    desc: &clap_plugin_descriptor,
    path: &Path,
    index: u32,
) -> Option<ClapPluginInfo> {
    let id = c_string(desc.id);
    if id.is_empty() {
        return None;
    }
    let features = features_of(desc);
    let name = c_string(desc.name);
    Some(ClapPluginInfo {
        name: if name.is_empty() { id.clone() } else { name },
        vendor: c_string(desc.vendor),
        version: c_string(desc.version),
        description: c_string(desc.description),
        url: c_string(desc.url),
        role: classify(&features, 0, 0),
        features,
        path: path.to_path_buf(),
        index,
        clap_version: version_string(desc.clap_version),
        has_gui: false,
        has_state: false,
        audio_inputs: 0,
        audio_outputs: 0,
        note_inputs: 0,
        note_outputs: 0,
        param_count: 0,
        probe_error: None,
        id,
    })
}

/// Creates the plugin once, without activating it, to fill in capabilities.
fn probe_instance(module: &ModuleHandle, info: &mut ClapPluginInfo, compatible: bool) {
    if !compatible {
        info.probe_error = Some(format!(
            "built for CLAP {}, which this host cannot use",
            info.clap_version
        ));
        return;
    }
    match PluginShared::open(module.clone(), &info.id, &info.name) {
        Ok(shared) => {
            info.has_gui = shared.has_gui();
            info.has_state = shared.has_state();
            info.audio_inputs = shared.audio_in.len() as u32;
            info.audio_outputs = shared.audio_out.len() as u32;
            info.note_inputs = shared.note_in.len() as u32;
            info.note_outputs = shared.note_out_count;
            info.param_count = shared.exposed.len() as u32;
            info.role = classify(&info.features, info.audio_inputs, info.note_inputs);
            if matches!(info.role, ClapRole::Instrument | ClapRole::AudioEffect)
                && info.audio_outputs == 0
            {
                info.probe_error = Some("no audio output port".to_string());
            }
        }
        Err(e) => info.probe_error = Some(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn macos_paths() {
        let p = standard_paths(Os::Mac, &env(&[("HOME", "/Users/me")]));
        assert_eq!(
            p,
            [
                PathBuf::from("/Users/me/Library/Audio/Plug-Ins/CLAP"),
                PathBuf::from("/Library/Audio/Plug-Ins/CLAP"),
            ]
        );
    }

    #[test]
    fn windows_paths() {
        let p = standard_paths(
            Os::Windows,
            &env(&[
                ("COMMONPROGRAMFILES", "C:\\Program Files\\Common Files"),
                ("LOCALAPPDATA", "C:\\Users\\me\\AppData\\Local"),
            ]),
        );
        assert_eq!(p.len(), 2);
        assert!(p[0].ends_with("CLAP"));
        assert!(p[0].starts_with("C:\\Program Files\\Common Files"));
        assert!(p[1].ends_with("Programs/Common/CLAP") || p[1].ends_with("Programs\\Common\\CLAP"));
    }

    #[test]
    fn linux_paths_and_clap_path() {
        let p = standard_paths(
            Os::Other,
            &env(&[
                ("HOME", "/home/me"),
                ("CLAP_PATH", "/opt/a:/opt/b:/usr/lib/clap"),
            ]),
        );
        let expected = if cfg!(windows) {
            // `split_paths` uses ';' on Windows, so the colon list stays whole.
            vec![
                PathBuf::from("/opt/a:/opt/b:/usr/lib/clap"),
                PathBuf::from("/home/me/.clap"),
                PathBuf::from("/usr/lib/clap"),
            ]
        } else {
            vec![
                PathBuf::from("/opt/a"),
                PathBuf::from("/opt/b"),
                PathBuf::from("/usr/lib/clap"),
                PathBuf::from("/home/me/.clap"),
            ]
        };
        assert_eq!(p, expected, "CLAP_PATH first, duplicates removed");
    }

    #[test]
    fn missing_environment_still_yields_system_paths() {
        let p = standard_paths(Os::Other, &env(&[]));
        assert_eq!(p, [PathBuf::from("/usr/lib/clap")]);
    }

    #[test]
    fn candidates_are_found_recursively_and_bundles_are_opaque() {
        let root = std::env::temp_dir().join(format!("peel-clap-cands-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("vendor/deep")).unwrap();
        std::fs::create_dir_all(root.join("Bundle.clap/Contents/MacOS")).unwrap();
        std::fs::write(root.join("Bundle.clap/Contents/MacOS/inner.clap"), b"x").unwrap();
        std::fs::write(root.join("vendor/deep/Synth.CLAP"), b"x").unwrap();
        std::fs::write(root.join("vendor/readme.txt"), b"x").unwrap();
        std::fs::write(root.join("top.clap"), b"x").unwrap();

        let found = candidate_files(std::slice::from_ref(&root));
        let names: Vec<String> = found
            .iter()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, ["Bundle.clap", "top.clap", "vendor/deep/Synth.CLAP"]);

        // A direct file path is accepted as its own candidate.
        let direct = candidate_files(&[root.join("top.clap")]);
        assert_eq!(direct, [root.join("top.clap")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_that_is_not_a_plugin_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("peel-clap-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("garbage.clap");
        std::fs::write(&bad, b"this is not a shared library").unwrap();
        let err = scan_file(&bad).expect_err("must fail");
        assert!(matches!(err, ClapError::Load { .. }), "{err}");
        let report = scan_paths(std::slice::from_ref(&dir));
        assert!(report.plugins.is_empty());
        assert_eq!(report.failures.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
