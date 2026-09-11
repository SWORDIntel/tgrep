fn main() {
    cc::Build::new()
        // tgrep wrappers
        .file("native/keystone_wrapper.c")
        .file("native/qihse_wrapper.c")
        .file("native/qihse_wal_wrapper.c")
        .file("native/hash_wrapper.c")
        // KEYSTONE trigram source (all tgrep extensions are in this file)
        .file("/home/john/Documents/KEYSTONE/src/keystone_trigram.c")
        // KEYSTONE core + hash index sources
        .file("/home/john/Documents/KEYSTONE/src/keystone.c")
        .file("/home/john/Documents/KEYSTONE/src/keystone_avx512.c")
        .file("/home/john/Documents/KEYSTONE/src/keystone_avx512_search.c")
        .file("/home/john/Documents/KEYSTONE/src/dsmil_hash_indexer.c")
        // QIHSE WAL source
        .file("/home/john/Documents/QIHSE/src/tractable/qihse_wal.c")
        .include("/home/john/Documents/KEYSTONE/include")
        .include("/home/john/Documents/QIHSE/include")
        .include("/home/john/Documents/QIHSE/persistence")
        .flag("-std=c11")
        .flag("-D_GNU_SOURCE")
        .flag("-Wall")
        .flag("-Wextra")
        .flag("-O2")
        .flag("-msse4.2")
        .compile("tgrep_native");

    println!("cargo:rerun-if-changed=native/keystone_wrapper.c");
    println!("cargo:rerun-if-changed=native/qihse_wrapper.c");
    println!("cargo:rerun-if-changed=native/qihse_wal_wrapper.c");
    println!("cargo:rerun-if-changed=native/hash_wrapper.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/src/keystone_trigram.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/src/keystone.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/src/keystone_avx512.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/src/keystone_avx512_search.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/src/dsmil_hash_indexer.c");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/include/keystone_trigram.h");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/include/dsmil_hash_indexer.h");
    println!("cargo:rerun-if-changed=/home/john/Documents/KEYSTONE/include/keystone.h");
    println!("cargo:rerun-if-changed=/home/john/Documents/QIHSE/src/tractable/qihse_wal.c");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
