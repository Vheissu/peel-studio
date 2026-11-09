//! Exercise the real CLI and its isolated runtime, including native VM stalls.
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("peel-script-cli-{}", peel_document::Id::new()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn execute(script: &str, extension: &str) -> (std::process::Output, Scratch) {
    let scratch = Scratch::new();
    let source = scratch.0.join(format!("action.{extension}"));
    std::fs::write(&source, script).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_peel"))
        .arg("script")
        .arg("--demo")
        .arg(&source)
        .arg(scratch.0.join("result.peelproj"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(8) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("CLI failed to enforce the script deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    (child.wait_with_output().unwrap(), scratch)
}

#[test]
fn isolated_javascript_and_lua_apply_valid_edits() {
    for (extension, source) in [
        ("js", "peel.project.meta.title='Scripted'; peel.log('ok');"),
        ("lua", "peel.project.meta.title='Scripted'; peel.log('ok')"),
    ] {
        let (output, scratch) = execute(source, extension);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("ok"));
        let loaded = peel_project::load(&scratch.0.join("result.peelproj")).unwrap();
        assert_eq!(loaded.project.meta.title, "Scripted");
    }
}

#[test]
fn native_library_stalls_are_killed_without_saving() {
    for (extension, source) in [
        ("lua", "string.find(string.rep('a',100000), '.-.-.-.-.-b')"),
        ("js", "Array.prototype.indexOf.call({length: 2**53-1}, 1);"),
    ] {
        let (output, scratch) = execute(source, extension);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("time limit"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!scratch.0.join("result.peelproj").exists());
    }
}
