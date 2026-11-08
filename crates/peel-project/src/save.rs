//! Durable saving.
//!
//! Order of operations, so that a crash at any point leaves a loadable
//! project:
//!
//! 1. Write every new shard (temp file, fsync, rename) and sync `objects/`.
//! 2. Copy the previous root into `history/`.
//! 3. Write the new root to a temp file, fsync it, rename it over the old
//!    root, then sync the directory.
//! 4. Re-read the new root and confirm every shard it names is present.
//! 5. Only then trim old history and delete shards nothing references.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use peel_document::{Project, SCHEMA_VERSION, validate};

use crate::error::{ProjectError, Result, io_at};
use crate::history::{entry_file_name, history_dir, list_history, objects_dir};
use crate::manifest::{MANIFEST_FORMAT, MAX_MANIFEST_BYTES, Manifest, ObjectIndex, ObjectRef};
use crate::media::{asset_path_problems, data_dir_for, link_media, missing_media};
use crate::shard::{Encoded, check_finite, encode_arrangement, encode_core, encode_pattern};
use crate::util::{
    create_dir_all, hash_hex, is_hash, is_temp_name, sync_dir, utc_timestamp, write_atomic,
};

/// How many previous roots are kept for recovery by default.
pub const DEFAULT_HISTORY_LIMIT: usize = 20;

#[derive(Clone, Debug)]
pub struct SaveOptions {
    /// Previous root manifests to keep in `history/`.
    pub history_limit: usize,
    /// Delete shards that neither the new root nor any kept history root
    /// references.
    pub collect_garbage: bool,
    /// Recorded in the manifest. Defaults to this crate's version.
    pub app_version: Option<String>,
}

