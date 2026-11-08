//! Filesystem and formatting helpers shared by the persistence modules.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Result, io_at};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp file name beside `path`: `.<name>.tmp-<pid>-<n>`.
pub(crate) fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.tmp-{}-{n}", std::process::id()))
}

pub(crate) fn is_temp_name(name: &str) -> bool {
    name.starts_with('.') && name.contains(".tmp-")
}

/// Writes `bytes` to a temp file beside `path`, flushes it to disk and renames
/// it into place. A crash leaves either the old file or the new one, never a
/// partial `path`. The parent directory is not synced; callers batch that.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = temp_sibling(path);
    let write = || -> io::Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&tmp);
        return Err(io_at(&tmp)(e));
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(io_at(path)(e));
    }
    Ok(())
}

/// Flushes directory metadata so renames and creations inside it survive a
/// power loss. Windows cannot open a directory as a file, so this is a no-op
/// there; filesystems that refuse directory fsync are tolerated.
pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        match fs::File::open(dir).and_then(|f| f.sync_all()) {
            Ok(()) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
                ) => {}
            Err(e) => return Err(io_at(dir)(e)),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

pub(crate) fn hash_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// True for a 64-digit lowercase hexadecimal BLAKE3 digest. Hashes from a
/// manifest become file names, so they are checked before use.
pub(crate) fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// RFC 3339 UTC timestamp with millisecond precision.
pub(crate) fn utc_timestamp(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let millis = d.subsec_millis();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, day) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to (year, month, day) in the proleptic Gregorian
/// calendar (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Checks a `/`-separated relative path stored inside a project or bundle:
/// no empty, `.` or `..` segments, no absolute prefix, drive letters,
/// backslashes or control characters. Returns the segments.
pub(crate) fn safe_relative_segments(path: &str) -> std::result::Result<Vec<&str>, &'static str> {
    if path.is_empty() {
        return Err("path is empty");
    }
    if path.len() > 1024 {
        return Err("path is too long");
    }
    let mut out = Vec::new();
    for seg in path.split('/') {
        if seg.is_empty() {
            return Err("empty path segment or absolute path");
        }
        if seg == "." || seg == ".." {
            return Err("path escapes its directory");
        }
        if seg.len() > 255 {
            return Err("path segment is too long");
        }
        if seg.chars().any(|c| c.is_control() || c == '\\' || c == ':') {
            return Err("path contains a forbidden character");
        }
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Err("path segment ends with a dot or space");
        }
        out.push(seg);
    }
    Ok(out)
}

/// `dir` joined with the segments of a validated relative path.
pub(crate) fn join_segments(dir: &Path, segments: &[&str]) -> PathBuf {
    let mut p = dir.to_path_buf();
    for s in segments {
        p.push(s);
    }
    p
}

pub(crate) fn create_dir_all(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(io_at(dir))
}

/// Streams `reader` into `writer` through a fixed buffer, hashing as it goes.
/// Fails with `InvalidData` once more than `limit` bytes have been read.
pub(crate) fn copy_hashed(
    reader: &mut impl io::Read,
    writer: &mut impl io::Write,
    limit: u64,
) -> io::Result<(u64, String)> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 128 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "data exceeds its allowed size",
            ));
        }
        hasher.update(&buf[..n]);
        writer.write_all(&buf[..n])?;
    }
    Ok((total, hasher.finalize().to_hex().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        // Leap day.
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_800);
        assert_eq!(utc_timestamp(t), "2024-02-29T00:00:00.000Z");
        let t = UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_123);
        assert_eq!(utc_timestamp(t), "2023-11-14T22:13:20.123Z");
    }

    #[test]
    fn relative_path_rules() {
        assert!(safe_relative_segments("media/a.wav").is_ok());
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "C:/x",
            "a\\b",
            "a/./b",
            "a/b/",
            "x.",
        ] {
            assert!(safe_relative_segments(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn hash_shape() {
        assert!(is_hash(&hash_hex(b"x")));
        assert!(!is_hash("../../etc/passwd"));
        assert!(!is_hash(&hash_hex(b"x").to_uppercase()));
    }
}
