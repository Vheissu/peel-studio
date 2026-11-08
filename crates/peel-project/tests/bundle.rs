mod common;

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use peel_project::{
    BundleLimits, ProjectError, data_dir_for, export_bundle, import_asset, import_bundle,
    import_bundle_with, load, read_manifest, resolve_asset_path, save,
};
use tempfile::{TempDir, tempdir};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use common::{fake_audio, rich_project};

/// A saved project with managed media, and its exported bundle.
fn exported() -> (TempDir, PathBuf, PathBuf) {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    let wav = fake_audio(dir.path(), "kick.wav", 5, 300_000);
    let asset = import_asset(&root, &wav).unwrap();
    let project = rich_project(Some(asset));
    save(&project, &root).unwrap();
    let bundle = dir.path().join("Shared Song.peelbundle");
    export_bundle(&root, &bundle).unwrap();
    (dir, root, bundle)
}

fn entries(bundle: &Path) -> Vec<(String, Vec<u8>)> {
    let mut zip = ZipArchive::new(File::open(bundle).unwrap()).unwrap();
    (0..zip.len())
        .map(|i| {
            let mut f = zip.by_index(i).unwrap();
            let mut data = Vec::new();
            f.read_to_end(&mut data).unwrap();
            (f.name().unwrap().into_owned(), data)
        })
        .collect()
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut zip = ZipWriter::new(File::create(path).unwrap());
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in entries {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

/// Rebuilds `bundle` with `f` applied to its entry list.
fn rewrite(bundle: &Path, out: &Path, f: impl FnOnce(&mut Vec<(String, Vec<u8>)>)) {
    let mut all = entries(bundle);
    f(&mut all);
    let refs: Vec<(&str, &[u8])> = all
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    write_zip(out, &refs);
}

fn assert_nothing_extracted(parent: &Path, dest: &Path) {
    assert!(!dest.exists(), "destination must not be created");
    let stray: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.ends_with(".zip") && !n.ends_with(".peelbundle"))
        .collect();
    assert!(stray.is_empty(), "left behind: {stray:?}");
}

#[test]
fn bundle_round_trip_restores_the_project_and_media() {
    let (dir, root, bundle) = exported();
    let original = load(&root).unwrap().project;

    // Layout inside the archive is the documented one.
    let names: Vec<String> = entries(&bundle).into_iter().map(|(n, _)| n).collect();
    assert!(names.contains(&"project.peelproj".to_string()));
    assert_eq!(
        names
            .iter()
            .filter(|n| n.starts_with("project.peeldata/objects/"))
            .count(),
        5
    );
    assert_eq!(
        names
            .iter()
            .filter(|n| n.starts_with("project.peeldata/media/"))
            .count(),
        1
    );

    let dest = dir.path().join("imported");
    let imported_root = import_bundle(&bundle, &dest).unwrap();
    assert_eq!(imported_root, dest.join("Shared Song.peelproj"));

    let loaded = load(&imported_root).unwrap();
    assert_eq!(loaded.project, original);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    let asset = original.assets.iter().find(|a| a.managed).unwrap();
    assert_eq!(
        fs::read(resolve_asset_path(&imported_root, asset)).unwrap(),
        fs::read(resolve_asset_path(&root, asset)).unwrap()
    );
    assert_eq!(
        read_manifest(&imported_root).unwrap(),
        read_manifest(&root).unwrap()
    );
}

#[test]
fn export_report_lists_what_was_left_out() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    let wav = fake_audio(dir.path(), "kick.wav", 5, 2000);
    let asset = import_asset(&root, &wav).unwrap();
    let project = rich_project(Some(asset));
    save(&project, &root).unwrap();
    // Media removed after the import: reported, not fatal.
    fs::remove_dir_all(data_dir_for(&root).join("media")).unwrap();

    let report = export_bundle(&root, &dir.path().join("x.peelbundle")).unwrap();
    assert_eq!(report.media_files, 0);
    assert_eq!(report.missing_media, vec!["kick.wav".to_string()]);
    assert_eq!(
        report.external_assets,
        vec!["external loop.wav".to_string()]
    );
    assert_eq!(report.objects, 6);
}

