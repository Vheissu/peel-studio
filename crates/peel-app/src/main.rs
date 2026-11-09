//! Peel Studio desktop application.

mod analysis_view;
mod audio_io;
mod browser;
mod dialogs;
mod eq_editor;
mod eq_resonance;
mod fx_editor;
mod harmony;
mod inspector;
mod instrument_editor;
mod keys;
mod layout;
mod mastering_ui;
mod midi;
mod mixer;
mod note_tools;
mod perform;
mod piano_roll;
mod playlist;
mod prefs;
mod rack;
mod recorder;
mod rename;
mod script_host;
mod scripts;
mod settings;
mod studio;
mod transport;
mod util;
mod welcome;
mod workspace;

#[cfg(test)]
mod tests;

#[cfg(test)]
use inspector::reaches as inspector_reaches;

use std::path::PathBuf;

use peel_session::{Session, template};
use peel_ui::WindowOptions;

fn main() {
    if peel_extensions::dispatch_worker() {
        return;
    }

    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();
    let mut mcp = false;
    let mut headless = false;
    let mut open: Option<PathBuf> = None;
    for arg in std::env::args_os().skip(1) {
        if arg == "--mcp" {
            mcp = true;
        } else if arg == "--mcp-headless" {
            headless = true;
        } else if arg == "--help" || arg == "-h" {
            eprintln!("Usage: peel-studio [--mcp | --mcp-headless] [project.peelproj]");
            return;
        } else if arg.to_string_lossy().starts_with('-') || open.is_some() {
            eprintln!("Unknown argument: {}", arg.to_string_lossy());
            std::process::exit(2);
        } else {
            open = Some(PathBuf::from(arg));
        }
    }
    if mcp && headless {
        eprintln!("Choose either --mcp or --mcp-headless");
        std::process::exit(2);
    }
    if headless {
        if let Err(error) = peel_mcp::run_headless(open, false) {
            eprintln!("Peel MCP: {error}");
            std::process::exit(1);
        }
        return;
    }
    let prefs = prefs::Prefs::load();
    let mut session = Session::new_with_audio(
        template::showcase(),
        prefs.audio_device.clone(),
        prefs.buffer_frames,
    );
    session.prefer_input_device(prefs.input_device.clone());
    if let Some(c) = prefs.input_channels() {
        session.set_input_channels(c);
    }
    let options = WindowOptions {
        title: "Peel Studio".into(),
        ..Default::default()
    };
    let result = peel_ui::run(options, move |_ui| {
        let mut studio = studio::Studio::new(session);
        if let Some(path) = open {
            studio.open_path(path);
        }
        if mcp {
            studio.enable_mcp();
        }
        studio
    });
    if let Err(e) = result {
        eprintln!("Peel Studio could not start: {e}");
        std::process::exit(1);
    }
}
