//! Peel Studio desktop application.

mod browser;
mod inspector;
mod midi;
mod mixer;
mod piano_roll;
mod playlist;
mod rack;
mod studio;
mod transport;
mod util;
mod workspace;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use peel_session::{Session, template};
use peel_ui::WindowOptions;

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();
    let open: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);
    let session = Session::new(template::beat_and_melody());
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
