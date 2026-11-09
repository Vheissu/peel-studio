mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use peel_document::{ExpressionPoint, NoteExpression, Project, Time};
use peel_project::{
    TemplateError, data_dir_for, import_asset, list_templates, load, load_template,
    resolve_asset_path, save, save_as, save_template,
};
use tempfile::tempdir;

use common::{fake_audio, rich_project};

/// A saved source project with a managed sample, a real external loop,
/// plugin state, routing and recorded expression.
fn source(dir: &Path) -> (Project, PathBuf, PathBuf) {
    let root = dir.join("Source").join("Song.peelproj");
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    let kick = fake_audio(dir, "kick.wav", 7, 4096);
    let managed = import_asset(&root, &kick).unwrap();
    let mut p = rich_project(Some(managed));
    let loop_file = fake_audio(dir, "external loop.wav", 9, 2048);
    let ext = p.assets.iter_mut().find(|a| !a.managed).unwrap();
    ext.path = loop_file.to_string_lossy().into_owned();
    ext.hash = blake3::hash(&fs::read(&loop_file).unwrap())
        .to_hex()
        .to_string();
    p.channels[1].device.state = Some("cGx1Z2luIHN0YXRl".into());
    let note = p.patterns[0].notes[0].id;
    std::sync::Arc::make_mut(&mut p.patterns[0])
        .expression
        .push(NoteExpression {
            note,
            pitch: vec![
                ExpressionPoint {
                    offset: Time::ZERO,
                    value: 0.0,
                },
                ExpressionPoint {
                    offset: Time::new(1, 3),
                    value: 1.5,
                },
            ],
            pressure: vec![ExpressionPoint {
                offset: Time::new(1, 8),
                value: 0.7,
            }],
            timbre: Vec::new(),
        });
    save(&p, &root).unwrap();
    (p, root, loop_file)
}

/// Every file under `dir` with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.clone(), fs::read(&p).unwrap());
            }
        }
    }
    out
}

fn without_identity(mut p: Project) -> Project {
    p.id = peel_document::Id::NIL;
    p.meta.title.clear();
    p.revision = 0;
    // Saving recorded expression raises the schema; that is the format.
    p.schema = 0;
    for a in &mut p.assets {
        a.path.clear();
        a.managed = false;
    }
    p
}

#[test]
fn a_template_is_a_complete_independent_copy() {
    let dir = tempdir().unwrap();
    let (project, root, loop_file) = source(dir.path());
    let lib = dir.path().join("Templates");
    let before = snapshot(root.parent().unwrap());

    let saved = save_template(&project, Some(&root), &lib, " Night drive ").unwrap();
    assert_eq!(saved, lib.join("Night drive.peelproj"));
    assert_eq!(
        snapshot(root.parent().unwrap()),
        before,
        "the source project is untouched"
    );
    // Nothing but the template itself: no staging left behind.
    let names: Vec<_> = fs::read_dir(&lib)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");

    // The source and its external file go away; the template still has
    // everything, the external loop now managed inside it.
    fs::remove_dir_all(root.parent().unwrap()).unwrap();
    fs::remove_file(&loop_file).unwrap();
    let loaded = load_template(&saved).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    let t = loaded.project;
    assert_ne!(t.id, project.id);
    assert_eq!(t.meta.title, "Untitled");
    assert!(t.assets.iter().all(|a| a.managed));
    for a in &t.assets {
        let file = resolve_asset_path(&saved, a);
        let bytes = fs::read(&file).unwrap();
        assert_eq!(blake3::hash(&bytes).to_hex().to_string(), a.hash);
    }
    // Instruments, effects, plugin state, routing, patterns and
    // expression come back exactly.
    assert_eq!(
        without_identity(t.clone()),
        without_identity(project.clone())
    );
    assert_eq!(
        t.channels[1].device.state.as_deref(),
        Some("cGx1Z2luIHN0YXRl")
    );
    assert!(!t.patterns[0].expression.is_empty());

    // Loading again gives another id; the template file never changes.
    let library = snapshot(&lib);
    assert_ne!(load_template(&saved).unwrap().project.id, t.id);
    assert_eq!(snapshot(&lib), library);

    // Saving the new project somewhere else brings the media along, so it
    // survives the template going away too.
    let song = dir.path().join("Mine").join("Song.peelproj");
    save_as(&t, Some(&saved), &song).unwrap();
    assert_eq!(snapshot(&lib), library, "the template is not written");
    fs::remove_dir_all(&lib).unwrap();
    let reopened = load(&song).unwrap();
    assert!(reopened.warnings.is_empty(), "{:?}", reopened.warnings);
    for a in &reopened.project.assets {
        assert!(resolve_asset_path(&song, a).is_file(), "{}", a.name);
    }
}

