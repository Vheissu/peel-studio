use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What a plugin is for, derived from its CLAP features.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClapRole {
    /// Plays notes and produces audio (`instrument`).
    Instrument,
    /// Processes audio (`audio-effect`).
    AudioEffect,
    /// Transforms notes (`note-effect`). Not hostable as a [`peel_dsp::Device`].
    NoteEffect,
    /// Listens without changing the sound (`analyzer`, `note-detector`).
    Analyzer,
    /// No recognised role feature and no ports to infer one from.
    Other,
}

/// One plugin inside a CLAP file, as found by the scanner.
///
/// A single `.clap` file can hold several plugins; `index` is the position in
/// the file's factory and `id` is the stable identity. Loading goes by `id`,
/// never by `index`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClapPluginInfo {
    /// Stable plugin id, e.g. `com.vendor.synth`.
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub description: String,
    pub url: String,
    /// Raw CLAP feature strings, e.g. `instrument`, `synthesizer`, `stereo`.
    pub features: Vec<String>,
    pub role: ClapRole,
    /// The `.clap` file (the bundle directory on macOS).
    pub path: PathBuf,
    /// Position of the plugin in the file's factory.
    pub index: u32,
    /// CLAP version the plugin was built against, as `major.minor.revision`.
    pub clap_version: String,
    /// The plugin exposes the `clap.gui` extension.
    pub has_gui: bool,
    /// The plugin exposes `clap.state`, so its full state can be saved.
    pub has_state: bool,
    pub audio_inputs: u32,
    pub audio_outputs: u32,
    pub note_inputs: u32,
    pub note_outputs: u32,
    pub param_count: u32,
    /// Set when the scanner could not create the plugin to probe its
    /// capabilities. The descriptor fields above are still valid.
    pub probe_error: Option<String>,
}

impl ClapPluginInfo {
    /// The `DeviceDescriptor::type_id` a loaded instance reports.
    pub fn type_id(&self) -> String {
        format!("clap:{}", self.id)
    }

    pub fn is_instrument(&self) -> bool {
        self.role == ClapRole::Instrument
    }

    pub fn is_effect(&self) -> bool {
        self.role == ClapRole::AudioEffect
    }

    /// True when [`crate::ClapDevice::load`] can host this plugin.
    pub fn is_hostable(&self) -> bool {
        matches!(self.role, ClapRole::Instrument | ClapRole::AudioEffect)
            && self.probe_error.is_none()
    }
}

/// Picks a role from the feature list, falling back to the port layout.
pub(crate) fn classify(features: &[String], audio_inputs: u32, note_inputs: u32) -> ClapRole {
    let has = |f: &str| features.iter().any(|x| x == f);
    if has("instrument") {
        ClapRole::Instrument
    } else if has("audio-effect") {
        ClapRole::AudioEffect
    } else if has("note-effect") {
        ClapRole::NoteEffect
    } else if has("analyzer") || has("note-detector") {
        ClapRole::Analyzer
    } else if audio_inputs > 0 {
        ClapRole::AudioEffect
    } else if note_inputs > 0 {
        ClapRole::Instrument
    } else {
        ClapRole::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn features_decide_the_role() {
        assert_eq!(
            classify(&f(&["instrument", "synthesizer"]), 0, 1),
            ClapRole::Instrument
        );
        assert_eq!(
            classify(&f(&["audio-effect", "reverb"]), 2, 0),
            ClapRole::AudioEffect
        );
        assert_eq!(classify(&f(&["note-effect"]), 0, 1), ClapRole::NoteEffect);
        assert_eq!(classify(&f(&["analyzer"]), 2, 0), ClapRole::Analyzer);
    }

    #[test]
    fn ports_decide_when_features_do_not() {
        assert_eq!(classify(&[], 2, 0), ClapRole::AudioEffect);
        assert_eq!(classify(&[], 0, 1), ClapRole::Instrument);
        assert_eq!(classify(&[], 0, 0), ClapRole::Other);
    }
}
