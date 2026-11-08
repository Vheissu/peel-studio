//! Advisory project lock: `Song.peeldata/lock`.
//!
//! One writable session owns a project. The lock file records the owner's
//! process id, host name and acquisition time. Another instance sees the
//! lock and opens read-only (or branches); a lock whose owner process no
//! longer exists on this machine is stale and can be taken over.
//!
//! Limits, all deliberate:
//!
//! * The lock is advisory. [`save`](crate::save) does not check it.
//! * Liveness is only decidable for processes on the same host. A lock from
//!   another host is reported as [`LockStatus::HeldElsewhere`] and never
//!   treated as stale automatically (network shares make that unsafe); the
//!   user can override with [`ProjectLock::acquire_breaking`].
//! * Liveness uses `std` only, so it asks the operating system through a
//!   tiny subprocess (`/proc` on Linux, `kill -0` on other Unix, `tasklist`
//!   on Windows). If that cannot be determined the lock counts as held.
//! * A recycled process id can make a dead owner look alive. That fails
//!   safe: the user is asked rather than a live session being overridden.
//! * Network filesystems without working hard links or atomic exclusive
//!   create can allow two sessions to race. The adapter for such shares
//!   should require a local working copy.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use peel_document::Id;
use serde::{Deserialize, Serialize};

use crate::error::{ProjectError, Result, io_at};
use crate::media::data_dir_for;
use crate::util::{create_dir_all, sync_dir, temp_sibling, utc_timestamp};

/// Who holds a lock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockInfo {
    pub pid: u32,
    pub hostname: String,
    /// RFC 3339 UTC time the lock was taken.
    pub acquired_at: String,
    /// Random value identifying this particular acquisition.
    pub token: String,
}

impl fmt::Display for LockInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "process {} on {} since {}",
            self.pid, self.hostname, self.acquired_at
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockStatus {
    /// No lock file.
    Free,
    /// Held by a live process on this machine.
    Held(LockInfo),
    /// Held by a process on another machine; liveness cannot be checked.
    HeldElsewhere(LockInfo),
    /// Written by a process on this machine that no longer exists.
    Stale(LockInfo),
    /// A lock file exists but cannot be understood.
    Unreadable,
}

/// A held lock. Dropping it releases the lock if it is still ours.
#[derive(Debug)]
pub struct ProjectLock {
    path: PathBuf,
    info: LockInfo,
    released: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Liveness {
    Alive,
    Dead,
    Unknown,
}

pub(crate) fn lock_path(project_path: &Path) -> PathBuf {
    data_dir_for(project_path).join("lock")
}

impl ProjectLock {
    /// Takes the lock, replacing a stale one. Fails with
    /// [`ProjectError::Locked`] when a live or unverifiable owner holds it.
    pub fn acquire(project_path: &Path) -> Result<ProjectLock> {
        Self::acquire_inner(project_path, false)
    }

    /// Takes the lock whatever its state: for a lock the user has confirmed
    /// belongs to a dead session on another machine, or to a crashed one.
    pub fn acquire_breaking(project_path: &Path) -> Result<ProjectLock> {
        Self::acquire_inner(project_path, true)
    }

    /// Inspects the lock without taking it.
    pub fn status(project_path: &Path) -> Result<LockStatus> {
        let path = lock_path(project_path);
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(LockStatus::Free),
            Err(e) => return Err(io_at(&path)(e)),
        };
        let Ok(info) = serde_json::from_slice::<LockInfo>(&bytes) else {
            return Ok(LockStatus::Unreadable);
        };
        if info.hostname != hostname() {
            return Ok(LockStatus::HeldElsewhere(info));
        }
        Ok(match process_liveness(info.pid) {
            Liveness::Dead => LockStatus::Stale(info),
            Liveness::Alive | Liveness::Unknown => LockStatus::Held(info),
        })
    }

    pub fn info(&self) -> &LockInfo {
        &self.info
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Releases the lock now, reporting any failure.
    pub fn release(mut self) -> Result<()> {
        self.released = true;
        self.remove_if_ours()
    }

    fn remove_if_ours(&self) -> Result<()> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                if serde_json::from_slice::<LockInfo>(&bytes)
                    .is_ok_and(|held| held.token == self.info.token)
                {
                    fs::remove_file(&self.path).map_err(io_at(&self.path))?;
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_at(&self.path)(e)),
        }
    }

    fn acquire_inner(project_path: &Path, force: bool) -> Result<ProjectLock> {
        let data_dir = data_dir_for(project_path);
        create_dir_all(&data_dir)?;
        let path = lock_path(project_path);
        let info = LockInfo {
            pid: std::process::id(),
            hostname: hostname(),
            acquired_at: utc_timestamp(SystemTime::now()),
            token: Id::new().to_string(),
        };
        let body = serde_json::to_vec_pretty(&info).expect("lock info serializes");

        // A few rounds: create, or inspect the existing lock and retry once
        // a stale or forced one has been moved aside.
        for _ in 0..4 {
            match create_exclusive(&path, &body) {
                Ok(()) => {
                    sync_dir(&data_dir)?;
                    return Ok(ProjectLock {
                        path,
                        info,
                        released: false,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_at(&path)(e)),
            }
            let observed = fs::read(&path).ok();
            let holder = observed
                .as_deref()
                .and_then(|b| serde_json::from_slice::<LockInfo>(b).ok());
            let takeover = force
                || match Self::status(project_path)? {
                    LockStatus::Free => continue,
                    LockStatus::Stale(_) => true,
                    LockStatus::Held(i) | LockStatus::HeldElsewhere(i) => {
                        return Err(ProjectError::Locked(i));
                    }
                    LockStatus::Unreadable => {
                        return Err(ProjectError::Locked(LockInfo {
                            pid: 0,
                            hostname: "an unknown session".into(),
                            acquired_at: "an unknown time".into(),
                            token: String::new(),
                        }));
                    }
                };
            if takeover {
                move_aside(&path, observed.as_deref(), holder.as_ref())?;
            }
        }
        Err(ProjectError::Io {
            path,
            source: io::Error::new(io::ErrorKind::WouldBlock, "the project lock is contended"),
        })
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.remove_if_ours();
        }
    }
}

