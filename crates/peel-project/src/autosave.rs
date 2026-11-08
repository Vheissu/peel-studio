//! Autosave: the same durable save path, into a private directory per
//! project, never touching the user's own project files.
//!
//! Layout: `<autosave root>/<project id>/autosave.peelproj` plus its
//! `autosave.peeldata/`. Autosave keeps a short history so a crash during an
//! autosave can fall back to the previous one.

use std::fs;
use std::path::{Path, PathBuf};

use peel_document::{Id, Project};

use crate::error::{ProjectError, Result, io_at};
use crate::load::read_manifest;
use crate::media::data_dir_for;
use crate::media::link_media;
use crate::save::{SaveOptions, save_with};

const AUTOSAVE_FILE: &str = "autosave.peelproj";
const AUTOSAVE_HISTORY: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutosaveEntry {
    pub project_id: Id,
    /// Root manifest of the autosave; pass it to [`crate::load`].
    pub path: PathBuf,
    pub title: String,
    pub revision: u64,
    pub saved_at: String,
}

/// Where the autosave for `project_id` lives under `autosave_root`.
pub fn autosave_path(autosave_root: &Path, project_id: Id) -> PathBuf {
    autosave_root
        .join(project_id.to_string())
        .join(AUTOSAVE_FILE)
}

/// Saves `project` into the autosave area and returns the root manifest path.
///
/// Managed media is not copied (see [`autosave_from`]); assets then show as
/// missing media when the autosave is loaded on its own.
pub fn autosave(project: &Project, autosave_root: &Path) -> Result<PathBuf> {
    let path = autosave_path(autosave_root, project.id);
    let opts = SaveOptions {
        history_limit: AUTOSAVE_HISTORY,
        ..SaveOptions::default()
    };
    save_with(project, &path, &opts)?;
    Ok(path)
}

/// Like [`autosave`], and also hard links (or copies) the project's managed
/// media from the saved project at `source_project_path`, so the autosave is
/// complete by itself.
pub fn autosave_from(
    project: &Project,
    autosave_root: &Path,
    source_project_path: &Path,
) -> Result<PathBuf> {
    let path = autosave_path(autosave_root, project.id);
    link_media(
        project,
        &data_dir_for(source_project_path),
        &data_dir_for(&path),
    )?;
    autosave(project, autosave_root)
}

/// Autosaves found under `autosave_root`, most recently saved first.
/// Unreadable ones are skipped. A missing root is an empty list.
pub fn list_autosaves(autosave_root: &Path) -> Result<Vec<AutosaveEntry>> {
    let read = match fs::read_dir(autosave_root) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_at(autosave_root)(e)),
    };
    let mut out = Vec::new();
    for entry in read.filter_map(|e| e.ok()) {
        let Ok(id) = entry.file_name().to_string_lossy().parse::<Id>() else {
            continue;
        };
        let path = entry.path().join(AUTOSAVE_FILE);
        let Ok(manifest) = read_manifest(&path) else {
            continue;
        };
        out.push(AutosaveEntry {
            project_id: id,
            path,
            title: manifest.title,
            revision: manifest.revision,
            saved_at: manifest.saved_at,
        });
    }
    out.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
    Ok(out)
}

/// Deletes the autosave for a project, for instance after a successful save
/// or a clean close.
pub fn discard_autosave(autosave_root: &Path, project_id: Id) -> Result<()> {
    let dir = autosave_root.join(project_id.to_string());
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ProjectError::Io {
            path: dir,
            source: e,
        }),
    }
}
