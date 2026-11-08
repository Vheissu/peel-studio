//! Scanning in a child process.
//!
//! The application runs [`scan_file_isolated`] (usually through
//! [`scan_isolated`]) instead of [`crate::scan_file`]. Each plugin file gets
//! its own short-lived `peel-clap-scan` process, so a plugin that crashes,
//! aborts, hangs or prints garbage while loading costs one failed file in the
//! report and nothing else.
//!
//! Protocol: the worker is started with the plugin path as its only argument
//! and writes one line, `PEEL-CLAP-SCAN <json>`, to stdout. Other output is
//! ignored (plugins like to print). Exit status 0 with a result line is
//! success; anything else is a crash.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::ClapError;
use crate::info::ClapPluginInfo;
use crate::scan::{ScanFailure, ScanReport, candidate_files, scan_file};

const RESULT_PREFIX: &str = "PEEL-CLAP-SCAN ";

#[derive(Serialize, Deserialize)]
enum WorkerMessage {
    Plugins(Vec<ClapPluginInfo>),
    Error(String),
}

/// The body of the `peel-clap-scan` binary: scan the one path in `args` and
/// print the result line. Kept in the library so tests and the binary share
/// it.
pub fn run_scan_worker(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    let (Some(path), None) = (args.next(), args.next()) else {
        eprintln!("usage: peel-clap-scan <file.clap>");
        return ExitCode::from(64);
    };
    let message = match scan_file(Path::new(&path)) {
        Ok(plugins) => WorkerMessage::Plugins(plugins),
        Err(e) => WorkerMessage::Error(e.to_string()),
    };
    match serde_json::to_string(&message) {
        Ok(json) => {
            println!("{RESULT_PREFIX}{json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cannot encode scan result: {e}");
            ExitCode::from(70)
        }
    }
}

/// Looks for `peel-clap-scan` next to the running executable (and one level
/// up, which covers test binaries in `target/<profile>/deps`).
pub fn default_scanner_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = format!("peel-clap-scan{}", std::env::consts::EXE_SUFFIX);
    let dir = exe.parent()?;
    [dir.join(&name), dir.parent()?.join(&name)]
        .into_iter()
        .find(|p| p.is_file())
}

fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "terminated".to_string(),
    }
}

/// Scans one file in a child process, killing it after `timeout`.
///
/// `scanner` is the `peel-clap-scan` executable.
pub fn scan_file_isolated(
    scanner: &Path,
    file: &Path,
    timeout: Duration,
) -> Result<Vec<ClapPluginInfo>, ClapError> {
    let mut child = Command::new(scanner)
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ClapError::Scanner(format!("cannot start {}: {e}", scanner.display())))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ClapError::Scanner("no stdout pipe".into()))?;
    // Read on a thread so a chatty plugin cannot fill the pipe and stall.
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // The reader ends when the pipe closes; do not wait for it in
                // case the plugin left a grandchild holding the pipe open.
                return Err(ClapError::ScannerTimedOut(file.to_path_buf()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ClapError::Scanner(e.to_string()));
            }
        }
    };
    if !status.success() {
        return Err(ClapError::ScannerCrashed {
            path: file.to_path_buf(),
            status: describe(status),
        });
    }
    let output = reader
        .join()
        .map_err(|_| ClapError::Scanner("reader thread panicked".into()))?;
    let text = String::from_utf8_lossy(&output);
    let json = text
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(RESULT_PREFIX))
        .ok_or_else(|| ClapError::Scanner("the scanner printed no result".into()))?;
    match serde_json::from_str::<WorkerMessage>(json)
        .map_err(|e| ClapError::Scanner(format!("unreadable scanner result: {e}")))?
    {
        WorkerMessage::Plugins(plugins) => Ok(plugins),
        WorkerMessage::Error(message) => Err(ClapError::Scanner(message)),
    }
}

/// Scans every `.clap` under `dirs`, one child process per file. Failures
/// (crash, timeout, unloadable file) are collected in the report; they never
/// stop the scan.
pub fn scan_isolated(scanner: &Path, dirs: &[PathBuf], timeout: Duration) -> ScanReport {
    let mut report = ScanReport::default();
    for file in candidate_files(dirs) {
        match scan_file_isolated(scanner, &file, timeout) {
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
