fn main() {
    // Detect SIMD flags from environment (set by compile.sh) or fall back to SSE4.2
    let simd_flags = std::env::var("TGREP_SIMD_FLAGS")
        .unwrap_or_else(|_| "-msse4.2".to_string());

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
