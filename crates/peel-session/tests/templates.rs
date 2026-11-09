//! Projects started from a user template: media read from the template
//! until the first save, which brings it along.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use peel_document::Id;
use peel_session::{Session, template};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("peel-session-{tag}-{}", Id::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.clone(), std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// A saved project whose sampler plays a managed sample.
fn saved_with_media(dir: &Path) -> Session {
    let song = dir.join("Song").join("Song.peelproj");
    std::fs::create_dir_all(song.parent().unwrap()).unwrap();
    let wav = dir.join("hit.wav");
    std::fs::write(&wav, vec![3u8; 2048]).unwrap();
    let asset = peel_project::import_asset(&song, &wav).unwrap();
    let mut project = template::beat_and_melody();
    project.channels[0].device.sample = Some(asset.id);
    project.assets.push(asset);
    let mut s = Session::new_silent(project);
    s.save(Some(song)).unwrap();
    std::fs::remove_file(wav).unwrap();
    s
}

#[test]
fn saving_a_template_leaves_the_open_project_alone() {
    let dir = temp_dir("template-untouched");
    let mut s = saved_with_media(&dir);
    s.doc.edit("Rename project", |p| {
        p.meta.title = "Work in progress".into()
    });
    let path = s.path.clone();
    let revision = s.doc.revision();
    let before = files(&dir.join("Song"));

    s.save_template(&dir.join("Templates"), "Starter").unwrap();
    assert_eq!(s.path, path);
    assert_eq!(s.project().meta.title, "Work in progress");
    assert_eq!(s.doc.revision(), revision);
    assert!(s.doc.is_dirty(), "unsaved work stays unsaved");
    assert_eq!(s.doc.undo_label(), Some("Rename project"));
    assert_eq!(files(&dir.join("Song")), before, "no file of it changes");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_template_project_reads_media_from_the_template_until_saved() {
    let dir = temp_dir("template-media");
    let s = saved_with_media(&dir);
    let lib = dir.join("Templates");
    let tpl = s.save_template(&lib, "Hits").unwrap();
    drop(s);
    // The source project is gone; the template has the sample.
    std::fs::remove_dir_all(dir.join("Song")).unwrap();
    let library = files(&lib);

    let loaded = peel_project::load_template(&tpl).unwrap().project;
    let mut s = Session::new_silent(template::empty());
    s.replace_project_with_media(loaded, Some(tpl.clone()));
    assert!(s.path.is_none());
    assert!(!s.doc.is_dirty());
    assert!(s.doc.undo_label().is_none());
    assert_eq!(s.media_source(), Some(tpl.as_path()));
    let asset = s.project().assets[0].clone();
    assert!(
        s.asset_path(&asset)
            .starts_with(peel_project::data_dir_for(&tpl))
    );
    assert!(s.asset_path(&asset).is_file());

    // Save needs a file of its own; the first save copies the media there.
    assert!(s.save(None).is_err(), "never written back to the template");
    let mine = dir.join("Mine").join("Copy.peelproj");
    s.save(Some(mine.clone())).unwrap();
    assert_eq!(s.media_source(), Some(mine.as_path()));
    assert_eq!(s.project().meta.title, "Copy");
    assert_eq!(files(&lib), library, "the template is unchanged");

    std::fs::remove_dir_all(&lib).unwrap();
    let reopened = peel_project::load(&mine).unwrap();
    assert!(reopened.warnings.is_empty(), "{:?}", reopened.warnings);
    assert!(s.asset_path(&asset).is_file());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn save_as_brings_managed_media_to_the_new_file() {
    let dir = temp_dir("save-as-media");
    let mut s = saved_with_media(&dir);
    let moved = dir.join("Elsewhere").join("Moved.peelproj");
    s.save(Some(moved.clone())).unwrap();
    std::fs::remove_dir_all(dir.join("Song")).unwrap();
    let reopened = peel_project::load(&moved).unwrap();
    assert!(reopened.warnings.is_empty(), "{:?}", reopened.warnings);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn recorded_instrument_templates_need_no_external_sample_files() {
    let dir = temp_dir("recorded-kit-template");
    let session = Session::new_silent(template::rock_metal());
    let template = session
        .save_template(&dir.join("Library"), "My metal kit")
        .unwrap();
    let loaded = peel_project::load_template(&template).unwrap();
    assert!(loaded.project.assets.is_empty());
    assert!(loaded.warnings.is_empty());
    for (name, piece) in [
        ("Kick", 1.0),
        ("Snare", 2.0),
        ("Hat", 3.0),
        ("Crash", 6.0),
        ("Toms", 8.0),
    ] {
        let channel = loaded
            .project
            .channels
            .iter()
            .find(|c| c.name == name)
            .unwrap();
        assert_eq!(channel.device.type_id, "peel.drum.acoustic");
        assert_eq!(channel.device.params.get("kit.piece"), Some(&piece));
        assert!(channel.device.sample.is_none());
        assert!(peel_devices::create(&channel.device.type_id).is_some());
    }
    for (name, kind, preset, root) in [
        ("Guitar", "peel.guitar.recorded", "High Gain", 40),
        ("Chug", "peel.guitar.recorded", "Palm Mute", 40),
        ("Bass", "peel.bass.recorded", "Rock Pick", 28),
    ] {
        let channel = loaded
            .project
            .channels
            .iter()
            .find(|c| c.name == name)
            .unwrap();
        assert_eq!(channel.device.type_id, kind);
        assert_eq!(channel.device.preset.as_deref(), Some(preset));
        assert_eq!(channel.root_key, root);
        assert!(channel.device.sample.is_none());
        assert!(peel_devices::create(kind).is_some());
    }
    assert!(
        loaded
            .project
            .patterns
            .iter()
            .any(|p| p.name == "Double kick")
    );
    assert!(
        files(&dir)
            .keys()
            .all(|p| p.extension().is_none_or(|e| e != "wav")),
        "embedded recordings aren't copied into every personal template"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
