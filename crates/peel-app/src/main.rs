//! Peel Studio desktop application.

mod browser;
mod dialogs;
mod harmony;
mod inspector;
mod keys;
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
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();
    let open: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);
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
        studio
    });
    if let Err(e) = result {
        eprintln!("Peel Studio could not start: {e}");
        std::process::exit(1);
    }
}
