//! Out-of-process CLAP scanner: `peel-clap-scan <file.clap>`.
//!
//! The application starts one of these per plugin file so a plugin that
//! crashes or hangs while loading cannot take the application down. See
//! `peel_host_clap::scan_file_isolated`.

use std::process::ExitCode;

fn main() -> ExitCode {
    peel_host_clap::run_scan_worker(std::env::args_os().skip(1))
}
