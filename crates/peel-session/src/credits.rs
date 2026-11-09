//! Source attribution metadata from existing audio sidecars.

use std::io::Read;
use std::path::{Path, PathBuf};

use peel_document::{AudioAsset, Project};

const KEY: &str = "peel.guitar-techs.audio-hashes";

fn sidecar(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".credits.txt");
    PathBuf::from(name)
}

fn credit_text(path: &Path) -> Option<String> {
    let file = std::fs::File::open(sidecar(path)).ok()?;
    let mut text = String::new();
    file.take(65_537).read_to_string(&mut text).ok()?;
    (text.len() <= 65_536).then(|| text.replace("\r\n", "\n"))
}

fn carries_credit(path: &Path) -> bool {
    credit_text(path).is_some_and(|text| {
        text.contains("Peel-Sample-Credit: guitar-techs-14963133-palm-v1")
            || (text.contains("https://zenodo.org/records/14963133") && text.contains("CC BY 4.0"))
    })
}

/// Preserve sample credit from an existing sidecar when audio is imported.
/// Call inside the edit that registers the asset, so undo remains atomic.
pub fn inherit_audio_credits(project: &mut Project, asset: &AudioAsset) {
    if !carries_credit(Path::new(&asset.path)) {
        return;
    }
    let value = project
        .extensions
        .entry(KEY.to_string())
        .or_insert_with(|| serde_json::json!([]));
    if let Some(hashes) = value.as_array_mut()
        && !hashes.iter().any(|h| h.as_str() == Some(&asset.hash))
    {
        hashes.push(serde_json::Value::String(asset.hash.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peel_document::Id;

    #[test]
    fn existing_credits_follow_imported_audio_and_project_roundtrip() {
        let dir = std::env::temp_dir().join(format!("peel-credits-{}", Id::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.wav");
        let notice = include_str!("../../../assets/strings/palm-muted/NOTICE.txt");
        std::fs::write(sidecar(&source), notice).unwrap();
        let asset = AudioAsset {
            id: Id::new(),
            hash: "test-hash".into(),
            name: "source.wav".into(),
            path: source.to_string_lossy().into(),
            managed: false,
            sample_rate: 48000,
            channels: 1,
            frames: 1,
        };
        let mut imported = Project::empty("Imported");
        inherit_audio_credits(&mut imported, &asset);
        imported.assets.push(asset);
        let bytes = serde_json::to_vec(&imported).unwrap();
        let mut loaded: Project = serde_json::from_slice(&bytes).unwrap();
        // Save As and templates change paths while retaining content hashes.
        loaded.assets[0].path = "moved/audio.wav".into();
        assert_eq!(loaded.extensions, imported.extensions);
        assert_eq!(loaded.extensions[KEY], serde_json::json!(["test-hash"]));
        assert_eq!(std::fs::read_to_string(sidecar(&source)).unwrap(), notice);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