/// Atomically creates `path` containing `body`, failing with `AlreadyExists`
/// if it is there. The content is written to a temp file first and hard
/// linked into place, so no reader ever sees an empty lock; filesystems
/// without hard links fall back to exclusive create.
fn create_exclusive(path: &Path, body: &[u8]) -> io::Result<()> {
    let tmp = temp_sibling(path);
    let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let written = f.write_all(body).and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    let linked = fs::hard_link(&tmp, path);
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
            f.write_all(body)?;
            f.sync_all()
        }
    }
}

/// Removes a lock we judged stale. The file is renamed away first, then read
/// back: if it is no longer the lock we examined (someone else took over in
/// the meantime) it is put back instead of deleted.
fn move_aside(path: &Path, observed: Option<&[u8]>, holder: Option<&LockInfo>) -> Result<()> {
    let aside = temp_sibling(path);
    match fs::rename(path, &aside) {
        Ok(()) => {}
        // Someone else already removed it.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_at(path)(e)),
    }
    let moved = fs::read(&aside).ok();
    let same = match (observed, moved.as_deref()) {
        (Some(a), Some(b)) => a == b,
        (None, _) => holder.is_none(),
        _ => false,
    };
    if same {
        let _ = fs::remove_file(&aside);
    } else if fs::hard_link(&aside, path).is_ok() {
        let _ = fs::remove_file(&aside);
        return Err(ProjectError::Locked(
            moved
                .as_deref()
                .and_then(|b| serde_json::from_slice::<LockInfo>(b).ok())
                .unwrap_or(LockInfo {
                    pid: 0,
                    hostname: "another session".into(),
                    acquired_at: "just now".into(),
                    token: String::new(),
                }),
        ));
    } else {
        // A new lock already exists; ours was only a stale leftover.
        let _ = fs::remove_file(&aside);
    }
    Ok(())
}

/// This machine's name, from sources that agree between a terminal-launched
/// and a desktop-launched process. Falls back to `"unknown-host"`.
pub(crate) fn hostname() -> String {
    #[cfg(windows)]
    {
        if let Ok(name) = std::env::var("COMPUTERNAME")
            && !name.trim().is_empty()
        {
            return name.trim().to_string();
        }
    }
    for file in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(text) = fs::read_to_string(file) {
            let name = text.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    if let Ok(out) = Command::new("hostname").output() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && !name.is_empty() {
            return name;
        }
    }
    for var in ["HOSTNAME", "COMPUTERNAME", "HOST"] {
        if let Ok(name) = std::env::var(var)
            && !name.trim().is_empty()
        {
            return name.trim().to_string();
        }
    }
    "unknown-host".to_string()
}

fn process_liveness(pid: u32) -> Liveness {
    if pid == std::process::id() {
        return Liveness::Alive;
    }
    probe_process(pid)
}

#[cfg(unix)]
fn probe_process(pid: u32) -> Liveness {
    if cfg!(target_os = "linux") && Path::new("/proc/self").exists() {
        return if Path::new("/proc").join(pid.to_string()).exists() {
            Liveness::Alive
        } else {
            Liveness::Dead
        };
    }
    // `kill -0` sends no signal; it only reports whether the process exists.
    // EPERM (exists, owned by someone else) still means alive.
    match Command::new("kill").args(["-0", &pid.to_string()]).output() {
        Ok(out) if out.status.success() => Liveness::Alive,
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
            if err.contains("not permitted") || err.contains("permission") {
                Liveness::Alive
            } else if err.contains("no such process") || err.contains("not found") {
                Liveness::Dead
            } else {
                Liveness::Unknown
            }
        }
        Err(_) => Liveness::Unknown,
    }
}

#[cfg(windows)]
fn probe_process(pid: u32) -> Liveness {
    match Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
    {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.contains(&format!("\"{pid}\"")) {
                Liveness::Alive
            } else {
                Liveness::Dead
            }
        }
        _ => Liveness::Unknown,
    }
}

#[cfg(not(any(unix, windows)))]
fn probe_process(_pid: u32) -> Liveness {
    Liveness::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        assert_eq!(process_liveness(std::process::id()), Liveness::Alive);
    }

    #[test]
    fn hostname_is_not_empty() {
        assert!(!hostname().is_empty());
    }
}
