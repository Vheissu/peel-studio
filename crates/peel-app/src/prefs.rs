//! User preferences and recent projects, stored as JSON in the platform
//! config directory. Never part of a project.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 12;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub recent: Vec<PathBuf>,
    pub show_welcome: bool,
    pub show_browser: bool,
    pub show_inspector: bool,
    pub show_scripts: bool,
    pub show_editor: bool,
    pub show_help: bool,
    pub audio_device: Option<String>,
    pub buffer_frames: Option<u32>,
    pub octave: i32,
    pub sample_dir: Option<PathBuf>,
    /// Optional virtual environment interpreter for Python actions.
    pub python: Option<PathBuf>,
    /// Audio input to record from (`None` is the system default).
    pub input_device: Option<String>,
    /// Device channels recorded: one (mono) or two (stereo), zero-based.
    pub input_channels: Vec<u16>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            recent: Vec::new(),
            show_welcome: true,
            show_browser: true,
            show_inspector: true,
            show_scripts: false,
            show_editor: true,
            show_help: false,
            audio_device: None,
            buffer_frames: None,
            octave: 4,
            sample_dir: None,
            python: None,
            input_device: None,
            input_channels: Vec::new(),
        }
    }
}

/// `<config>/Peel Studio` (macOS: ~/Library/Application Support).
pub fn app_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Peel Studio")
}

pub fn autosave_dir() -> PathBuf {
    app_dir().join("Autosave")
}

fn prefs_path() -> PathBuf {
    app_dir().join("prefs.json")
}

impl Prefs {
    /// Loads preferences; tests and screenshots use defaults when
    /// `PEEL_NO_PREFS` is set.
    pub fn load() -> Prefs {
        if std::env::var_os("PEEL_NO_PREFS").is_some() || cfg!(test) {
            return Prefs::default();
        }
        std::fs::read(prefs_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if std::env::var_os("PEEL_NO_PREFS").is_some() || cfg!(test) {
            return;
        }
        let path = prefs_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    pub fn add_recent(&mut self, path: &Path) {
        let path = path.to_path_buf();
        self.recent.retain(|p| p != &path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
        self.save();
    }

    /// Recent projects that still exist on disk.
    pub fn existing_recent(&self) -> Vec<PathBuf> {
        self.recent.iter().filter(|p| p.exists()).cloned().collect()
    }
}

impl Prefs {
    /// The saved channel choice as the session's type.
    pub fn input_channels(&self) -> Option<peel_session::InputChannels> {
        match self.input_channels.as_slice() {
            [a] => Some(peel_session::InputChannels::Mono(*a)),
            [a, b] => Some(peel_session::InputChannels::Stereo(*a, *b)),
            _ => None,
        }
    }

    pub fn set_input_channels(&mut self, c: peel_session::InputChannels) {
        self.input_channels = match c {
            peel_session::InputChannels::Mono(a) => vec![a],
            peel_session::InputChannels::Stereo(a, b) => vec![a, b],
        };
    }
}
