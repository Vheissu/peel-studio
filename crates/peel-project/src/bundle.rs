//! Portable `.peelbundle` archives.
//!
//! A bundle is a ZIP holding the root manifest, the shards it references and
//! the managed media, under a fixed layout:
//!
//! ```text
//! project.peelproj
//! project.peeldata/objects/<blake3>.json
//! project.peeldata/media/<file>
//! ```
//!
//! Import treats the archive as hostile. Every entry name must match that
//! layout exactly (which rules out traversal, absolute paths, drive letters,
//! backslashes and odd characters outright), symlinks and special files are
//! refused, duplicate and case-colliding names are refused, sizes and
//! compression ratios are bounded while reading (not trusted from headers),
//! every shard and medium is verified against its hash, and extraction
//! happens in a fresh sibling directory that is renamed into place only after
//! the whole project loads.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::{ProjectError, Result, io_at};
use crate::load::{self, read_object, read_root_bytes};
use crate::manifest::{MAX_MANIFEST_BYTES, Manifest};
use crate::media::{data_dir_for, managed_segments, resolve_asset_path};
use crate::shard::{CoreIn, KIND_CORE, decode};
use crate::util::{
    copy_hashed, create_dir_all, hash_hex, is_hash, join_segments, sync_dir, temp_sibling,
};

const ROOT_ENTRY: &str = "project.peelproj";
const DATA_ENTRY: &str = "project.peeldata";

/// Resource limits applied while reading a bundle.
#[derive(Clone, Debug)]
pub struct BundleLimits {
    pub max_entries: usize,
    /// Total uncompressed bytes across all entries.
    pub max_total_bytes: u64,
    pub max_entry_bytes: u64,
    /// Largest manifest or shard.
    pub max_shard_bytes: u64,
    /// Largest allowed uncompressed:compressed ratio of one entry...
    pub max_ratio: u64,
    /// ...applied only to entries at least this large (tiny entries naturally
    /// have high ratios).
    pub ratio_min_bytes: u64,
}

impl Default for BundleLimits {
    fn default() -> Self {
        BundleLimits {
            max_entries: 200_000,
            max_total_bytes: 8 * 1024 * 1024 * 1024,
            max_entry_bytes: 4 * 1024 * 1024 * 1024,
            max_shard_bytes: 256 * 1024 * 1024,
            max_ratio: 200,
            ratio_min_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleReport {
    /// Root manifest plus shards written.
    pub objects: usize,
    pub media_files: usize,
    /// Uncompressed bytes packed.
    pub bytes: u64,
    /// Assets referenced by path rather than managed; not in the bundle.
    pub external_assets: Vec<String>,
    /// Managed assets whose media file was absent; not in the bundle.
    pub missing_media: Vec<String>,
}

fn zip_err(e: zip::result::ZipError) -> ProjectError {
    ProjectError::Bundle(e.to_string())
}

fn reject(name: &str, reason: impl Into<String>) -> ProjectError {
    ProjectError::UnsafeBundleEntry {
        name: name.to_string(),
        reason: reason.into(),
    }
}

// ---------------------------------------------------------------- export

/// Packs the project at `project_path` into a `.peelbundle`. The archive is
/// written beside the destination, verified in full, and then renamed into
/// place, so `bundle_path` is never left half-written.
pub fn export_bundle(project_path: &Path, bundle_path: &Path) -> Result<BundleReport> {
    let data_dir = data_dir_for(project_path);
    let root_bytes = read_root_bytes(project_path)?;
    let manifest = Manifest::parse(&root_bytes)?;

    let tmp = temp_sibling(bundle_path);
    let built = write_bundle(&tmp, project_path, &data_dir, &root_bytes, &manifest);
    let report = match built {
        Ok(r) => r,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };
    let verified = File::open(&tmp)
        .map_err(io_at(&tmp))
        .and_then(|f| scan(f, &BundleLimits::default(), None));
    if let Err(e) = verified {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, bundle_path) {
        let _ = fs::remove_file(&tmp);
        return Err(io_at(bundle_path)(e));
    }
    if let Some(parent) = bundle_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        sync_dir(parent)?;
    }
    Ok(report)
}

fn write_bundle(
    tmp: &Path,
    project_path: &Path,
    data_dir: &Path,
    root_bytes: &[u8],
    manifest: &Manifest,
) -> Result<BundleReport> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)
        .map_err(io_at(tmp))?;
    let mut zip = ZipWriter::new(BufWriter::new(file));
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let write_err = |e: io::Error| io_at(tmp)(e);
    let mut report = BundleReport::default();

