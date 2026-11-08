//! `Song.peeldata/history/`: the previous root manifests, newest last.
//!
//! Entries are named `<sequence>-<hash16>.peelproj`. The sequence is a
//! monotonic counter rather than a revision or a clock, so undo (which can
//! lower a revision) and clock changes never reorder history.

use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct HistoryEntry {
    pub seq: u64,
    /// First 16 hex digits of the BLAKE3 hash of the root bytes.
    pub hash16: String,
    pub path: PathBuf,
}

pub(crate) fn history_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("history")
}

pub(crate) fn objects_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("objects")
}

pub(crate) fn entry_file_name(seq: u64, root_hash: &str) -> String {
    format!("{seq:010}-{}.peelproj", &root_hash[..16])
}

/// History entries in ascending sequence order. A missing directory is empty.
pub(crate) fn list_history(data_dir: &Path) -> Vec<HistoryEntry> {
    let Ok(read) = fs::read_dir(history_dir(data_dir)) else {
        return Vec::new();
    };
    let mut entries: Vec<HistoryEntry> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".peelproj")?;
            let (seq, hash16) = stem.split_once('-')?;
            if hash16.len() != 16 || !hash16.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            Some(HistoryEntry {
                seq: seq.parse().ok()?,
                hash16: hash16.to_string(),
                path: e.path(),
            })
        })
        .collect();
    entries.sort_by_key(|e| e.seq);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_in_sequence_order_and_ignores_strangers() {
        let dir = tempfile::tempdir().unwrap();
        let hist = history_dir(dir.path());
        fs::create_dir_all(&hist).unwrap();
        let h = "ab".repeat(32);
        for seq in [10u64, 2, 33] {
            fs::write(hist.join(entry_file_name(seq, &h)), b"{}").unwrap();
        }
        fs::write(hist.join("notes.txt"), b"x").unwrap();
        fs::write(hist.join("zz-nothex.peelproj"), b"x").unwrap();
        let seqs: Vec<u64> = list_history(dir.path()).iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![2, 10, 33]);
    }
}
