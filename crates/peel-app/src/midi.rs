//! Standard MIDI File import: opens the file as a new project.

use std::path::Path;

use peel_midi::{ImportOptions, MergeOptions, import_smf};
use peel_session::{PlayMode, Session, template};

/// Imports `path` as a new project. Returns true on success.
pub fn import(session: &mut Session, path: &Path) -> bool {
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "MIDI".into());
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            session.notify(format!("Could not read {}: {e}", path.display()), true);
            return false;
        }
    };
    let imported = match import_smf(&bytes, &ImportOptions::default()) {
        Ok(i) => i,
        Err(e) => {
            session.notify(format!("Could not import {name}: {e}"), true);
            return false;
        }
    };
    let notes = imported.report.total_notes;
    let skipped: u64 = imported.report.unsupported.iter().map(|u| u.count).sum();
    let mut project = template::empty();
    project.patterns.clear();
    project.meta.title = imported
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(name.clone());
    project.mixer.tracks[0]
        .inserts
        .push(template::device("peel.fx.limiter", None));
    if let Err(e) = imported.merge_with_new_mixer_tracks(&mut project, &MergeOptions::new_project())
    {
        session.notify(format!("Could not import {name}: {e}"), true);
        return false;
    }
    session.replace_project(project);
    session.set_mode(PlayMode::Song);
    let extra = if skipped > 0 {
        format!(", {skipped} unsupported events skipped")
    } else {
        String::new()
    };
    session.notify(format!("Imported {notes} notes from {name}{extra}"), false);
    true
}
