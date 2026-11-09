mod common;

use std::fs;
use std::path::Path;

use peel_document::{Id, Note, Project, Time};
use peel_project::{
    AudioProbe, ProjectError, SaveOptions, collect_garbage, data_dir_for, import_asset,
    import_asset_with, load, read_manifest, resolve_asset_path, save, save_as, save_with,
};
use tempfile::tempdir;

use common::{fake_audio, rich_project, shard_files};

fn project_with_media(dir: &Path) -> (Project, std::path::PathBuf) {
    let root = dir.join("Song.peelproj");
    let src = fake_audio(dir, "Kick Sample.WAV", 7, 4096);
    let asset = import_asset(&root, &src).unwrap();
    (rich_project(Some(asset)), root)
}

fn edit_pattern(project: &mut Project, index: usize, f: impl FnOnce(&mut peel_document::Pattern)) {
    f(std::sync::Arc::make_mut(&mut project.patterns[index]));
    project.revision += 1;
}

#[test]
fn save_load_round_trip_is_exact() {
    let dir = tempdir().unwrap();
    let (project, root) = project_with_media(dir.path());

    let report = save(&project, &root).unwrap();
    assert_eq!(report.revision, 41);
    assert!(report.missing_media.is_empty());

    let loaded = load(&root).unwrap();
    assert_eq!(loaded.project, project);
    assert!(!loaded.recovered_from_history);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);

    // Triplets are exact rationals after the trip.
    let triplets = &loaded.project.patterns[0];
    assert!(triplets.notes.iter().any(|n| n.start == Time::new(2, 3)));
    assert!(triplets.notes.iter().any(|n| n.length == Time::new(1, 960)));
}

#[test]
fn round_trip_without_assets_and_with_empty_project() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Empty.peelproj");
    let project = Project::empty("Empty");
    save(&project, &root).unwrap();
    assert_eq!(load(&root).unwrap().project, project);
}