    zip.start_file(ROOT_ENTRY, deflated).map_err(zip_err)?;
    zip.write_all(root_bytes).map_err(write_err)?;
    report.objects += 1;
    report.bytes += root_bytes.len() as u64;

    let mut core_bytes = None;
    for obj in manifest.objects() {
        let bytes = read_object(data_dir, obj)?;
        zip.start_file(format!("{DATA_ENTRY}/objects/{}.json", obj.hash), deflated)
            .map_err(zip_err)?;
        zip.write_all(&bytes).map_err(write_err)?;
        report.objects += 1;
        report.bytes += bytes.len() as u64;
        if obj.hash == manifest.objects.core.hash {
            core_bytes = Some(bytes);
        }
    }
    let core_bytes = core_bytes.expect("core object was iterated");
    let (core, _): (CoreIn, _) = decode(&core_bytes, "core object", KIND_CORE)?;

    let mut packed: HashSet<String> = HashSet::new();
    for asset in &core.assets {
        if !asset.managed {
            report.external_assets.push(asset.name.clone());
            continue;
        }
        let segments = managed_segments(&asset.path).map_err(|why| ProjectError::UnsafePath {
            path: asset.path.clone(),
            reason: why.to_string(),
        })?;
        if segments.len() != 2 || segments[0] != "media" || !is_media_name(segments[1]) {
            return Err(ProjectError::UnsafePath {
                path: asset.path.clone(),
                reason: "managed media must be a plain file directly inside media/".into(),
            });
        }
        let entry_name = format!("{DATA_ENTRY}/{}", asset.path);
        if !packed.insert(entry_name.to_ascii_lowercase()) {
            continue;
        }
        let src = resolve_asset_path(project_path, asset);
        let Ok(mut input) = File::open(&src) else {
            report.missing_media.push(asset.name.clone());
            packed.remove(&entry_name.to_ascii_lowercase());
            continue;
        };
        let len = input.metadata().map_err(io_at(&src))?.len();
        zip.start_file(&entry_name, stored.large_file(len >= u64::from(u32::MAX)))
            .map_err(zip_err)?;
        let (copied, hash) = copy_hashed(&mut input, &mut zip, u64::MAX).map_err(io_at(&src))?;
        if !asset.hash.eq_ignore_ascii_case(&hash) {
            return Err(ProjectError::HashMismatch {
                hash: asset.hash.clone(),
                actual: hash,
            });
        }
        report.media_files += 1;
        report.bytes += copied;
    }

    let writer = zip.finish().map_err(zip_err)?;
    let file = writer
        .into_inner()
        .map_err(|e| io_at(tmp)(e.into_error()))?;
    file.sync_all().map_err(io_at(tmp))?;
    Ok(report)
}

// ---------------------------------------------------------------- import

/// Extracts a bundle into `dest_dir`, which must not exist or must be empty,
/// and returns the path of the extracted `.peelproj` (named after the bundle
/// file). On any failure nothing is left behind.
pub fn import_bundle(bundle_path: &Path, dest_dir: &Path) -> Result<PathBuf> {
    import_bundle_with(bundle_path, dest_dir, &BundleLimits::default())
}