#[test]
fn bundles_are_deterministic() {
    let (dir, root, bundle) = exported();
    let again = dir.path().join("again.peelbundle");
    export_bundle(&root, &again).unwrap();
    assert_eq!(fs::read(&bundle).unwrap(), fs::read(&again).unwrap());
}

#[test]
fn destination_must_be_fresh() {
    let (dir, _root, bundle) = exported();

    let occupied = dir.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("precious.txt"), b"keep me").unwrap();
    assert!(matches!(
        import_bundle(&bundle, &occupied),
        Err(ProjectError::DestinationNotEmpty(_))
    ));
    assert_eq!(fs::read(occupied.join("precious.txt")).unwrap(), b"keep me");

    let file = dir.path().join("a-file");
    fs::write(&file, b"x").unwrap();
    assert!(matches!(
        import_bundle(&bundle, &file),
        Err(ProjectError::DestinationNotEmpty(_))
    ));

    // An existing empty directory is fine.
    let empty = dir.path().join("empty");
    fs::create_dir(&empty).unwrap();
    let root = import_bundle(&bundle, &empty).unwrap();
    assert!(load(&root).is_ok());
}

#[test]
fn path_traversal_entries_are_rejected() {
    let (dir, _root, bundle) = exported();
    let good = entries(&bundle);

    for evil in [
        "../evil",
        "project.peeldata/../../evil",
        "project.peeldata/media/../../evil.wav",
        "/tmp/evil",
        "C:/evil.txt",
        "project.peeldata\\media\\evil.wav",
        "project.peeldata/media/a b.wav",
        "project.peeldata//media/x.wav",
    ] {
        let mut all = good.clone();
        all.push((evil.to_string(), b"pwned".to_vec()));
        let refs: Vec<(&str, &[u8])> = all
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_slice()))
            .collect();
        let sandbox = tempdir().unwrap();
        let bad = sandbox.path().join("bad.peelbundle");
        write_zip(&bad, &refs);
        let dest = sandbox.path().join("out");
        match import_bundle(&bad, &dest) {
            Err(ProjectError::UnsafeBundleEntry { name, .. }) => assert_eq!(name, evil),
            other => panic!("{evil:?} was not rejected: {other:?}"),
        }
        assert_nothing_extracted(sandbox.path(), &dest);
        assert!(!sandbox.path().join("evil").exists());
        assert!(!sandbox.path().parent().unwrap().join("evil").exists());
    }
    drop(dir);
}

#[test]
fn symlink_entries_are_rejected() {
    let (_dir, _root, bundle) = exported();
    let sandbox = tempdir().unwrap();
    let bad = sandbox.path().join("link.peelbundle");

    let mut zip = ZipWriter::new(File::create(&bad).unwrap());
    let opts = SimpleFileOptions::default();
    for (name, data) in entries(&bundle) {
        zip.start_file(name, opts).unwrap();
        zip.write_all(&data).unwrap();
    }
    zip.add_symlink("project.peeldata/media/link.wav", "/etc/passwd", opts)
        .unwrap();
    zip.finish().unwrap();

    let dest = sandbox.path().join("out");
    match import_bundle(&bad, &dest) {
        Err(ProjectError::UnsafeBundleEntry { reason, .. }) => {
            assert!(reason.contains("symbolic link"), "{reason}")
        }
        other => panic!("symlink accepted: {other:?}"),
    }
    assert_nothing_extracted(sandbox.path(), &dest);
}

