//! Managed media: audio files copied into `Song.peeldata/media/`, named by
//! the hash of their bytes.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use peel_document::{AudioAsset, Id, Project};

use crate::error::{ProjectError, Result, io_at};
use crate::util::{
    copy_hashed, create_dir_all, join_segments, safe_relative_segments, sync_dir, temp_sibling,
};

/// What a decoder learned about a file. `import_asset` never decodes audio
/// itself; the caller may supply this through a probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioProbe {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
}

/// `Song.peelproj` -> `Song.peeldata`, beside the root file.
pub fn data_dir_for(project_path: &Path) -> PathBuf {
    let stem = project_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "project".to_string());
    project_path.with_file_name(format!("{stem}.peeldata"))
}

/// Copies `source` into the project's managed media directory and returns the
/// asset record. The file is named `<blake3>.<extension>`; importing the same
/// bytes twice stores one file. Audio is not decoded, so `sample_rate`,
/// `channels` and `frames` are 0 (see [`import_asset_with`]).
///
/// `project_path` is the `.peelproj` path; the project need not be saved yet.
pub fn import_asset(project_path: &Path, source: &Path) -> Result<AudioAsset> {
    import_asset_with(project_path, source, |_| None)
}

/// Like [`import_asset`], filling the audio properties from `probe`, which is
/// called with the path of the managed copy.
pub fn import_asset_with(
    project_path: &Path,
    source: &Path,
    probe: impl FnOnce(&Path) -> Option<AudioProbe>,
) -> Result<AudioAsset> {
    let media_dir = data_dir_for(project_path).join("media");
    create_dir_all(&media_dir)?;

    let mut input = File::open(source).map_err(io_at(source))?;
    if !input.metadata().map_err(io_at(source))?.is_file() {
        return Err(ProjectError::malformed(
            "audio import",
            format!("{} is not a file", source.display()),
        ));
    }

    // Copy to a temp file while hashing, then name the result by its hash.
    let tmp = temp_sibling(&media_dir.join("import"));
    let result = (|| -> Result<(String, u64)> {
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(io_at(&tmp))?;
        let (len, hash) = copy_hashed(&mut input, &mut out, u64::MAX).map_err(io_at(source))?;
        out.sync_all().map_err(io_at(&tmp))?;
        Ok((hash, len))
    })();
    let (hash, len) = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };

    let ext = managed_extension(source);
    let file_name = if ext.is_empty() {
        hash.clone()
    } else {
        format!("{hash}.{ext}")
    };
    let dest = media_dir.join(&file_name);
    let intact = fs::metadata(&dest).is_ok_and(|m| m.len() == len);
    if intact {
        let _ = fs::remove_file(&tmp);
    } else if let Err(e) = fs::rename(&tmp, &dest) {
        let _ = fs::remove_file(&tmp);
        return Err(io_at(&dest)(e));
    }
    sync_dir(&media_dir)?;

    let probed = probe(&dest);
    Ok(AudioAsset {
        id: Id::new(),
        hash,
        name: source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_name.clone()),
        path: format!("media/{file_name}"),
        managed: true,
        sample_rate: probed.map_or(0, |p| p.sample_rate),
        channels: probed.map_or(0, |p| p.channels),
        frames: probed.map_or(0, |p| p.frames),
    })
}

/// Lowercase alphanumeric extension of at most 8 characters, or empty.
fn managed_extension(source: &Path) -> String {
    source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or_default()
}

/// Where an asset's file lives on disk for the project at `project_path`.
///
/// Managed assets resolve inside the data directory; external ones are
/// absolute, or relative to the folder holding the project file. A managed
/// path that would leave the data directory (a hand-edited or hostile
/// project) resolves to a path that does not exist.
pub fn resolve_asset_path(project_path: &Path, asset: &AudioAsset) -> PathBuf {
    if asset.managed {
        let data_dir = data_dir_for(project_path);
        match managed_segments(&asset.path) {
            Ok(segments) => join_segments(&data_dir, &segments),
            Err(_) => data_dir.join("media").join(".invalid-asset-path"),
        }
    } else {
        let p = Path::new(&asset.path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            project_path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(p)
        }
    }
}

/// Segments of a managed asset path, which must be a safe relative path
/// inside the data directory.
pub(crate) fn managed_segments(path: &str) -> std::result::Result<Vec<&str>, &'static str> {
    safe_relative_segments(path)
}

/// Error messages for managed assets whose paths are unsafe.
pub(crate) fn asset_path_problems(project: &Project) -> Vec<String> {
    project
        .assets
        .iter()
        .filter(|a| a.managed)
        .filter_map(|a| {
            managed_segments(&a.path).err().map(|why| {
                format!(
                    "asset {} has an unsafe managed path {:?}: {why}",
                    a.name, a.path
                )
            })
        })
        .collect()
}

/// Names of managed assets whose media file is absent from `data_dir`.
pub(crate) fn missing_media(project: &Project, data_dir: &Path) -> Vec<String> {
    project
        .assets
        .iter()
        .filter(|a| a.managed)
        .filter_map(|a| {
            let segments = managed_segments(&a.path).ok()?;
            let file = join_segments(data_dir, &segments);
            (!file.is_file()).then(|| a.name.clone())
        })
        .collect()
}

/// Makes the managed media of `project` available in `to_data_dir`, hard
/// linking from `from_data_dir` when possible and copying otherwise. Files
/// already present are left alone; sources that are missing are skipped (the
/// save report lists them).
pub(crate) fn link_media(
    project: &Project,
    from_data_dir: &Path,
    to_data_dir: &Path,
) -> Result<()> {
    if from_data_dir == to_data_dir {
        return Ok(());
    }
    for asset in project.assets.iter().filter(|a| a.managed) {
        let Ok(segments) = managed_segments(&asset.path) else {
            continue;
        };
        let src = join_segments(from_data_dir, &segments);
        let dst = join_segments(to_data_dir, &segments);
        if dst.exists() || !src.is_file() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            create_dir_all(parent)?;
        }
        if fs::hard_link(&src, &dst).is_err() {
            let tmp = temp_sibling(&dst);
            if let Err(e) = fs::copy(&src, &tmp).and_then(|_| fs::rename(&tmp, &dst)) {
                let _ = fs::remove_file(&tmp);
                return Err(io_at(&dst)(e));
            }
        }
    }
    Ok(())
}
