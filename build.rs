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
        .file("/home/john/Documents/KEYSTONE/src/keystone_trigram.c")
        // KEYSTONE core + hash index sources
        .file("/home/john/Documents/KEYSTONE/src/keystone.c")
        .file("/home/john/Documents/KEYSTONE/src/keystone_avx512.c")
        .file("/home/john/Documents/KEYSTONE/src/keystone_avx512_search.c")
        .file("/home/john/Documents/KEYSTONE/src/dsmil_hash_indexer.c")
        // QIHSE WAL source
        .file("/home/john/Documents/QIHSE/src/tractable/qihse_wal.c")
        .file("/home/john/Documents/QIHSE/src/tractable/qihse_table_store.c")
        // QIHSE btree source (persistent word index)
        .file("/home/john/Documents/QIHSE/src/frieze/qihse_btree.c")
        .include("/home/john/Documents/KEYSTONE/include")
        .include("/home/john/Documents/QIHSE/include")
        .include("/home/john/Documents/QIHSE/persistence")
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

    println!("cargo:rerun-if-env-changed=TGREP_SIMD_FLAGS");
    println!("cargo:rerun-if-changed=native/keystone_wrapper.c");
    println!("cargo:rerun-if-changed=native/qihse_wrapper.c");
    println!("cargo:rerun-if-changed=native/qihse_wal_wrapper.c");
    println!("cargo:rerun-if-changed=native/qihse_cache_wrapper.c");
    println!("cargo:rerun-if-changed=native/hash_wrapper.c");
    println!("cargo:rerun-if-changed=native/word_index_mmap.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/QIHSE/src/tractable/qihse_table_store.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/QIHSE/src/tractable/qihse_wal.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/QIHSE/src/frieze/qihse_btree.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/src/keystone_trigram.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/src/keystone.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/src/keystone_avx512.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/src/keystone_avx512_search.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/src/dsmil_hash_indexer.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/include/keystone_trigram.h");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/include/dsmil_hash_indexer.h");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/include/keystone.h");
    println!("cargo:rerun-if-changed=/home/john/Documents/QIHSE/include/qihse_btree.h");
    println!("cargo:rerun-if-changed=/home/john/Documents/QIHSE/include/qihse_hash_index.h");
    println!("cargo:rerun-if-changed=/home/john/Documents/QIHSE/include/qihse_platform.h");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
