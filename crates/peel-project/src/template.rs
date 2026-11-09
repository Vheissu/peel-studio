//! A library of the user's own project templates.
//!
//! Each template is an ordinary saved project in the library folder,
//! named after the template:
//!
//! ```text
//! <library>/Night drive.peelproj
//! <library>/Night drive.peeldata/objects/...
//! <library>/Night drive.peeldata/media/...
//! ```
//!
//! A template holds all of its media. Managed media is linked or copied
//! from the source project, and external files are copied in as managed
//! media, so the template keeps working if the source project or the
//! external files go away.
//!
//! [`save_template`] builds the template in a hidden staging folder
//! inside the library, loads it back to check it, then moves the data
//! folder and finally the root file into place. The root file goes last,
//! so a template only shows up once it is complete. An existing template
//! is never overwritten.

use std::fs;
use std::path::{Path, PathBuf};

use peel_document::{Id, Project};

use crate::error::{ProjectError, io_at};
use crate::load::{LoadedProject, load, read_manifest};
use crate::media::{
    data_dir_for, import_asset, link_media, managed_segments, missing_media, resolve_asset_path,
};
use crate::save::{SaveOptions, save_with};
use crate::util::{create_dir_all, join_segments, sync_dir};

/// Longest template name, in characters.
pub const MAX_TEMPLATE_NAME_CHARS: usize = 64;

const EXTENSION: &str = "peelproj";
const STAGING_PREFIX: &str = ".staging-";

/// A template found in the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateEntry {
    /// The name shown to the user (the file name without extension).
    pub name: String,
    /// The template's root `.peelproj` file.
    pub path: PathBuf,
}

/// Why a template could not be saved.
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("Enter a name")]
    EmptyName,
    #[error("Use {MAX_TEMPLATE_NAME_CHARS} characters or fewer")]
    NameTooLong,
    #[error("Names can't use {0}")]
    BadCharacter(String),
    #[error("Names can't start with a dot")]
    LeadingDot,
    #[error("Names can't end with a dot or a space")]
    TrailingDot,
    #[error("{0} is reserved by the system")]
    Reserved(String),
    #[error("A template named {0} already exists")]
    Exists(String),
    #[error("Missing audio: {}. Restore or remove it, then try again", .0.join(", "))]
    MissingMedia(Vec<String>),
    #[error(
        "This template is missing audio: {}. Restore the files in its .peeldata folder or save the template again",
        .0.join(", ")
    )]
    IncompleteTemplate(Vec<String>),
    #[error("{0}")]
    Project(#[from] ProjectError),
}

/// Checks a template name and returns it trimmed. Names become file names,
/// so path separators, characters Windows refuses and reserved device
/// names are rejected; `..` and other dot names cannot appear.
pub fn template_name(raw: &str) -> std::result::Result<String, TemplateError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(TemplateError::EmptyName);
    }
    if name.chars().count() > MAX_TEMPLATE_NAME_CHARS {
        return Err(TemplateError::NameTooLong);
    }
    if let Some(c) = name.chars().find(|c| {
        c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        let shown = if c.is_control() {
            "control characters".to_string()
        } else {
            c.to_string()
        };
        return Err(TemplateError::BadCharacter(shown));
    }
    if name.starts_with('.') {
        return Err(TemplateError::LeadingDot);
    }
    if name.ends_with('.') {
        return Err(TemplateError::TrailingDot);
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let upper = stem.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err(TemplateError::Reserved(name.to_string()));
    }
    Ok(name.to_string())
}

