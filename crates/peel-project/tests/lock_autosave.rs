mod common;

use std::fs;
use std::process::Command;

use peel_document::Project;
use peel_project::{
    LockInfo, LockStatus, ProjectError, ProjectLock, autosave, autosave_from, autosave_path,
    data_dir_for, discard_autosave, import_asset, list_autosaves, load, resolve_asset_path, save,
};
use tempfile::tempdir;

use common::{fake_audio, rich_project};

/// A pid that is certain to have exited.
fn dead_pid() -> u32 {
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .arg("--list")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn this_host() -> String {
    let dir = tempdir().unwrap();
    let lock = ProjectLock::acquire(&dir.path().join("p.peelproj")).unwrap();
    lock.info().hostname.clone()
}

fn plant_lock(root: &std::path::Path, pid: u32, hostname: &str) {
    fs::create_dir_all(data_dir_for(root)).unwrap();
    let info = LockInfo {
        pid,
        hostname: hostname.to_string(),
        acquired_at: "2026-01-01T00:00:00.000Z".into(),
        token: "planted".into(),
    };
    fs::write(
        data_dir_for(root).join("lock"),
        serde_json::to_vec(&info).unwrap(),
    )
    .unwrap();
}

#[test]
fn lock_is_exclusive_and_released_on_drop() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    assert_eq!(ProjectLock::status(&root).unwrap(), LockStatus::Free);

    let lock = ProjectLock::acquire(&root).unwrap();
    assert_eq!(lock.info().pid, std::process::id());
    assert!(lock.path().is_file());
    assert!(matches!(
        ProjectLock::status(&root).unwrap(),
        LockStatus::Held(i) if i.pid == std::process::id()
    ));

    match ProjectLock::acquire(&root) {
        Err(ProjectError::Locked(info)) => assert_eq!(info.pid, std::process::id()),
        other => panic!("second acquire succeeded: {other:?}"),
    }
    // Failing to acquire must not disturb the holder.
    assert!(lock.path().is_file());

    drop(lock);
    assert_eq!(ProjectLock::status(&root).unwrap(), LockStatus::Free);
    let again = ProjectLock::acquire(&root).unwrap();
    again.release().unwrap();
    assert_eq!(ProjectLock::status(&root).unwrap(), LockStatus::Free);
}

#[test]
fn lock_file_records_owner() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    let lock = ProjectLock::acquire(&root).unwrap();
    let text = fs::read_to_string(data_dir_for(&root).join("lock")).unwrap();
    let info: LockInfo = serde_json::from_str(&text).unwrap();
    assert_eq!(&info, lock.info());
    assert!(!info.hostname.is_empty());
    assert!(info.acquired_at.ends_with('Z'));
    assert!(!info.token.is_empty());
}

#[test]
fn stale_lock_from_a_dead_process_is_taken_over() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    let pid = dead_pid();
    plant_lock(&root, pid, &this_host());

    assert!(matches!(
        ProjectLock::status(&root).unwrap(),
        LockStatus::Stale(i) if i.pid == pid
    ));
    let lock = ProjectLock::acquire(&root).unwrap();
    assert_eq!(lock.info().pid, std::process::id());
    assert!(matches!(
        ProjectLock::status(&root).unwrap(),
        LockStatus::Held(_)
    ));
    // No debris from moving the stale lock aside.
    let debris: Vec<_> = fs::read_dir(data_dir_for(&root))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "lock")
        .collect();
    assert!(debris.is_empty(), "{debris:?}");
}

#[test]
fn lock_from_another_host_is_never_assumed_stale() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    plant_lock(&root, dead_pid(), "some-other-machine.local");

    assert!(matches!(
        ProjectLock::status(&root).unwrap(),
        LockStatus::HeldElsewhere(i) if i.hostname == "some-other-machine.local"
    ));
    assert!(matches!(
        ProjectLock::acquire(&root),
        Err(ProjectError::Locked(_))
    ));
    // The user can confirm and break it.
    let lock = ProjectLock::acquire_breaking(&root).unwrap();
    assert_eq!(lock.info().pid, std::process::id());
}

#[test]
fn live_process_on_this_host_blocks_acquire() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    // A lock owned by a running process (this one) on this host.
    plant_lock(&root, std::process::id(), &this_host());
    assert!(matches!(
        ProjectLock::acquire(&root),
        Err(ProjectError::Locked(_))
    ));
}

