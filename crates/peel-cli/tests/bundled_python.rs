//! Release/CI verification with no system Python or package installation.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        // Copy through symlinks: the test installation is independent of the original.
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "requires scripts/bundle-python.py target/debug/python; CI runs this explicitly"]
fn pytheory_works_from_a_relocated_package_with_no_python_on_path() {
    let binary = Path::new(env!("CARGO_BIN_EXE_peel"));
    let runtime = binary.parent().unwrap().join("python");
    assert!(
        runtime.join("peel-python.json").is_file(),
        "Bundle Python beside the CLI before running this test"
    );
    let scratch = Scratch(
        std::env::temp_dir().join(format!("Peel portable café {}", peel_document::Id::new())),
    );
    std::fs::create_dir_all(&scratch.0).unwrap();
    let installed = scratch.0.join(binary.file_name().unwrap());
    std::fs::copy(binary, &installed).unwrap();
    copy_tree(&runtime, &scratch.0.join("python"));
    let before = snapshot(&scratch.0.join("python"));
    let source = scratch.0.join("chord.py");
    std::fs::write(
        &source,
        include_str!("../../../assets/scripts/pytheory_chord.py"),
    )
    .unwrap();
    let result = scratch.0.join("result.peelproj");
    let mut child = Command::new(installed)
        .arg("script")
        .arg("--demo")
        .arg(source)
        .arg(&result)
        .env("PATH", "")
        .env_remove("PEEL_PYTHON")
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .current_dir(&scratch.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Bundled Python did not finish");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Added a C major triad using PyTheory")
    );
    let project = peel_project::load(&result).unwrap().project;
    let pattern = &project.patterns[0];
    for key in [60, 64, 67] {
        assert!(
            pattern
                .notes
                .iter()
                .any(|n| n.channel == project.channels[0].id
                    && n.key == key
                    && n.start == peel_document::Time::ZERO)
        );
    }
    assert_eq!(
        snapshot(&scratch.0.join("python")),
        before,
        "Running scripts must not modify a signed bundle"
    );
}

fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
    let mut files = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_dir() {
            files.extend(snapshot(&entry.path()));
        } else {
            let metadata = entry.metadata().unwrap();
            files.insert(entry.path(), (metadata.len(), metadata.modified().unwrap()));
        }
    }
    files
}