/// Templates in `dir`, sorted by name (ignoring case). Hidden files,
/// folders and roots that do not read as a project manifest are skipped;
/// a missing folder is an empty library. Only the small root manifests
/// are read.
pub fn list_templates(dir: &Path) -> Vec<TemplateEntry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<TemplateEntry> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(&format!(".{EXTENSION}"))?;
            if stem.is_empty() || stem.starts_with('.') || !e.file_type().ok()?.is_file() {
                return None;
            }
            read_manifest(&path).ok()?;
            Some(TemplateEntry {
                name: stem.to_string(),
                path,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// Saves `project` to the library in `dir` as a template called `name`
/// and returns its root file. `media_source` is the project file whose
/// data folder holds the project's managed media (its own file, or the
/// template it started from); external files are read where they are.
///
/// Nothing in `project` or at `media_source` changes. The stored copy gets
/// a fresh project id and the template's name as its title.
pub fn save_template(
    project: &Project,
    media_source: Option<&Path>,
    dir: &Path,
    name: &str,
) -> std::result::Result<PathBuf, TemplateError> {
    let name = template_name(name)?;
    create_dir_all(dir)?;
    let root = dir.join(format!("{name}.{EXTENSION}"));
    let data = data_dir_for(&root);
    ensure_free(dir, &name, &root, &data)?;

    // Every file the template needs must be there before anything is written.
    let mut missing = Vec::new();
    let mut external = Vec::new();
    for asset in &project.assets {
        let source = match (asset.managed, media_source) {
            (true, Some(src)) => match managed_segments(&asset.path) {
                Ok(segments) => Some(join_segments(&data_dir_for(src), &segments)),
                Err(_) => None,
            },
            (true, None) => None,
            (false, _) => Some(external_path(media_source, &asset.path)),
        };
        match source {
            Some(file) if file.is_file() => {
                if !asset.managed {
                    external.push((asset.id, file));
                }
            }
            _ => missing.push(asset.name.clone()),
        }
    }
    if !missing.is_empty() {
        return Err(TemplateError::MissingMedia(missing));
    }

    let staging = dir.join(format!("{STAGING_PREFIX}{}", Id::new()));
    let result = stage(project, media_source, &staging, &name, &external)
        .and_then(|staged| commit(&staged, &root, &data, &name));
    let _ = fs::remove_dir_all(&staging);
    result?;
    Ok(root)
}

/// Loads a template as a new project: a fresh id and the title Untitled,
/// so saving it asks for a new file. The template itself is not changed.
///
/// A template holds all of its audio, so any file it names that is not
/// there (managed or not) is an error rather than a load warning: starting
/// from it would give a project that silently lacks sound.
pub fn load_template(path: &Path) -> std::result::Result<LoadedProject, TemplateError> {
    let mut loaded = load(path)?;
    let missing: Vec<String> = loaded
        .project
        .assets
        .iter()
        .filter(|a| !resolve_asset_path(path, a).is_file())
        .map(|a| a.name.clone())
        .collect();
    if !missing.is_empty() {
        return Err(TemplateError::IncompleteTemplate(missing));
    }
    loaded.project.id = Id::new();
    loaded.project.meta.title = "Untitled".to_string();
    Ok(loaded)
}

/// Where an external asset path points, read the way a session plays it:
/// absolute as is, relative beside the project file when there is one.
fn external_path(project_file: Option<&Path>, path: &str) -> PathBuf {
    let p = Path::new(path);
    match project_file.and_then(Path::parent) {
        Some(dir) if !p.is_absolute() => dir.join(p),
        _ => p.to_path_buf(),
    }
}

/// Refuses a name already in use, ignoring case (the library may live on
/// a case-insensitive disk). Any root or data folder of that name counts,
/// readable or not: a data folder without a root may be another save in
/// its commit window or something worth recovering, so it is never
/// removed here.
fn ensure_free(
    dir: &Path,
    name: &str,
    root: &Path,
    data: &Path,
) -> std::result::Result<(), TemplateError> {
    let wanted = name.to_lowercase();
    let mut taken = root.exists() || data.exists();
    if let Ok(read) = fs::read_dir(dir) {
        taken |= read.filter_map(|e| e.ok()).any(|e| {
            let file = e.file_name().to_string_lossy().to_lowercase();
            [EXTENSION, "peeldata"]
                .iter()
                .any(|ext| file.strip_suffix(&format!(".{ext}")) == Some(wanted.as_str()))
        });
    }
    if taken {
        return Err(TemplateError::Exists(name.to_string()));
    }
    Ok(())
}

/// Writes the template into `staging` and checks it loads complete.
/// Returns the staged root.
fn stage(
    project: &Project,
    media_source: Option<&Path>,
    staging: &Path,
    name: &str,
    external: &[(Id, PathBuf)],
) -> std::result::Result<PathBuf, TemplateError> {
    create_dir_all(staging)?;
    let root = staging.join(format!("{name}.{EXTENSION}"));
    let mut copy = project.clone();
    copy.id = Id::new();
    copy.meta.title = name.to_string();
    if let Some(src) = media_source {
        link_media(&copy, &data_dir_for(src), &data_dir_for(&root))?;
    }
    for (id, file) in external {
        let managed = import_asset(&root, file)?;
        if let Some(asset) = copy.assets.iter_mut().find(|a| a.id == *id) {
            asset.hash = managed.hash;
            asset.path = managed.path;
            asset.managed = true;
        }
    }
    let opts = SaveOptions {
        history_limit: 0,
        ..SaveOptions::default()
    };
    save_with(&copy, &root, &opts)?;
    let loaded = load(&root)?;
    let missing = missing_media(&loaded.project, &data_dir_for(&root));
    if !missing.is_empty() {
        return Err(TemplateError::MissingMedia(missing));
    }
    Ok(root)
}

/// Moves a staged template into the library: data folder first, root last.
fn commit(
    staged: &Path,
    root: &Path,
    data: &Path,
    name: &str,
) -> std::result::Result<(), TemplateError> {
    let staged_data = data_dir_for(staged);
    // Renaming a folder fails when the target holds anything, so another
    // template's data is never replaced.
    if data.exists() {
        return Err(TemplateError::Exists(name.to_string()));
    }
    fs::rename(&staged_data, data).map_err(io_at(data))?;
    // A hard link is the commit: it never replaces an existing file. There
    // is deliberately no rename fallback, which could overwrite a root
    // created in the meantime; a disk without hard links gets the error.
    if let Err(e) = fs::hard_link(staged, root) {
        let _ = fs::rename(data, &staged_data);
        return Err(if e.kind() == std::io::ErrorKind::AlreadyExists {
            TemplateError::Exists(name.to_string())
        } else {
            io_at(root)(e).into()
        });
    }
    let parent = root.parent().unwrap_or(Path::new("."));
    sync_dir(parent)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert_eq!(template_name("  Night drive ").unwrap(), "Night drive");
        assert_eq!(
            template_name("Lo-fi (v2) ünïcode").unwrap(),
            "Lo-fi (v2) ünïcode"
        );
        assert_eq!(template_name("v1.2 kit").unwrap(), "v1.2 kit");
        for bad in [
            "",
            "   ",
            "..",
            ".",
            ".hidden",
            "a/b",
            "../escape",
            "a\\b",
            "C:",
            "why?",
            "tab\there",
            "dot.",
            "CON",
            "nul.txt",
            "com1",
            "LPT9",
        ] {
            assert!(template_name(bad).is_err(), "{bad:?}");
        }
        assert!(template_name(&"x".repeat(MAX_TEMPLATE_NAME_CHARS)).is_ok());
        assert!(matches!(
            template_name(&"x".repeat(MAX_TEMPLATE_NAME_CHARS + 1)),
            Err(TemplateError::NameTooLong)
        ));
        assert!(template_name("Console").is_ok(), "only exact device names");
        assert!(template_name("COMA").is_ok());
    }

    /// A staged template in `lib`'s staging folder, ready to commit.
    fn staged(lib: &Path, name: &str) -> (PathBuf, PathBuf) {
        let staging = lib.join(format!("{STAGING_PREFIX}test"));
        let root = stage(&Project::empty("Blank"), None, &staging, name, &[]).unwrap();
        (staging, root)
    }

    fn temp_lib(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("peel-template-{tag}-{}", Id::new()));
        create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn commit_never_replaces_a_root_made_in_the_meantime() {
        let lib = temp_lib("commit-root");
        let (staging, staged) = staged(&lib, "Kit");
        // Another save placed its root after this one checked the name.
        let root = lib.join("Kit.peelproj");
        fs::write(&root, b"theirs").unwrap();
        let err = commit(&staged, &root, &data_dir_for(&root), "Kit").unwrap_err();
        assert!(matches!(err, TemplateError::Exists(_)), "{err}");
        assert_eq!(fs::read(&root).unwrap(), b"theirs");
        assert!(
            !data_dir_for(&root).exists(),
            "our data went back to staging"
        );
        assert!(data_dir_for(&staged).is_dir());
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir_all(&lib);
    }

    #[test]
    fn commit_never_replaces_data_made_in_the_meantime() {
        let lib = temp_lib("commit-data");
        let (_, staged) = staged(&lib, "Kit");
        let root = lib.join("Kit.peelproj");
        let data = data_dir_for(&root);
        create_dir_all(&data.join("objects")).unwrap();
        fs::write(data.join("objects").join("theirs.json"), b"theirs").unwrap();
        let err = commit(&staged, &root, &data, "Kit").unwrap_err();
        assert!(matches!(err, TemplateError::Exists(_)), "{err}");
        assert_eq!(
            fs::read(data.join("objects").join("theirs.json")).unwrap(),
            b"theirs"
        );
        assert!(!root.exists());
        let _ = fs::remove_dir_all(&lib);
    }
}