#[test]
fn names_are_never_reused_or_used_as_paths() {
    let dir = tempdir().unwrap();
    let (project, root, _) = source(dir.path());
    let lib = dir.path().join("Templates");
    let first = save_template(&project, Some(&root), &lib, "Kit").unwrap();
    let library = snapshot(&lib);

    for dup in ["Kit", "kit", " KIT "] {
        let err = save_template(&project, Some(&root), &lib, dup).unwrap_err();
        assert!(matches!(err, TemplateError::Exists(_)), "{dup}: {err}");
        assert_eq!(
            err.to_string(),
            format!("A template named {} already exists", dup.trim())
        );
    }
    for bad in ["", "  ", "../Escape", "a/b", "..", ".hidden", "x\\y", "CON"] {
        assert!(
            save_template(&project, Some(&root), &lib, bad).is_err(),
            "{bad:?}"
        );
    }
    assert_eq!(snapshot(&lib), library, "nothing written or replaced");
    assert!(!dir.path().join("Escape.peelproj").exists());
    assert_eq!(list_templates(&lib).len(), 1);
    assert!(load(&first).is_ok());
}

#[test]
fn missing_media_fails_without_a_library_entry() {
    let dir = tempdir().unwrap();
    let (project, root, loop_file) = source(dir.path());
    let lib = dir.path().join("Templates");
    fs::remove_file(&loop_file).unwrap();
    let err = save_template(&project, Some(&root), &lib, "Broken").unwrap_err();
    assert!(
        matches!(&err, TemplateError::MissingMedia(names) if names == &["external loop.wav".to_string()]),
        "{err}"
    );
    assert!(
        err.to_string()
            .starts_with("Missing audio: external loop.wav")
    );
    assert!(list_templates(&lib).is_empty());
    assert_eq!(fs::read_dir(&lib).unwrap().count(), 0, "no staging left");

    // Managed media with nowhere to read it from is missing too.
    let err = save_template(&project, None, &lib, "Broken").unwrap_err();
    assert!(matches!(err, TemplateError::MissingMedia(_)), "{err}");
}

#[test]
fn the_library_lists_what_loads_and_skips_the_rest() {
    let dir = tempdir().unwrap();
    let lib = dir.path().join("Templates");
    assert!(list_templates(&lib).is_empty(), "no folder yet");
    let blank = Project::empty("Blank");
    for name in ["beta", "Alpha", "gamma"] {
        save_template(&blank, None, &lib, name).unwrap();
    }
    // Debris: a corrupt root, a hidden file, a folder and an orphaned
    // data folder from an interrupted save.
    fs::write(lib.join("Corrupt.peelproj"), b"not json").unwrap();
    fs::write(lib.join(".hidden.peelproj"), b"{}").unwrap();
    fs::create_dir_all(lib.join("Folder.peelproj")).unwrap();
    fs::create_dir_all(lib.join("Orphan.peeldata").join("objects")).unwrap();

    // A fresh listing (as after a restart) finds them in name order.
    let names: Vec<_> = list_templates(&lib).into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["Alpha", "beta", "gamma"]);

    // A root whose objects were damaged lists, but does not load.
    let gamma = lib.join("gamma.peelproj");
    fs::remove_dir_all(data_dir_for(&gamma).join("objects")).unwrap();
    assert!(load_template(&gamma).is_err());

    // A data folder without a root (an interrupted save, or another save
    // mid-commit) is never deleted or reused: its name is unavailable.
    let orphan = lib.join("Orphan.peeldata");
    fs::write(orphan.join("objects").join("keep.json"), b"keep").unwrap();
    for name in ["Orphan", "orphan"] {
        let err = save_template(&blank, None, &lib, name).unwrap_err();
        assert!(matches!(err, TemplateError::Exists(_)), "{name}: {err}");
    }
    assert_eq!(
        fs::read(orphan.join("objects").join("keep.json")).unwrap(),
        b"keep"
    );
    assert!(!lib.join("Orphan.peelproj").exists());
    assert!(!lib.join("orphan.peelproj").exists());
}

#[test]
fn a_template_missing_its_audio_does_not_load() {
    let dir = tempdir().unwrap();
    let (project, root, loop_file) = source(dir.path());
    let lib = dir.path().join("Templates");
    let saved = save_template(&project, Some(&root), &lib, "Kit").unwrap();
    // Self-contained: the source going away changes nothing.
    fs::remove_dir_all(root.parent().unwrap()).unwrap();
    fs::remove_file(&loop_file).unwrap();
    assert!(load_template(&saved).is_ok());

    // Its own audio going away is an error naming the files, not a warning.
    let media = data_dir_for(&saved).join("media");
    let first = fs::read_dir(&media)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_file(&first).unwrap();
    let err = load_template(&saved).unwrap_err();
    match &err {
        TemplateError::IncompleteTemplate(names) => assert_eq!(names.len(), 1, "{names:?}"),
        other => panic!("{other}"),
    }
    assert!(
        err.to_string()
            .starts_with("This template is missing audio: "),
        "{err}"
    );
    assert!(load(&saved).is_ok(), "a plain load only warns");
}