#[test]
fn duplicate_entries_are_rejected() {
    let sandbox = tempdir().unwrap();
    let bad = sandbox.path().join("dup.peelbundle");
    write_zip(
        &bad,
        &[
            ("project.peelproj", b"{}"),
            // Renamed to a duplicate below; same length so offsets hold.
            ("project.peelprok", b"{}"),
        ],
    );
    let mut bytes = fs::read(&bad).unwrap();
    let needle = b"project.peelprok";
    let mut replaced = 0;
    while let Some(pos) = bytes.windows(needle.len()).position(|w| w == needle) {
        bytes[pos..pos + needle.len()].copy_from_slice(b"project.peelproj");
        replaced += 1;
    }
    assert_eq!(replaced, 2, "local header and central directory");
    fs::write(&bad, bytes).unwrap();

    let dest = sandbox.path().join("out");
    match import_bundle(&bad, &dest) {
        Err(ProjectError::Bundle(m)) => assert!(m.contains("duplicate"), "{m}"),
        other => panic!("duplicates accepted: {other:?}"),
    }
    assert_nothing_extracted(sandbox.path(), &dest);
}

#[test]
fn case_colliding_names_are_rejected() {
    let (_dir, _root, bundle) = exported();
    let sandbox = tempdir().unwrap();
    let bad = sandbox.path().join("case.peelbundle");
    rewrite(&bundle, &bad, |all| {
        let media = all
            .iter()
            .find(|(n, _)| n.starts_with("project.peeldata/media/"))
            .unwrap()
            .clone();
        let upper = media.0.replace("kick", "KICK").replace(".wav", ".WAV");
        let (name, data) = media;
        // Same media twice under names that differ only by case.
        all.push((name.replace(".wav", ".Wav"), data.clone()));
        all.push((upper, data));
    });
    let dest = sandbox.path().join("out");
    match import_bundle(&bad, &dest) {
        Err(ProjectError::UnsafeBundleEntry { reason, .. }) => {
            assert!(reason.contains("duplicate"), "{reason}")
        }
        other => panic!("case collision accepted: {other:?}"),
    }
    assert_nothing_extracted(sandbox.path(), &dest);
}

#[test]
fn zip_bombs_are_rejected() {
    // 64 MiB of zeros deflates to roughly 64 KiB: a ratio above 900:1.
    let sandbox = tempdir().unwrap();
    let bomb = sandbox.path().join("bomb.peelbundle");
    let zeros = vec![0u8; 64 * 1024 * 1024];
    write_zip(
        &bomb,
        &[
            ("project.peelproj", b"{}"),
            ("project.peeldata/media/silence.wav", &zeros),
        ],
    );
    assert!(fs::metadata(&bomb).unwrap().len() < 1024 * 1024);

    let dest = sandbox.path().join("out");
    match import_bundle(&bomb, &dest) {
        Err(ProjectError::BundleLimit(m)) => assert!(m.contains("ratio"), "{m}"),
        other => panic!("bomb accepted: {other:?}"),
    }
    assert_nothing_extracted(sandbox.path(), &dest);
}

#[test]
fn size_limits_apply_to_totals_and_entries() {
    let (dir, _root, bundle) = exported();

    let tight = BundleLimits {
        max_total_bytes: 100_000,
        ..BundleLimits::default()
    };
    let dest = dir.path().join("total");
    assert!(matches!(
        import_bundle_with(&bundle, &dest, &tight),
        Err(ProjectError::BundleLimit(_))
    ));
    assert!(!dest.exists());

    let tight = BundleLimits {
        max_entry_bytes: 1000,
        ..BundleLimits::default()
    };
    let dest = dir.path().join("entry");
    assert!(matches!(
        import_bundle_with(&bundle, &dest, &tight),
        Err(ProjectError::BundleLimit(_))
    ));

    let tight = BundleLimits {
        max_entries: 2,
        ..BundleLimits::default()
    };
    assert!(matches!(
        import_bundle_with(&bundle, &dir.path().join("count"), &tight),
        Err(ProjectError::BundleLimit(_))
    ));
}

