use std::process::Command;

fn git_output(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn build_version() {
    let mut revision = git_output(&["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "source-archive".to_owned());
    if git_output(&["status", "--porcelain"]).is_some_and(|status| !status.is_empty()) {
        revision.push_str("-dirty");
    }
    // Watch Git metadata and tracked inputs so a rebuild records the actual
    // checkout, including worktrees whose .git entry is a file.
    for name in ["HEAD", "index"] {
        if let Some(path) = git_output(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(reference) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git_output(&["rev-parse", "--git-path", &reference]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Ok(output) = Command::new("git").args(["ls-files", "-z"]).output() {
        if output.status.success() {
            for path in output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
            {
                if let Ok(path) = std::str::from_utf8(path) {
                    // Cargo's line protocol cannot represent newline-containing paths.
                    if !path.contains(['\r', '\n']) {
                        println!("cargo:rerun-if-changed={path}");
                    }
                }
            }
        }
    }
    let version = std::env::var("CARGO_PKG_VERSION").expect("Cargo supplies package version");
    let profile = std::env::var("PROFILE").expect("Cargo supplies build profile");
    let target = std::env::var("TARGET").expect("Cargo supplies target triple");
    println!("cargo:rustc-env=TGREP_BUILD_VERSION={version} ({revision}; {profile}; {target})");
}

fn main() {
    build_version();
    println!("cargo:rerun-if-env-changed=TGREP_SIMD_FLAGS");
    for directory in [
        "native",
        "vendor/KEYSTONE/include",
        "vendor/QIHSE/include",
        "vendor/QIHSE/persistence",
        "vendor/KEYSTONE/src",
        "vendor/QIHSE/src",
    ] {
        println!("cargo:rerun-if-changed={directory}");
    }
    // Detect SIMD flags from environment (set by compile.sh) or fall back to SSE4.2
    let simd_flags = std::env::var("TGREP_SIMD_FLAGS").unwrap_or_else(|_| "-msse4.2".to_string());

    let mut build = cc::Build::new();
    build
        // tgrep wrappers
        .file("native/keystone_wrapper.c")
        .file("native/qihse_wrapper.c")
        .file("native/qihse_wal_wrapper.c")
        .file("native/qihse_opt_wrapper.c")
        .file("native/hash_wrapper.c")
        .file("native/qihse_cache_wrapper.c")
        .file("native/word_index_mmap.c")
        // KEYSTONE trigram source (all tgrep extensions are in this file)
        .file("vendor/KEYSTONE/src/keystone_trigram.c")
        // KEYSTONE core + hash index sources
        .file("vendor/KEYSTONE/src/keystone.c")
        .file("vendor/KEYSTONE/src/keystone_avx512.c")
        .file("vendor/KEYSTONE/src/keystone_avx512_search.c")
        .file("vendor/KEYSTONE/src/dsmil_hash_indexer.c")
        // QIHSE WAL source
        .file("vendor/QIHSE/src/tractable/qihse_wal.c")
        .file("vendor/QIHSE/src/tractable/qihse_table_store.c")
        // QIHSE btree source (persistent word index)
        .file("vendor/QIHSE/src/frieze/qihse_btree.c")
        .include("vendor/KEYSTONE/include")
        .include("vendor/QIHSE/include")
        .include("vendor/QIHSE/persistence")
        .flag("-std=c11")
        .flag("-D_GNU_SOURCE")
        .flag("-Wall")
        .flag("-Wextra")
        .flag("-O2");

    // Apply SIMD flags (may be multiple: "-msse4.2 -mavx2 -mavx512f")
    for flag in simd_flags.split_whitespace() {
        build.flag(flag);
    }

    // Pass SIMD level to Rust code via cfg
    if simd_flags.contains("avx512f") {
        println!("cargo:rustc-cfg=avx512");
    } else if simd_flags.contains("avx2") {
        println!("cargo:rustc-cfg=avx2");
    }

    build.compile("tgrep_native");

    for f in [
        "native/keystone_wrapper.c",
        "native/qihse_wrapper.c",
        "native/qihse_wal_wrapper.c",
        "native/qihse_opt_wrapper.c",
        "native/qihse_cache_wrapper.c",
        "native/hash_wrapper.c",
        "native/word_index_mmap.c",
        "vendor/QIHSE/src/tractable/qihse_table_store.c",
        "vendor/QIHSE/src/tractable/qihse_wal.c",
        "vendor/QIHSE/src/frieze/qihse_btree.c",
        "vendor/KEYSTONE/src/keystone_trigram.c",
        "vendor/KEYSTONE/src/keystone.c",
        "vendor/KEYSTONE/src/keystone_avx512.c",
        "vendor/KEYSTONE/src/keystone_avx512_search.c",
        "vendor/KEYSTONE/src/dsmil_hash_indexer.c",
        "vendor/KEYSTONE/include/keystone_trigram.h",
        "vendor/KEYSTONE/include/dsmil_hash_indexer.h",
        "vendor/KEYSTONE/include/keystone.h",
        "vendor/QIHSE/include/qihse_btree.h",
        "vendor/QIHSE/include/qihse_hash_index.h",
        "vendor/QIHSE/include/qihse_platform.h",
    ] {
        println!("cargo:rerun-if-changed={f}");
    }
    println!("cargo:rerun-if-changed=Cargo.toml");
}
