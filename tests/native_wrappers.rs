// SPDX-License-Identifier: LicenseRef-SRSL-1.1
//! Native boundary regressions, discovered by `cargo test --test native_wrappers`.
//!
//! Each fixture links public vendored C sources and receives an owned scratch directory.
//! Set TGREP_NATIVE_TEST_SANITIZERS=address,undefined,float-cast-overflow for sanitizer runs.
//! Run ThreadSanitizer separately, filtering to cache_count_can_run_during_snapshot_load:
//! TGREP_NATIVE_TEST_SANITIZERS=thread cargo test --test native_wrappers cache_count_can_run

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

struct ScratchDirectory(PathBuf);

impl ScratchDirectory {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let path = env::temp_dir().join(format!(
                "tgrep-native-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create {}: {error}", path.display()),
            }
        }
        panic!("could not allocate a unique native-test directory");
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

fn assert_success(stage: &str, output: Output) {
    assert!(
        output.status.success(),
        "{stage}: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_fixture(name: &str, sources: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = ScratchDirectory::new(name);
    let executable = scratch.0.join("fixture");
    let compiler = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let mut command = Command::new(compiler);
    command.args([
        "-std=c11",
        "-D_GNU_SOURCE",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-g",
        "-pthread",
    ]);
    if let Ok(sanitizers) = env::var("TGREP_NATIVE_TEST_SANITIZERS") {
        assert!(
            sanitizers == "thread"
                || (!sanitizers.is_empty()
                    && sanitizers.split(',').all(|name| matches!(
                        name,
                        "address" | "undefined" | "float-cast-overflow"
                    ))),
            "unsupported native-test sanitizer selection"
        );
        command.arg(format!("-fsanitize={sanitizers}"));
        command.arg("-fno-sanitize-recover=all");
    }
    if name == "hash_index" {
        command.arg("-msse4.2");
    }
    command
        .arg("-I")
        .arg(root.join("vendor/KEYSTONE/include"))
        .arg("-I")
        .arg(root.join("vendor/QIHSE/include"))
        .arg(root.join(format!("tests/native/{name}.c")));
    for source in sources {
        command.arg(root.join(source));
    }
    if name == "hash_index" {
        command.arg("-lm");
    }
    command.arg("-o").arg(&executable);
    assert_success(
        "compile native fixture",
        command.output().expect("run C compiler"),
    );
    assert_success(
        name,
        Command::new(executable)
            .arg(&scratch.0)
            .current_dir(&scratch.0)
            .output()
            .expect("run native fixture"),
    );
}

#[test]
fn cache_failures_do_not_become_incomplete_hits() {
    run_fixture("cache", &["vendor/QIHSE/src/tractable/qihse_table_store.c"]);
}

#[test]
fn optimization_loading_and_signature_identity() {
    run_fixture("optimization", &[]);
}

#[test]
fn wal_replay_respects_transaction_incarnations() {
    run_fixture("wal", &["vendor/QIHSE/src/tractable/qihse_wal.c"]);
}

#[test]
fn atomic_write_handles_interruptions_and_zero_progress() {
    run_fixture("atomic_write", &[]);
}

#[test]
fn word_index_publication_preserves_existing_mappings() {
    run_fixture(
        "word_index",
        &[
            "vendor/QIHSE/src/frieze/qihse_btree.c",
            "native/qihse_wrapper.c",
        ],
    );
}

#[test]
fn standalone_btree_test_reports_failure() {
    run_fixture(
        "btree_diagnostic",
        &["vendor/QIHSE/src/frieze/qihse_btree.c"],
    );
}

#[test]
fn cache_count_can_run_during_snapshot_load() {
    run_fixture(
        "cache_concurrency",
        &["vendor/QIHSE/src/tractable/qihse_table_store.c"],
    );
}

#[test]
fn hash_index_persistence_and_result_bounds() {
    run_fixture(
        "hash_index",
        &[
            "vendor/KEYSTONE/src/dsmil_hash_indexer.c",
            "vendor/KEYSTONE/src/keystone.c",
            "vendor/KEYSTONE/src/keystone_avx512.c",
            "vendor/KEYSTONE/src/keystone_avx512_search.c",
        ],
    );
}