#[test]
fn tampered_shard_is_rejected() {
    let (dir, _root, bundle) = exported();
    let bad = dir.path().join("tampered.peelbundle");
    rewrite(&bundle, &bad, |all| {
        let shard = all
            .iter_mut()
            .find(|(n, _)| n.starts_with("project.peeldata/objects/"))
            .unwrap();
        let mid = shard.1.len() / 2;
        shard.1[mid] ^= 0x20;
    });
    let dest = dir.path().join("out");
    assert!(matches!(
        import_bundle(&bad, &dest),
        Err(ProjectError::HashMismatch { .. })
    ));
    assert!(!dest.exists());
}

#[test]
fn tampered_media_is_rejected() {
    let (dir, _root, bundle) = exported();
    let bad = dir.path().join("tampered-media.peelbundle");
    rewrite(&bundle, &bad, |all| {
        let media = all
            .iter_mut()
            .find(|(n, _)| n.starts_with("project.peeldata/media/"))
            .unwrap();
        media.1[10] ^= 0xFF;
    });
    let dest = dir.path().join("out");
    assert!(matches!(
        import_bundle(&bad, &dest),
        Err(ProjectError::HashMismatch { .. })
    ));
    assert!(!dest.exists());
}

#[test]
fn missing_and_extra_objects_are_rejected() {
    let (dir, _root, bundle) = exported();

    let missing = dir.path().join("missing.peelbundle");
    rewrite(&bundle, &missing, |all| {
        let pos = all
            .iter()
            .position(|(n, _)| n.starts_with("project.peeldata/objects/"))
            .unwrap();
        all.remove(pos);
    });
    assert!(matches!(
        import_bundle(&missing, &dir.path().join("a")),
        Err(ProjectError::MissingObject { .. })
    ));

    let extra = dir.path().join("extra.peelbundle");
    rewrite(&bundle, &extra, |all| {
        let data = b"{}".to_vec();
        let hash = blake3::hash(&data).to_hex().to_string();
        all.push((format!("project.peeldata/objects/{hash}.json"), data));
    });
    assert!(matches!(
        import_bundle(&extra, &dir.path().join("b")),
        Err(ProjectError::UnsafeBundleEntry { .. })
    ));

    let orphan_media = dir.path().join("orphan.peelbundle");
    rewrite(&bundle, &orphan_media, |all| {
        all.push(("project.peeldata/media/orphan.wav".into(), vec![1, 2, 3]));
    });
    assert!(matches!(
        import_bundle(&orphan_media, &dir.path().join("c")),
        Err(ProjectError::UnsafeBundleEntry { .. })
    ));
}

#[test]
fn not_a_zip_and_foreign_zip_are_rejected() {
    let sandbox = tempdir().unwrap();
    let junk = sandbox.path().join("junk.peelbundle");
    fs::write(&junk, b"definitely not a zip").unwrap();
    assert!(matches!(
        import_bundle(&junk, &sandbox.path().join("out")),
        Err(ProjectError::Bundle(_))
    ));

    let foreign = sandbox.path().join("foreign.peelbundle");
    write_zip(&foreign, &[("readme.txt", b"hello")]);
    assert!(matches!(
        import_bundle(&foreign, &sandbox.path().join("out2")),
        Err(ProjectError::UnsafeBundleEntry { .. })
    ));

    let no_root = sandbox.path().join("noroot.peelbundle");
    write_zip(&no_root, &[("project.peeldata/media/a.wav", b"x")]);
    assert!(import_bundle(&no_root, &sandbox.path().join("out3")).is_err());
}

#[test]
fn export_refuses_to_pack_corrupt_projects_and_leaves_no_partial_file() {
    let (dir, root, _bundle) = exported();
    let manifest = read_manifest(&root).unwrap();
    fs::write(
        data_dir_for(&root)
            .join("objects")
            .join(format!("{}.json", manifest.objects.patterns[0].hash)),
        b"damaged",
    )
    .unwrap();
    let out = dir.path().join("broken.peelbundle");
    assert!(export_bundle(&root, &out).is_err());
    assert!(!out.exists());
    let strays: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp-"))
        .collect();
    assert!(strays.is_empty(), "{strays:?}");
}
