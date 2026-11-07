// SPDX-License-Identifier: LicenseRef-SRSL-1.1
//! Shell regressions use Python's standard library and private fake command boundaries.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

struct ScratchDirectory(PathBuf);

impl ScratchDirectory {
    fn new(group: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let path = env::temp_dir().join(format!(
                "tgrep-shell-{group}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create {}: {error}", path.display()),
            }
        }
        panic!("could not allocate a shell-test directory");
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            if std::thread::panicking() {
                eprintln!("remove {}: {error}", self.0.display());
            } else {
                panic!("remove {}: {error}", self.0.display());
            }
        }
    }
}

fn run_group(group: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = ScratchDirectory::new(group);
    // GNU timeout owns the child process group and bounds the complete fixture group.
    let output = Command::new("timeout")
        .args(["--kill-after=2s", "60s", "python3"])
        .arg(root.join("tests/shell_tools.py"))
        .arg("--root")
        .arg(root)
        .arg("--scratch")
        .arg(&scratch.0)
        .arg(group)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .current_dir(&scratch.0)
        .output()
        .expect("run shell fixture");
    assert!(
        output.status.success(),
        "{group}: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn installer_owns_destinations_and_reports_partial_failures() {
    run_group("installer");
}

#[test]
fn compile_cleans_only_the_physical_checkout() {
    run_group("compile");
}

#[test]
fn runner_preserves_paths_results_and_caller_directory() {
    run_group("runner");
}

#[test]
fn packaging_scripts_preserve_physical_checkout_paths() {
    run_group("packaging");
}
