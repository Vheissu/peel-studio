//! Loading: read the root, verify every shard against its hash, rebuild the
//! [`Project`] and validate it. A corrupt latest root falls back to the
//! newest history root that loads cleanly.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use peel_document::{FORMAT, Project, validate};

use crate::error::{ProjectError, Result, io_at};
use crate::history::{list_history, objects_dir};
use crate::manifest::{MAX_MANIFEST_BYTES, Manifest, ObjectRef};
use crate::media::{asset_path_problems, data_dir_for, missing_media};
use crate::shard::{
    ArrangementIn, CoreIn, KIND_ARRANGEMENT, KIND_CORE, KIND_PATTERN, PatternIn, decode,
    unknown_field_warnings,
};
use crate::util::{hash_hex, is_hash};

/// Largest single shard that will be read into memory.
pub(crate) const MAX_SHARD_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug)]
pub struct LoadedProject {
    pub project: Project,
    /// Validation warnings, fields this version dropped, missing media and
    /// recovery notes.
    pub warnings: Vec<String>,
    /// True when the latest root could not be read and an older saved version
    /// was loaded instead. Saving again replaces the damaged root.
    pub recovered_from_history: bool,
}

/// Reads only the root manifest. Useful to check `required_capabilities`
/// against the device registry before loading.
pub fn read_manifest(path: &Path) -> Result<Manifest> {
    Manifest::parse(&read_root_bytes(path)?)
}

/// Loads the project whose root manifest is at `path`.
pub fn load(path: &Path) -> Result<LoadedProject> {
    let data_dir = data_dir_for(path);
    let latest = match load_root(path, &data_dir) {
        Ok((project, warnings, _)) => {
            return Ok(LoadedProject {
                project,
                warnings,
                recovered_from_history: false,
            });
        }
        Err(e) if e.is_recoverable() => e,
        Err(e) => return Err(e),
    };

    for entry in list_history(&data_dir).into_iter().rev() {
        let Ok((project, mut warnings, manifest)) = load_root(&entry.path, &data_dir) else {
            continue;
        };
        warnings.insert(
            0,
            format!(
                "the latest saved version could not be loaded ({latest}); recovered the version \
                 saved at {} (revision {})",
                manifest.saved_at, manifest.revision
            ),
        );
        return Ok(LoadedProject {
            project,
            warnings,
            recovered_from_history: true,
        });
    }
    Err(latest)
}

pub(crate) fn read_root_bytes(path: &Path) -> Result<Vec<u8>> {
    let meta = fs::metadata(path).map_err(io_at(path))?;
    if meta.len() > MAX_MANIFEST_BYTES {
        return Err(ProjectError::NotAProject(format!(
            "{} is too large to be a project root",
            path.display()
        )));
    }
    fs::read(path).map_err(io_at(path))
}

/// Reads a shard, checking its length and hash.
pub(crate) fn read_object(data_dir: &Path, obj: &ObjectRef) -> Result<Vec<u8>> {
    if !is_hash(&obj.hash) {
        return Err(ProjectError::malformed(
            "object reference",
            format!("{:?} is not a hash", obj.hash),
        ));
    }
    let file = objects_dir(data_dir).join(format!("{}.json", obj.hash));
    let meta = match fs::metadata(&file) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ProjectError::MissingObject {
                hash: obj.hash.clone(),
            });
        }
        Err(e) => return Err(io_at(&file)(e)),
    };
    if meta.len() > MAX_SHARD_BYTES {
        return Err(ProjectError::malformed(
            "project object",
            format!("{} is implausibly large", obj.hash),
        ));
    }
    let bytes = fs::read(&file).map_err(io_at(&file))?;
    let actual = hash_hex(&bytes);
    if actual != obj.hash {
        return Err(ProjectError::HashMismatch {
            hash: obj.hash.clone(),
            actual,
        });
    }
    Ok(bytes)
}

/// Loads the project described by the root manifest file at `root`.
fn load_root(root: &Path, data_dir: &Path) -> Result<(Project, Vec<String>, Manifest)> {
    let bytes = read_root_bytes(root)?;
    let manifest = Manifest::parse(&bytes)?;
    let mut warnings = Vec::new();
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
        warnings.extend(unknown_field_warnings(&value, &manifest, "root manifest"));
    }

    let (core, w): (CoreIn, _) = decode(
        &read_object(data_dir, &manifest.objects.core)?,
        "core object",
        KIND_CORE,
    )?;
    warnings.extend(w);

    let mut patterns = Vec::with_capacity(manifest.objects.patterns.len());
    for obj in &manifest.objects.patterns {
        let (shard, w): (PatternIn, _) =
            decode(&read_object(data_dir, obj)?, "pattern object", KIND_PATTERN)?;
        if shard.pattern.id != obj.id {
            return Err(ProjectError::malformed(
                "pattern object",
                format!(
                    "{} holds pattern {} but is indexed as {}",
                    obj.hash, shard.pattern.id, obj.id
                ),
            ));
        }
        warnings.extend(w);
        patterns.push(Arc::new(shard.pattern));
    }

    let mut arrangements = Vec::with_capacity(manifest.objects.arrangements.len());
    for obj in &manifest.objects.arrangements {
        let (shard, w): (ArrangementIn, _) = decode(
            &read_object(data_dir, obj)?,
            "arrangement object",
            KIND_ARRANGEMENT,
        )?;
        if shard.arrangement.id != obj.id {
            return Err(ProjectError::malformed(
                "arrangement object",
                format!(
                    "{} holds arrangement {} but is indexed as {}",
                    obj.hash, shard.arrangement.id, obj.id
                ),
            ));
        }
        warnings.extend(w);
        arrangements.push(Arc::new(shard.arrangement));
    }

    let project = Project {
        format: FORMAT.to_string(),
        schema: manifest.schema,
        id: manifest.project_id,
        revision: manifest.revision,
        meta: core.meta,
        tempo: core.tempo,
        time_signature: core.time_signature,
        swing: core.swing,
        seed: core.seed,
        channels: core.channels,
        patterns,
        arrangements,
        active_arrangement: core.active_arrangement,
        mixer: core.mixer,
        assets: core.assets,
        extensions: core.extensions,
    };

    let mut errors = asset_path_problems(&project);
    for issue in validate(&project) {
        if issue.is_error() {
            errors.push(issue.message);
        } else {
            warnings.push(issue.message);
        }
    }
    if !errors.is_empty() {
        return Err(ProjectError::Invalid(errors));
    }
    for name in missing_media(&project, data_dir) {
        warnings.push(format!(
            "managed media for {name} is missing from the project"
        ));
    }
    Ok((project, warnings, manifest))
}