impl Default for SaveOptions {
    fn default() -> Self {
        SaveOptions {
            history_limit: DEFAULT_HISTORY_LIMIT,
            collect_garbage: true,
            app_version: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveReport {
    pub root: PathBuf,
    pub data_dir: PathBuf,
    pub revision: u64,
    pub shards_written: usize,
    pub shards_reused: usize,
    pub bytes_written: u64,
    /// Shards deleted because nothing references them any more.
    pub shards_collected: usize,
    /// Managed assets whose media file is not in the data directory.
    pub missing_media: Vec<String>,
}

/// Saves `project` to `path` (a `.peelproj` file) with default options.
pub fn save(project: &Project, path: &Path) -> Result<SaveReport> {
    save_with(project, path, &SaveOptions::default())
}

/// Saves to a new location, bringing managed media along from
/// `previous_project_path` (hard links where possible).
pub fn save_as(
    project: &Project,
    previous_project_path: Option<&Path>,
    new_path: &Path,
) -> Result<SaveReport> {
    if let Some(prev) = previous_project_path {
        link_media(project, &data_dir_for(prev), &data_dir_for(new_path))?;
    }
    save(project, new_path)
}

pub fn save_with(project: &Project, path: &Path, opts: &SaveOptions) -> Result<SaveReport> {
    preflight(project)?;

    let core = encode_core(project)?;
    let patterns: Vec<Encoded> = project
        .patterns
        .iter()
        .map(|p| encode_pattern(p))
        .collect::<Result<_>>()?;
    let arrangements: Vec<Encoded> = project
        .arrangements
        .iter()
        .map(|a| encode_arrangement(a))
        .collect::<Result<_>>()?;

    let data_dir = data_dir_for(path);
    let objects = objects_dir(&data_dir);
    let history = history_dir(&data_dir);
    let parent = parent_dir(path);
    create_dir_all(parent)?;
    create_dir_all(&objects)?;
    create_dir_all(&history)?;

    // 1. Shards first. A shard is skipped when a file of the right name and
    // length is already there; load verifies the hash of what it reads.
    let mut written = 0usize;
    let mut reused = 0usize;
    let mut bytes_written = 0u64;
    for shard in std::iter::once(&core).chain(&patterns).chain(&arrangements) {
        let target = objects.join(format!("{}.json", shard.reference.hash));
        let present =
            fs::metadata(&target).is_ok_and(|m| m.is_file() && m.len() == shard.reference.size);
        if present {
            reused += 1;
        } else {
            write_atomic(&target, &shard.bytes)?;
            written += 1;
            bytes_written += shard.reference.size;
        }
    }
    sync_dir(&objects)?;

    // 2. The new root.
    let manifest = Manifest {
        format: MANIFEST_FORMAT.to_string(),
        schema: project.schema,
        project_id: project.id,
        revision: project.revision,
        title: project.meta.title.clone(),
        app_version: opts
            .app_version
            .clone()
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
        saved_at: utc_timestamp(SystemTime::now()),
        required_capabilities: required_capabilities(project),
        data_dir: data_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        objects: ObjectIndex {
            core: core.reference.clone(),
            patterns: patterns.iter().map(|e| e.reference.clone()).collect(),
            arrangements: arrangements.iter().map(|e| e.reference.clone()).collect(),
        },
    };
    let new_bytes = manifest.to_bytes();

    // 3. Keep the previous root in history; refuse to overwrite a project
    // written by a newer schema.
    archive_previous_root(path, &data_dir, &new_bytes, opts.history_limit)?;
    sync_dir(&history)?;

    // 4. Atomically replace the root.
    write_atomic(path, &new_bytes)?;
    sync_dir(parent)?;

    // 5. Verify what is now on disk.
    verify_root(path, &data_dir)?;

    // 6. Trim.
    prune_history(&data_dir, opts.history_limit);
    let collected = if opts.collect_garbage {
        collect(path, false)?.removed.len()
    } else {
        0
    };

    Ok(SaveReport {
        root: path.to_path_buf(),
        data_dir: data_dir.clone(),
        revision: project.revision,
        shards_written: written,
        shards_reused: reused,
        bytes_written,
        shards_collected: collected,
        missing_media: missing_media(project, &data_dir),
    })
}

fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Refuses to write a project that would not load back.
fn preflight(project: &Project) -> Result<()> {
    if project.schema == 0 || project.schema > SCHEMA_VERSION {
        return Err(ProjectError::Unsavable(format!(
            "project schema {} is not one this version can write",
            project.schema
        )));
    }
    let mut errors: Vec<String> = validate(project)
        .into_iter()
        .filter(|i| i.is_error())
        .map(|i| i.message)
        .collect();
    errors.extend(asset_path_problems(project));
    if !errors.is_empty() {
        return Err(ProjectError::Invalid(errors));
    }
    check_finite(project)
}

/// Sorted, de-duplicated device type ids used by channels and inserts.
pub fn required_capabilities(project: &Project) -> Vec<String> {
    let mut ids: Vec<String> = project
        .channels
        .iter()
        .map(|c| c.device.type_id.clone())
        .chain(
            project
                .mixer
                .tracks
                .iter()
                .flat_map(|t| t.inserts.iter().map(|d| d.type_id.clone())),
        )
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn archive_previous_root(
    root: &Path,
    data_dir: &Path,
    new_bytes: &[u8],
    limit: usize,
) -> Result<()> {
    let old = match fs::read(root) {
        Ok(b) if b.len() as u64 <= MAX_MANIFEST_BYTES => b,
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_at(root)(e)),
    };
    match Manifest::parse(&old) {
        Ok(_) => {}
        Err(e @ ProjectError::FutureSchema { .. }) => return Err(e),
        // A corrupt or foreign previous root is not worth keeping.
        Err(_) => return Ok(()),
    }
    if limit == 0 || old == new_bytes {
        return Ok(());
    }
    let old_hash = hash_hex(&old);
    let existing = list_history(data_dir);
    if existing.last().is_some_and(|e| e.hash16 == old_hash[..16]) {
        return Ok(());
    }
    let seq = existing.last().map_or(1, |e| e.seq + 1);
    let target = history_dir(data_dir).join(entry_file_name(seq, &old_hash));
    write_atomic(&target, &old)
}

fn verify_root(root: &Path, data_dir: &Path) -> Result<()> {
    let bytes = fs::read(root).map_err(io_at(root))?;
    let manifest = Manifest::parse(&bytes)?;
    let objects = objects_dir(data_dir);
    for obj in manifest.objects() {
        let file = objects.join(format!("{}.json", obj.hash));
        match fs::metadata(&file) {
            Ok(m) if m.len() == obj.size => {}
            Ok(_) => {
                return Err(ProjectError::HashMismatch {
                    hash: obj.hash.clone(),
                    actual: "a file of a different length".into(),
                });
            }
            Err(_) => {
                return Err(ProjectError::MissingObject {
                    hash: obj.hash.clone(),
                });
            }
        }
    }
    Ok(())
}

fn prune_history(data_dir: &Path, limit: usize) {
    let entries = list_history(data_dir);
    if entries.len() > limit {
        for old in &entries[..entries.len() - limit] {
            let _ = fs::remove_file(&old.path);
        }
    }
}

/// Result of a garbage collection pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Shard files (or leftover temp files) removed, or that would be.
    pub removed: Vec<PathBuf>,
    /// Shards still referenced by the root or a kept history root.
    pub retained: usize,
}

/// Removes shards that neither the current root nor any history root
/// references, plus leftover temp files from interrupted saves. With
/// `dry_run` nothing is deleted and the report lists what would be.
///
/// Media is never touched. If the root or any history root cannot be read,
/// nothing is removed: its references are unknown.
pub fn collect_garbage(project_path: &Path, dry_run: bool) -> Result<GcReport> {
    collect(project_path, dry_run).map(|mut r| {
        r.removed.sort();
        r
    })
}

fn collect(root: &Path, dry_run: bool) -> Result<GcReport> {
    let data_dir = data_dir_for(root);
    let mut referenced: HashSet<String> = HashSet::new();
    let mut manifests: Vec<PathBuf> = list_history(&data_dir)
        .into_iter()
        .map(|e| e.path)
        .collect();
    if root.exists() {
        manifests.push(root.to_path_buf());
    }
    for file in &manifests {
        let bytes = match fs::read(file) {
            Ok(b) => b,
            Err(_) => return Ok(GcReport::default()),
        };
        match Manifest::parse(&bytes) {
            Ok(m) => referenced.extend(m.objects().map(|o: &ObjectRef| o.hash.clone())),
            Err(_) => return Ok(GcReport::default()),
        }
    }
    if manifests.is_empty() {
        return Ok(GcReport::default());
    }

    let mut report = GcReport::default();
    let objects = objects_dir(&data_dir);
    let Ok(read) = fs::read_dir(&objects) else {
        return Ok(report);
    };
    for entry in read.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let doomed = if is_temp_name(&name) {
            true
        } else if let Some(hash) = name.strip_suffix(".json").filter(|h| is_hash(h)) {
            if referenced.contains(hash) {
                report.retained += 1;
                false
            } else {
                true
            }
        } else {
            false
        };
        if doomed {
            if !dry_run {
                fs::remove_file(entry.path()).map_err(io_at(&entry.path()))?;
            }
            report.removed.push(entry.path());
        }
    }
    Ok(report)
}