pub fn import_bundle_with(
    bundle_path: &Path,
    dest_dir: &Path,
    limits: &BundleLimits,
) -> Result<PathBuf> {
    match fs::symlink_metadata(dest_dir) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(ProjectError::DestinationNotEmpty(dest_dir.to_path_buf()));
            }
            let empty = fs::read_dir(dest_dir)
                .map_err(io_at(dest_dir))?
                .next()
                .is_none();
            if !empty {
                return Err(ProjectError::DestinationNotEmpty(dest_dir.to_path_buf()));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_at(dest_dir)(e)),
    }

    let stem = bundle_stem(bundle_path);
    let input = File::open(bundle_path).map_err(io_at(bundle_path))?;

    if let Some(parent) = dest_dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        create_dir_all(parent)?;
    }
    let staging = temp_sibling(dest_dir);
    fs::create_dir(&staging).map_err(io_at(&staging))?;

    let outcome = (|| -> Result<()> {
        scan(input, limits, Some((&staging, &stem)))?;
        // The extracted project must load, not just unpack.
        load::load(&staging.join(format!("{stem}.peelproj")))?;
        if dest_dir.exists() {
            fs::remove_dir(dest_dir).map_err(io_at(dest_dir))?;
        }
        fs::rename(&staging, dest_dir).map_err(io_at(dest_dir))
    })();
    match outcome {
        Ok(()) => {
            if let Some(parent) = dest_dir.parent().filter(|p| !p.as_os_str().is_empty()) {
                sync_dir(parent)?;
            }
            Ok(dest_dir.join(format!("{stem}.peelproj")))
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

/// File-system-safe project name derived from the bundle's file name.
fn bundle_stem(bundle_path: &Path) -> String {
    let raw = bundle_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() {
        "Song".to_string()
    } else {
        cleaned
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Root,
    Object(String),
    Media(String),
    Dir,
}

/// Maps an archive entry name to its role, or says why it is not allowed.
fn classify(name: &str) -> std::result::Result<Kind, &'static str> {
    if name.is_empty() {
        return Err("empty name");
    }
    if !name.is_ascii() {
        return Err("name is not plain ASCII");
    }
    if name.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err("name contains control characters");
    }
    if name.contains('\\') {
        return Err("name contains a backslash");
    }
    if name.starts_with('/') {
        return Err("absolute path");
    }
    if name.as_bytes().get(1) == Some(&b':') {
        return Err("drive-letter path");
    }
    let (body, is_dir) = match name.strip_suffix('/') {
        Some(b) => (b, true),
        None => (name, false),
    };
    let parts: Vec<&str> = body.split('/').collect();
    if parts.contains(&"..") {
        return Err("path traversal");
    }
    if parts.iter().any(|p| p.is_empty() || *p == ".") {
        return Err("empty or relative path segment");
    }
    match (parts.as_slice(), is_dir) {
        ([ROOT_ENTRY], false) => Ok(Kind::Root),
        ([DATA_ENTRY], true) | ([DATA_ENTRY, "objects" | "media"], true) => Ok(Kind::Dir),
        ([DATA_ENTRY, "objects", file], false) => match file.strip_suffix(".json") {
            Some(h) if is_hash(h) => Ok(Kind::Object(h.to_string())),
            _ => Err("object name is not a lowercase BLAKE3 hash"),
        },
        ([DATA_ENTRY, "media", file], false) if is_media_name(file) => {
            Ok(Kind::Media((*file).to_string()))
        }
        ([DATA_ENTRY, "media", _], false) => Err("unsafe media file name"),
        _ => Err("not part of the Peel bundle layout"),
    }
}

/// Plain file names for managed media: ASCII letters, digits, `.`, `_`, `-`;
/// no leading or trailing dot, no `..`, no Windows device names.
pub(crate) fn is_media_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 128
        || name.starts_with('.')
        || name.ends_with('.')
        || name.contains("..")
    {
        return false;
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    !reserved
}

struct Planned {
    index: usize,
    kind: Kind,
    size: u64,
}

/// Validates a whole archive and, when `extract` names a staging directory
/// and project stem, unpacks it there. Nothing is written outside `staging`.
fn scan(file: File, limits: &BundleLimits, extract: Option<(&Path, &str)>) -> Result<()> {
    let mut archive = ZipArchive::new(file).map_err(zip_err)?;
    if archive.has_duplicate_names() {
        return Err(ProjectError::Bundle(
            "the archive contains duplicate entries".into(),
        ));
    }
    if archive.len() > limits.max_entries {
        return Err(ProjectError::BundleLimit(format!(
            "{} entries (limit {})",
            archive.len(),
            limits.max_entries
        )));
    }
    // Local headers must agree with the central directory and entries must
    // not overlap, so nothing hides outside what we inspect.
    archive.validate_header_consistency().map_err(zip_err)?;

    // Pass 1: judge every entry from its header alone.
    let mut planned: Vec<Planned> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut declared_total = 0u64;
    let mut roots = 0usize;
    for index in 0..archive.len() {
        let entry = archive.by_index_data(index).map_err(zip_err)?;
        let name = entry
            .name()
            .map_err(|_| reject(&format!("#{index}"), "name is not valid text"))?
            .into_owned();
        let kind = classify(&name).map_err(|why| reject(&name, why))?;
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(reject(&name, "duplicate name (case-insensitive)"));
        }
        if entry.encrypted() {
            return Err(reject(&name, "encrypted entry"));
        }
        if entry.is_symlink() {
            return Err(reject(&name, "symbolic link"));
        }
        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170_000;
            if !matches!(file_type, 0 | 0o100_000 | 0o040_000) {
                return Err(reject(&name, "not a regular file"));
            }
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(reject(&name, "unsupported compression method"));
        }
        let size = entry.size();
        let cap = match kind {
            Kind::Root => limits.max_shard_bytes.min(MAX_MANIFEST_BYTES),
            Kind::Object(_) => limits.max_shard_bytes,
            Kind::Media(_) => limits.max_entry_bytes,
            Kind::Dir => 0,
        };
        if size > cap {
            return Err(ProjectError::BundleLimit(format!(
                "{name} declares {size} bytes (limit {cap})"
            )));
        }
        if size >= limits.ratio_min_bytes
            && size
                > limits
                    .max_ratio
                    .saturating_mul(entry.compressed_size().max(1))
        {
            return Err(ProjectError::BundleLimit(format!(
                "{name} expands {size} bytes from {} (ratio above {}:1)",
                entry.compressed_size(),
                limits.max_ratio
            )));
        }
        declared_total = declared_total.saturating_add(size);
        if declared_total > limits.max_total_bytes {
            return Err(ProjectError::BundleLimit(format!(
                "declared uncompressed size exceeds {} bytes",
                limits.max_total_bytes
            )));
        }
        if kind == Kind::Root {
            roots += 1;
        }
        if kind != Kind::Dir {
            planned.push(Planned { index, kind, size });
        }
    }
    if roots != 1 {
        return Err(ProjectError::Bundle(
            "the archive has no root manifest (project.peelproj)".into(),
        ));
    }

    // Pass 2: the root first, so we know which shards are legitimate.
    let mut actual_total = 0u64;
    let root_pos = planned
        .iter()
        .position(|p| p.kind == Kind::Root)
        .expect("one root");
    let root = planned.remove(root_pos);
    let root_bytes = read_entry(&mut archive, &root, ROOT_ENTRY, limits, &mut actual_total)?;
    let manifest = Manifest::parse(&root_bytes)?;
    let expected: HashMap<&str, u64> = manifest
        .objects()
        .map(|o| (o.hash.as_str(), o.size))
        .collect();
    let core_hash = manifest.objects.core.hash.as_str();
    if let Some((staging, stem)) = extract {
        let dest = staging.join(format!("{stem}.peelproj"));
        write_new(&dest, &root_bytes)?;
        create_dir_all(&staging.join(format!("{stem}.peeldata")).join("objects"))?;
        create_dir_all(&staging.join(format!("{stem}.peeldata")).join("media"))?;
    }

    let mut found_objects: HashSet<String> = HashSet::new();
    let mut media_hashes: HashMap<String, String> = HashMap::new();
    let mut core_bytes: Option<Vec<u8>> = None;
    for plan in &planned {
        match &plan.kind {
            Kind::Object(hash) => {
                let name = format!("{DATA_ENTRY}/objects/{hash}.json");
                let Some(&want_size) = expected.get(hash.as_str()) else {
                    return Err(reject(&name, "object is not referenced by the manifest"));
                };
                let bytes = read_entry(&mut archive, plan, &name, limits, &mut actual_total)?;
                let actual = hash_hex(&bytes);
                if &actual != hash {
                    return Err(ProjectError::HashMismatch {
                        hash: hash.clone(),
                        actual,
                    });
                }
                if bytes.len() as u64 != want_size {
                    return Err(ProjectError::malformed(
                        "bundle object",
                        format!(
                            "{hash} is {} bytes but the manifest says {want_size}",
                            bytes.len()
                        ),
                    ));
                }
                if let Some((staging, stem)) = extract {
                    let data = format!("{stem}.peeldata");
                    let file = format!("{hash}.json");
                    let dest = join_segments(staging, &[data.as_str(), "objects", file.as_str()]);
                    write_new(&dest, &bytes)?;
                }
                if hash == core_hash {
                    core_bytes = Some(bytes);
                }
                found_objects.insert(hash.clone());
            }
            Kind::Media(file) => {
                let name = format!("{DATA_ENTRY}/media/{file}");
                if actual_total.saturating_add(plan.size) > limits.max_total_bytes {
                    return Err(ProjectError::BundleLimit(format!(
                        "uncompressed size exceeds {} bytes",
                        limits.max_total_bytes
                    )));
                }
                let mut zf = archive.by_index(plan.index).map_err(zip_err)?;
                let hash = match extract {
                    Some((staging, stem)) => {
                        let data = format!("{stem}.peeldata");
                        let dest = join_segments(staging, &[data.as_str(), "media", file.as_str()]);
                        let mut out = OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&dest)
                            .map_err(io_at(&dest))?;
                        let (n, hash) = copy_entry(&mut zf, &mut out, plan.size, &name)?;
                        out.sync_all().map_err(io_at(&dest))?;
                        actual_total += n;
                        hash
                    }
                    None => {
                        let (n, hash) = copy_entry(&mut zf, &mut io::sink(), plan.size, &name)?;
                        actual_total += n;
                        hash
                    }
                };
                media_hashes.insert(file.clone(), hash);
            }
            Kind::Root | Kind::Dir => unreachable!("root removed, directories skipped"),
        }
    }

    for obj in manifest.objects() {
        if !found_objects.contains(&obj.hash) {
            return Err(ProjectError::MissingObject {
                hash: obj.hash.clone(),
            });
        }
    }

    // Media must belong to a managed asset and match its recorded hash.
    let core_bytes = core_bytes.expect("core object is among the found objects");
    let (core, _): (CoreIn, _) = decode(&core_bytes, "core object", KIND_CORE)?;
    let mut wanted: HashMap<String, &str> = HashMap::new();
    for asset in core.assets.iter().filter(|a| a.managed) {
        if let Some(file) = asset.path.strip_prefix("media/") {
            wanted.insert(file.to_string(), asset.hash.as_str());
        }
    }
    for (file, actual) in &media_hashes {
        match wanted.get(file) {
            None => {
                return Err(reject(
                    &format!("{DATA_ENTRY}/media/{file}"),
                    "media file is not referenced by any asset",
                ));
            }
            Some(want) if !want.eq_ignore_ascii_case(actual) => {
                return Err(ProjectError::HashMismatch {
                    hash: (*want).to_string(),
                    actual: actual.clone(),
                });
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Reads one entry fully into memory under the size limits.
fn read_entry(
    archive: &mut ZipArchive<File>,
    plan: &Planned,
    name: &str,
    limits: &BundleLimits,
    actual_total: &mut u64,
) -> Result<Vec<u8>> {
    let mut zf = archive.by_index(plan.index).map_err(zip_err)?;
    let mut out = Vec::with_capacity(plan.size.min(1 << 20) as usize);
    let (n, _) = copy_entry(&mut zf, &mut out, plan.size, name)?;
    *actual_total += n;
    if *actual_total > limits.max_total_bytes {
        return Err(ProjectError::BundleLimit(format!(
            "uncompressed size exceeds {} bytes",
            limits.max_total_bytes
        )));
    }
    Ok(out)
}

/// Copies an entry, refusing to produce more than its header declared.
/// Headers can lie; the byte count here is what actually decompressed.
fn copy_entry(
    reader: &mut impl Read,
    writer: &mut impl Write,
    declared: u64,
    name: &str,
) -> Result<(u64, String)> {
    let (n, hash) = copy_hashed(reader, writer, declared).map_err(|e| {
        if e.kind() == io::ErrorKind::InvalidData && e.to_string().contains("allowed size") {
            ProjectError::BundleLimit(format!(
                "{name} decompresses to more than its declared {declared} bytes"
            ))
        } else {
            ProjectError::Bundle(format!("{name}: {e}"))
        }
    })?;
    if n != declared {
        return Err(ProjectError::Bundle(format!(
            "{name} is {n} bytes but its header says {declared}"
        )));
    }
    Ok((n, hash))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_at(path))?;
    f.write_all(bytes).map_err(io_at(path))?;
    f.sync_all().map_err(io_at(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_accepts_only_the_layout() {
        let h = "a".repeat(64);
        assert_eq!(classify("project.peelproj"), Ok(Kind::Root));
        assert_eq!(
            classify(&format!("project.peeldata/objects/{h}.json")),
            Ok(Kind::Object(h.clone()))
        );
        assert_eq!(
            classify("project.peeldata/media/kick.wav"),
            Ok(Kind::Media("kick.wav".into()))
        );
        for bad in [
            "../evil",
            "project.peeldata/../evil",
            "/etc/passwd",
            "C:/evil",
            "project.peeldata\\media\\x.wav",
            "project.peeldata/media/.hidden",
            "project.peeldata/media/CON.wav",
            "project.peeldata/media/a b.wav",
            "project.peeldata/objects/short.json",
            "project.peeldata/other/x",
            "extra.txt",
            "project.peeldata/media/",
            "",
        ] {
            let r = classify(bad);
            assert!(
                r.is_err() || r == Ok(Kind::Dir) && bad.ends_with('/'),
                "{bad:?} -> {r:?}"
            );
        }
        assert!(classify("project.peeldata/media/x/y.wav").is_err());
        assert!(classify("../evil").is_err());
    }

    #[test]
    fn stems_are_sanitised() {
        assert_eq!(bundle_stem(Path::new("/x/My Song.peelbundle")), "My Song");
        assert_eq!(bundle_stem(Path::new("/x/a:b*c.peelbundle")), "a_b_c");
        assert_eq!(bundle_stem(Path::new("/")), "Song");
    }
}