#[test]
fn garbled_lock_blocks_until_broken() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    fs::create_dir_all(data_dir_for(&root)).unwrap();
    fs::write(data_dir_for(&root).join("lock"), b"not json").unwrap();
    assert_eq!(ProjectLock::status(&root).unwrap(), LockStatus::Unreadable);
    assert!(matches!(
        ProjectLock::acquire(&root),
        Err(ProjectError::Locked(_))
    ));
    assert!(ProjectLock::acquire_breaking(&root).is_ok());
}

#[test]
fn dropping_a_lock_never_removes_someone_elses() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    let first = ProjectLock::acquire(&root).unwrap();
    // The lock is broken and re-taken while the first handle is still alive.
    let second = ProjectLock::acquire_breaking(&root).unwrap();
    drop(first);
    assert!(
        matches!(ProjectLock::status(&root).unwrap(), LockStatus::Held(i) if i.token == second.info().token)
    );
}

#[test]
fn autosave_uses_the_normal_save_path_and_lists() {
    let dir = tempdir().unwrap();
    let autosaves = dir.path().join("autosaves");
    assert!(list_autosaves(&autosaves).unwrap().is_empty());

    let mut a = Project::empty("Alpha");
    a.revision = 3;
    let mut b = rich_project(None);
    b.meta.title = "Beta".into();

    let path_a = autosave(&a, &autosaves).unwrap();
    assert_eq!(path_a, autosave_path(&autosaves, a.id));
    assert_eq!(
        path_a,
        autosaves.join(a.id.to_string()).join("autosave.peelproj")
    );
    std::thread::sleep(std::time::Duration::from_millis(5));
    let path_b = autosave(&b, &autosaves).unwrap();

    assert_eq!(load(&path_a).unwrap().project, a);
    assert_eq!(load(&path_b).unwrap().project, b);

    let listed = list_autosaves(&autosaves).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].project_id, b.id, "most recent first");
    assert_eq!(listed[0].title, "Beta");
    assert_eq!(listed[1].project_id, a.id);
    assert_eq!(listed[1].revision, 3);

    // Repeated autosaves keep working, share unchanged shards and stay small.
    a.revision = 4;
    a.meta.title = "Alpha v2".into();
    autosave(&a, &autosaves).unwrap();
    assert_eq!(load(&path_a).unwrap().project, a);
    assert_eq!(list_autosaves(&autosaves).unwrap().len(), 2);

    // Unrelated and damaged directories are ignored.
    fs::create_dir(autosaves.join("not-an-id")).unwrap();
    fs::create_dir(autosaves.join("0".repeat(32))).unwrap();
    assert_eq!(list_autosaves(&autosaves).unwrap().len(), 2);

    discard_autosave(&autosaves, a.id).unwrap();
    discard_autosave(&autosaves, a.id).unwrap();
    assert_eq!(list_autosaves(&autosaves).unwrap().len(), 1);
}

#[test]
fn autosave_from_includes_managed_media() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("Song.peelproj");
    let wav = fake_audio(dir.path(), "kick.wav", 5, 5000);
    let asset = import_asset(&root, &wav).unwrap();
    let project = rich_project(Some(asset.clone()));
    save(&project, &root).unwrap();

    let autosaves = dir.path().join("autosaves");
    let path = autosave_from(&project, &autosaves, &root).unwrap();
    let loaded = load(&path).unwrap();
    assert_eq!(loaded.project, project);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(
        fs::read(resolve_asset_path(&path, &asset)).unwrap(),
        vec![5u8; 5000]
    );

    // Plain autosave of the same project reports the media as missing.
    let bare = dir.path().join("bare");
    let path = autosave(&project, &bare).unwrap();
    assert!(
        load(&path)
            .unwrap()
            .warnings
            .iter()
            .any(|w| w.contains("media"))
    );
}

#[test]
fn autosave_survives_a_corrupt_latest_root() {
    let dir = tempdir().unwrap();
    let autosaves = dir.path().join("autosaves");
    let mut p = Project::empty("Crashy");
    let path = autosave(&p, &autosaves).unwrap();
    p.meta.title = "Crashy 2".into();
    p.revision = 9;
    autosave(&p, &autosaves).unwrap();
    fs::write(&path, b"torn").unwrap();
    let loaded = load(&path).unwrap();
    assert!(loaded.recovered_from_history);
    assert_eq!(loaded.project.meta.title, "Crashy");
}
