//! Attribution for the bundled CC BY palm-muted recordings.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use peel_document::{AudioAsset, Project};

const KEY: &str = "peel.guitar-techs.audio-hashes";
const NOTICE: &str = include_str!("../../../assets/strings/palm-muted/NOTICE.txt");

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

fn complete_credit(path: &Path) -> bool {
    credit_text(path).is_some_and(|text| {
        [
            "https://zenodo.org/records/14963133",
            "CC BY 4.0",
            "https://creativecommons.org/licenses/by/4.0/",
            "Pedroza",
            "Taheri",
            "Abreu",
            "Corey",
            "Roman",
        ]
        .iter()
        .all(|part| text.contains(part))
    })
}

/// Preserve a Peel export's sample credit when its audio is imported.
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

fn required(project: &Project) -> bool {
    project
        .channels
        .iter()
        .any(|c| c.device.type_id == "peel.guitar.palm_muted")
        || project
            .extensions
            .get(KEY)
            .and_then(|v| v.as_array())
            .is_some_and(|hashes| {
                project
                    .assets
                    .iter()
                    .any(|a| hashes.iter().any(|h| h.as_str() == Some(&a.hash)))
            })
}

pub(crate) fn write(project: &Project, path: &Path) -> Result<(), String> {
    if !required(project) {
        return Ok(());
    }
    let target = sidecar(path);
    // Never truncate a user's existing credit file or follow a symlink to write.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
    {
        Ok(mut file) => file.write_all(NOTICE.as_bytes()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && complete_credit(path) => Ok(()),
        Err(e) => Err(e),
    }
    .map_err(|e| {
        format!(
            "Audio saved, but could not write sample credits to {}: {e}",
            target.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use peel_document::{DeviceInstance, Id};

    #[test]
    fn credits_follow_imported_audio_and_project_roundtrip() {
        let dir = std::env::temp_dir().join(format!("peel-credits-{}", Id::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.wav");
        let mut p = Project::empty("Credit");
        crate::template::add_channel(
            &mut p,
            "Chug",
            0,
            DeviceInstance::new("peel.guitar.palm_muted", 1),
            40,
        );
        write(&p, &source).unwrap();
        assert_eq!(std::fs::read_to_string(sidecar(&source)).unwrap(), NOTICE);
        // A repeated export can reuse its own notice, but not replace user text.
        write(&p, &source).unwrap();
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
        // Asset paths change on Save As or template creation; content hashes stay.
        loaded.assets[0].path = "moved/audio.wav".into();
        let output = dir.join("output.flac");
        write(&loaded, &output).unwrap();
        assert!(carries_credit(&output));
        std::fs::write(sidecar(&output), "My existing credits").unwrap();
        assert!(
            write(&loaded, &output)
                .unwrap_err()
                .contains("sample credits")
        );
        assert_eq!(
            std::fs::read_to_string(sidecar(&output)).unwrap(),
            "My existing credits"
        );
        loaded.assets.clear();
        assert!(!required(&loaded));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_sounds_do_not_need_sidecars() {
        let mut p = Project::empty("Legacy");
        crate::template::add_channel(
            &mut p,
            "Guitar",
            0,
            crate::template::device("peel.guitar.recorded", Some("Palm Mute")),
            40,
        );
        assert!(!required(&p));
        assert!(!required(&crate::template::empty()));
    }
}