#[test]
fn layout_and_manifest_contents() {
    let dir = tempdir().unwrap();
    let (project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();

    let data = data_dir_for(&root);
    assert_eq!(data, dir.path().join("Song.peeldata"));
    assert!(data.join("objects").is_dir());
    assert!(data.join("history").is_dir());
    assert!(data.join("media").is_dir());

    let manifest = read_manifest(&root).unwrap();
    assert_eq!(manifest.format, "peel-project-manifest");
    assert_eq!(manifest.schema, peel_document::SCHEMA_VERSION);
    assert_eq!(manifest.project_id, project.id);
    assert_eq!(manifest.revision, 41);
    assert_eq!(manifest.title, "Rich Song");
    assert!(manifest.saved_at.ends_with('Z'));
    assert_eq!(manifest.data_dir, "Song.peeldata");
    assert_eq!(
        manifest.required_capabilities,
        vec![
            "peel.drum.kick",
            "peel.fx.compressor",
            "peel.sampler",
            "peel.synth.poly"
        ]
    );
    // core + 2 patterns + 2 arrangements, each stored under its hash.
    assert_eq!(manifest.objects().count(), 5);
    assert_eq!(shard_files(&root).len(), 5);
    for obj in manifest.objects() {
        let file = data.join("objects").join(format!("{}.json", obj.hash));
        let bytes = fs::read(&file).unwrap();
        assert_eq!(bytes.len() as u64, obj.size);
        assert_eq!(blake3::hash(&bytes).to_hex().to_string(), obj.hash);
    }

    // The root is readable JSON that a person can open.
    let text = fs::read_to_string(&root).unwrap();
    assert!(text.contains("\"format\": \"peel-project-manifest\""));
}

#[test]
fn unchanged_shards_are_not_rewritten() {
    let dir = tempdir().unwrap();
    let (mut project, root) = project_with_media(dir.path());
    let first = save(&project, &root).unwrap();
    assert_eq!(first.shards_written, 5);
    assert_eq!(first.shards_reused, 0);

    let before: Vec<_> = shard_files(&root)
        .into_iter()
        .map(|p| {
            let m = fs::metadata(&p).unwrap().modified().unwrap();
            (p, m)
        })
        .collect();

    // Nothing changed: no shard is written.
    std::thread::sleep(std::time::Duration::from_millis(30));
    let again = save(&project, &root).unwrap();
    assert_eq!(again.shards_written, 0);
    assert_eq!(again.shards_reused, 5);
    assert_eq!(again.bytes_written, 0);
    for (p, m) in &before {
        assert_eq!(&fs::metadata(p).unwrap().modified().unwrap(), m);
    }

    // Edit one pattern: exactly one new shard, the rest keep their mtime.
    edit_pattern(&mut project, 1, |pat| pat.name = "Renamed".into());
    std::thread::sleep(std::time::Duration::from_millis(30));
    let third = save(&project, &root).unwrap();
    assert_eq!(third.shards_written, 1);
    assert_eq!(third.shards_reused, 4);
    for (p, m) in &before {
        if p.exists() {
            assert_eq!(&fs::metadata(p).unwrap().modified().unwrap(), m);
        }
    }
    assert_eq!(load(&root).unwrap().project, project);
}

#[test]
fn corrupt_shard_is_an_error_when_there_is_nothing_to_recover() {
    let dir = tempdir().unwrap();
    let (project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();

    let manifest = read_manifest(&root).unwrap();
    let victim = data_dir_for(&root)
        .join("objects")
        .join(format!("{}.json", manifest.objects.patterns[0].hash));
    let mut bytes = fs::read(&victim).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x01;
    fs::write(&victim, bytes).unwrap();

    match load(&root) {
        Err(ProjectError::HashMismatch { hash, .. }) => {
            assert_eq!(hash, manifest.objects.patterns[0].hash)
        }
        other => panic!("expected a hash mismatch, got {other:?}"),
    }
}

#[test]
fn missing_shard_is_reported() {
    let dir = tempdir().unwrap();
    let (project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();
    let manifest = read_manifest(&root).unwrap();
    fs::remove_file(
        data_dir_for(&root)
            .join("objects")
            .join(format!("{}.json", manifest.objects.arrangements[0].hash)),
    )
    .unwrap();
    assert!(matches!(
        load(&root),
        Err(ProjectError::MissingObject { .. })
    ));
}

#[test]
fn corrupt_latest_root_recovers_from_history() {
    let dir = tempdir().unwrap();
    let (mut project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();
    let v1 = project.clone();

    edit_pattern(&mut project, 0, |p| p.name = "Second save".into());
    save(&project, &root).unwrap();
    assert_eq!(load(&root).unwrap().project, project);

    // A torn or garbled root.
    fs::write(&root, b"{ \"format\": \"peel-project-manifest\", \"sche").unwrap();
    let loaded = load(&root).unwrap();
    assert!(loaded.recovered_from_history);
    assert_eq!(loaded.project, v1);
    assert!(
        loaded.warnings[0].contains("could not be loaded"),
        "{:?}",
        loaded.warnings
    );

    // Saving again repairs the root and keeps working.
    let mut repaired = loaded.project;
    edit_pattern(&mut repaired, 1, |p| p.name = "After recovery".into());
    save(&repaired, &root).unwrap();
    let again = load(&root).unwrap();
    assert!(!again.recovered_from_history);
    assert_eq!(again.project, repaired);
}

#[test]
fn corrupt_latest_shard_recovers_from_history() {
    let dir = tempdir().unwrap();
    let (mut project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();
    let v1 = project.clone();

    edit_pattern(&mut project, 0, |p| p.name = "Edited".into());
    save(&project, &root).unwrap();

    // Damage the shard only the newest root uses.
    let manifest = read_manifest(&root).unwrap();
    let victim = data_dir_for(&root)
        .join("objects")
        .join(format!("{}.json", manifest.objects.patterns[0].hash));
    fs::write(&victim, b"garbage").unwrap();

    let loaded = load(&root).unwrap();
    assert!(loaded.recovered_from_history);
    assert_eq!(loaded.project, v1);
}

#[test]
fn recovery_skips_damaged_history_entries() {
    let dir = tempdir().unwrap();
    let (mut project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();
    let v1 = project.clone();
    edit_pattern(&mut project, 0, |p| p.name = "v2".into());
    save(&project, &root).unwrap();
    edit_pattern(&mut project, 0, |p| p.name = "v3".into());
    save(&project, &root).unwrap();

    fs::write(&root, b"").unwrap();
    // Newest history entry is v2's root; damage it too.
    let mut hist: Vec<_> = fs::read_dir(data_dir_for(&root).join("history"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    hist.sort();
    assert_eq!(hist.len(), 2);
    fs::write(hist.last().unwrap(), b"nope").unwrap();

    let loaded = load(&root).unwrap();
    assert!(loaded.recovered_from_history);
    assert_eq!(loaded.project, v1);
}

#[test]
fn newer_schema_is_rejected_not_recovered() {
    let dir = tempdir().unwrap();
    let (mut project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();
    edit_pattern(&mut project, 0, |p| p.name = "v2".into());
    save(&project, &root).unwrap();

    let text = fs::read_to_string(&root).unwrap();
    let bumped = text.replace("\"schema\": 1", "\"schema\": 2");
    assert_ne!(text, bumped);
    fs::write(&root, bumped).unwrap();

    match load(&root) {
        Err(ProjectError::FutureSchema { found, supported }) => {
            assert_eq!(found, 2);
            assert_eq!(supported, 1);
        }
        other => panic!("expected FutureSchema, got {other:?}"),
    }
    // And saving over it is refused so the newer project is not destroyed.
    assert!(matches!(
        save(&project, &root),
        Err(ProjectError::FutureSchema { .. })
    ));
}

#[test]
fn foreign_file_is_not_a_project() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("x.peelproj");
    fs::write(&root, br#"{"hello": "world"}"#).unwrap();
    assert!(matches!(load(&root), Err(ProjectError::NotAProject(_))));
    assert!(matches!(
        load(&dir.path().join("missing.peelproj")),
        Err(ProjectError::Io { .. })
    ));
}

#[test]
fn history_is_bounded_and_unreferenced_shards_are_collected() {
    let dir = tempdir().unwrap();
    let (mut project, root) = project_with_media(dir.path());
    let opts = SaveOptions {
        history_limit: 3,
        ..SaveOptions::default()
    };
    let mut last = None;
    for i in 0..10 {
        edit_pattern(&mut project, 0, |p| p.name = format!("Take {i}"));
        last = Some(save_with(&project, &root, &opts).unwrap());
    }
    let history = fs::read_dir(data_dir_for(&root).join("history"))
        .unwrap()
        .count();
    assert_eq!(history, 3);
    assert!(last.unwrap().shards_collected > 0);

    // Live shards: current root (5) plus the pattern shards of 3 history roots.
    let shards = shard_files(&root).len();
    assert!(shards <= 5 + 3, "{shards} shards kept");
    assert_eq!(load(&root).unwrap().project, project);

    // Nothing left for a further pass; dry runs delete nothing.
    let dry = collect_garbage(&root, true).unwrap();
    assert!(dry.removed.is_empty());
    assert!(dry.retained >= 5);

    // A stray unreferenced shard shows up in a dry run and is removed for real.
    let stray = data_dir_for(&root)
        .join("objects")
        .join(format!("{}.json", "ab".repeat(32)));
    fs::write(&stray, b"{}").unwrap();
    let dry = collect_garbage(&root, true).unwrap();
    assert_eq!(dry.removed, vec![stray.clone()]);
    assert!(stray.exists());
    collect_garbage(&root, false).unwrap();
    assert!(!stray.exists());
}

#[test]
fn history_keeps_older_roots_loadable() {
    let dir = tempdir().unwrap();
    let (mut project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();
    for i in 0..4 {
        edit_pattern(&mut project, 1, |p| p.name = format!("v{i}"));
        save(&project, &root).unwrap();
    }
    // Every history root still resolves completely after garbage collection.
    let mut checked = 0;
    for entry in fs::read_dir(data_dir_for(&root).join("history")).unwrap() {
        let path = entry.unwrap().path();
        let manifest = peel_project::Manifest::parse(&fs::read(&path).unwrap()).unwrap();
        for obj in manifest.objects() {
            assert!(
                data_dir_for(&root)
                    .join("objects")
                    .join(format!("{}.json", obj.hash))
                    .exists()
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 4);
}

#[test]
fn invalid_and_non_finite_projects_are_refused_and_leave_the_root_alone() {
    let dir = tempdir().unwrap();
    let (project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();
    let root_before = fs::read(&root).unwrap();

    let mut dangling = project.clone();
    dangling.channels[0].track = Id::new();
    assert!(matches!(
        save(&dangling, &root),
        Err(ProjectError::Invalid(_))
    ));

    let mut nan = project.clone();
    nan.swing = f32::NAN;
    assert!(matches!(save(&nan, &root), Err(ProjectError::Unsavable(_))));

    let mut inf = project.clone();
    inf.tempo[0].bpm = f64::INFINITY;
    assert!(matches!(save(&inf, &root), Err(ProjectError::Unsavable(_))));

    let mut bad_path = project.clone();
    bad_path.assets[0].path = "../../outside.wav".into();
    assert!(matches!(
        save(&bad_path, &root),
        Err(ProjectError::Invalid(_))
    ));

    assert_eq!(fs::read(&root).unwrap(), root_before);
    assert_eq!(load(&root).unwrap().project, project);
}

#[test]
fn leftover_temp_files_are_cleaned_up() {
    let dir = tempdir().unwrap();
    let (project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();
    let leftover = data_dir_for(&root)
        .join("objects")
        .join(".abc.json.tmp-999-1");
    fs::write(&leftover, b"half a shard").unwrap();
    save(&project, &root).unwrap();
    assert!(!leftover.exists());
    // No temp files remain beside the root either.
    let strays: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp-"))
        .collect();
    assert!(strays.is_empty(), "{strays:?}");
}

#[test]
fn unknown_fields_produce_warnings() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    let project = Project::empty("Future");
    save(&project, &root).unwrap();

    // Simulate a newer minor version: add a field to the pattern shard and
    // re-address it, as that version would have.
    let manifest = read_manifest(&root).unwrap();
    let objects = data_dir_for(&root).join("objects");
    let old = objects.join(format!("{}.json", manifest.objects.patterns[0].hash));
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&old).unwrap()).unwrap();
    value["pattern"]["groove"] = serde_json::json!({"amount": 0.5});
    let bytes = serde_json::to_vec(&value).unwrap();
    let hash = blake3::hash(&bytes).to_hex().to_string();
    fs::write(objects.join(format!("{hash}.json")), &bytes).unwrap();

    let mut root_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&root).unwrap()).unwrap();
    root_json["objects"]["patterns"][0]["hash"] = hash.into();
    root_json["objects"]["patterns"][0]["size"] = bytes.len().into();
    root_json["future_root_field"] = true.into();
    fs::write(&root, serde_json::to_vec_pretty(&root_json).unwrap()).unwrap();

    let loaded = load(&root).unwrap();
    assert_eq!(loaded.project, project);
    let joined = loaded.warnings.join("\n");
    assert!(joined.contains("pattern.groove"), "{joined}");
    assert!(joined.contains("future_root_field"), "{joined}");
}

#[test]
fn extensions_survive_and_key_order_is_canonical() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    let mut a = Project::empty("Ext");
    a.extensions
        .insert("x".into(), serde_json::json!({"b": 1, "a": 2}));
    let mut b = a.clone();
    b.extensions
        .insert("x".into(), serde_json::json!({"a": 2, "b": 1}));
    let first = save(&a, &root).unwrap();
    let second = save(&b, &root).unwrap();
    assert_eq!(first.shards_written, 3);
    assert_eq!(second.shards_written, 0, "same content, same hash");
    assert_eq!(load(&root).unwrap().project.extensions, a.extensions);
}

#[test]
fn managed_assets_are_content_addressed() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    let a = fake_audio(dir.path(), "Snare Hit.WAV", 3, 1000);
    let b = fake_audio(dir.path(), "copy of snare.wav", 3, 1000);
    let c = fake_audio(dir.path(), "no extension", 9, 10);

    let one = import_asset(&root, &a).unwrap();
    let two = import_asset(&root, &b).unwrap();
    assert_eq!(one.hash, two.hash);
    assert_ne!(one.id, two.id);
    assert_eq!(one.path, format!("media/{}.wav", one.hash));
    assert_eq!(one.name, "Snare Hit.WAV");
    assert_eq!(two.name, "copy of snare.wav");
    assert!(one.managed);
    assert_eq!((one.sample_rate, one.channels, one.frames), (0, 0, 0));
    assert_eq!(one.hash, blake3::hash(&[3u8; 1000]).to_hex().to_string());

    let media = data_dir_for(&root).join("media");
    assert_eq!(fs::read_dir(&media).unwrap().count(), 1, "stored once");
    assert_eq!(
        fs::read(resolve_asset_path(&root, &one)).unwrap(),
        vec![3u8; 1000]
    );

    let probed = import_asset_with(&root, &c, |p| {
        assert!(p.exists());
        Some(AudioProbe {
            sample_rate: 44_100,
            channels: 2,
            frames: 5,
        })
    })
    .unwrap();
    assert_eq!(probed.path, format!("media/{}", probed.hash));
    assert_eq!(
        (probed.sample_rate, probed.channels, probed.frames),
        (44_100, 2, 5)
    );
}

#[test]
fn resolve_asset_path_never_leaves_the_data_directory() {
    let root = Path::new("/projects/Song.peelproj");
    let mut asset = peel_document::AudioAsset {
        id: Id::new(),
        hash: "0".repeat(64),
        name: "x".into(),
        path: "media/ok.wav".into(),
        managed: true,
        sample_rate: 0,
        channels: 0,
        frames: 0,
    };
    assert_eq!(
        resolve_asset_path(root, &asset),
        Path::new("/projects/Song.peeldata/media/ok.wav")
    );
    for bad in ["../x.wav", "/etc/passwd", "media/../../x", "C:\\x.wav"] {
        asset.path = bad.into();
        let resolved = resolve_asset_path(root, &asset);
        assert!(
            resolved.starts_with("/projects/Song.peeldata"),
            "{bad} -> {resolved:?}"
        );
        assert!(!resolved.exists());
    }
    asset.managed = false;
    asset.path = "/abs/loop.wav".into();
    assert_eq!(resolve_asset_path(root, &asset), Path::new("/abs/loop.wav"));
    asset.path = "samples/loop.wav".into();
    assert_eq!(
        resolve_asset_path(root, &asset),
        Path::new("/projects/samples/loop.wav")
    );
}

#[test]
fn missing_media_is_a_warning_not_an_error() {
    let dir = tempdir().unwrap();
    let (project, root) = project_with_media(dir.path());
    let report = save(&project, &root).unwrap();
    assert!(report.missing_media.is_empty());
    fs::remove_dir_all(data_dir_for(&root).join("media")).unwrap();

    let loaded = load(&root).unwrap();
    assert!(
        loaded.warnings.iter().any(|w| w.contains("media")),
        "{:?}",
        loaded.warnings
    );
    assert_eq!(save(&project, &root).unwrap().missing_media.len(), 1);
}

#[test]
fn save_as_brings_managed_media_along() {
    let dir = tempdir().unwrap();
    let (project, root) = project_with_media(dir.path());
    save(&project, &root).unwrap();

    let copy = dir.path().join("elsewhere").join("Copy.peelproj");
    let report = save_as(&project, Some(&root), &copy).unwrap();
    assert!(report.missing_media.is_empty());
    let asset = project.assets.iter().find(|a| a.managed).unwrap();
    assert_eq!(
        fs::read(resolve_asset_path(&copy, asset)).unwrap(),
        fs::read(resolve_asset_path(&root, asset)).unwrap()
    );
    assert_eq!(load(&copy).unwrap().project, project);
}

#[test]
fn notes_with_awkward_times_round_trip() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    let mut project = rich_project(None);
    let ch = project.channels[0].id;
    let pat = project.patterns[1].id;
    for (n, d) in [(1, 7), (-1, 3), (22, 7), (1, 15_360), (123_456_789, 960)] {
        project.pattern_mut(pat).unwrap().notes.push(Note::new(
            ch,
            Time::new(n, d),
            Time::new(1, 15_360),
            60,
            0.5,
        ));
    }
    // Negative starts are not valid musical positions for validation? They
    // are allowed by the model; keep the sorted-by-start invariant.
    project.pattern_mut(pat).unwrap().sort_notes();
    save(&project, &root).unwrap();
    assert_eq!(load(&root).unwrap().project, project);
}

#[test]
fn recorded_note_expression_survives_sharded_save_load_and_undo() {
    use peel_document::{Document, ExpressionPoint, NoteExpression};
    let dir = tempdir().unwrap();
    let path = dir.path().join("Expression.peelproj");
    let mut p = rich_project(None);
    let pid = p.patterns[0].id;
    let note = p.patterns[0].notes[0];
    let curve = NoteExpression {
        note: note.id,
        pitch: vec![
            ExpressionPoint {
                offset: Time::ZERO,
                value: 0.0,
            },
            ExpressionPoint {
                offset: note.length.checked_div(Time::beats(2)).unwrap(),
                value: 7.25,
            },
        ],
        pressure: vec![ExpressionPoint {
            offset: Time::ZERO,
            value: 0.4,
        }],
        timbre: vec![ExpressionPoint {
            offset: Time::ZERO,
            value: 0.75,
        }],
    };
    p.pattern_mut(pid).unwrap().expression.push(curve);
    save(&p, &path).unwrap();
    let mut doc = Document::new(load(&path).unwrap().project);
    assert_eq!(
        doc.project().patterns[0].expression,
        p.patterns[0].expression
    );
    doc.edit("Remove recorded expression", |p| {
        p.pattern_mut(pid).unwrap().expression.clear()
    });
    assert!(doc.undo().is_some());
    assert_eq!(
        doc.project().patterns[0].expression,
        p.patterns[0].expression
    );
    assert!(doc.redo().is_some());
    assert!(doc.project().patterns[0].expression.is_empty());
}
