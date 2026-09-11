use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::native::{self, KeystoneIndex, Wal};
use crate::store::{self, DocRecord, SegmentWriter};

// ── File list cache ─────────────────────────────────────────────────

/// Metadata for a single file, stored in the file list cache.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct CachedFileMeta {
    pub path: String, // absolute path
    pub size: u64,
    pub mtime_ns: i64,
    pub inode: u64,
    pub device: u64,
}

/// The file list cache — all files seen during the last build walk.
/// Used during search to skip the filesystem walk when the corpus
/// hasn't changed.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct FileListCache {
    pub roots: Vec<String>,
    pub files: Vec<CachedFileMeta>,
    pub built_at: u64, // unix timestamp
    pub hidden: bool,  // whether hidden files were included
}

/// Save the file list cache to the state directory.
pub fn save_file_list_cache(state_dir: &Path, cache: &FileListCache) -> std::io::Result<()> {
    let json = serde_json::to_string(cache)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let path = state_dir.join("file_list_cache.json");
    native::atomic_write(&path, json.as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    Ok(())
}

/// Load the file list cache from the state directory.
pub fn load_file_list_cache(state_dir: &Path) -> Option<FileListCache> {
    let path = state_dir.join("file_list_cache.json");
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB
const FLUSH_SOURCE_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB source
const _FLUSH_POSTING_COUNT: usize = 2_000_000; // ~128 MiB postings
const FLUSH_TIME_SECS: u64 = 30;
const READ_CHUNK_SIZE: usize = 256 * 1024; // 256 KiB chunks

// ── State directory ─────────────────────────────────────────────────

pub fn default_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TGREP_STATE_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local/share/tgrep");
    }
    PathBuf::from(".tgrep")
}

fn ensure_state_dirs(state_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(state_dir)?;
    fs::create_dir_all(state_dir.join("segments"))?;
    fs::create_dir_all(state_dir.join("tmp"))?;
    fs::create_dir_all(state_dir.join("wal"))?;
    Ok(())
}

// ── WAL recovery ────────────────────────────────────────────────────

/// Recover committed segments from the WAL.
///
/// On startup, this replays the WAL to find segments that were fully
/// committed (BEGIN + INSERT + COMMIT). It returns the list of committed
/// segment names. Segments from uncommitted transactions are considered
/// partial and should be cleaned up.
///
/// If the WAL directory doesn't exist or is empty, returns an empty list.
pub fn recover_committed_segments(state_dir: &Path) -> Vec<String> {
    let wal_dir = state_dir.join("wal");
    if !wal_dir.exists() {
        return Vec::new();
    }

    let wal = match Wal::create(&wal_dir) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("tgrep: warning: WAL open failed: {}", e);
            return Vec::new();
        }
    };

    let records = match wal.replay() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("tgrep: warning: WAL replay failed: {}", e);
            return Vec::new();
        }
    };

    // Extract committed segment names (INSERT records with segment names as keys)
    let mut segments: Vec<String> = Vec::new();
    for record in &records {
        // op_type 1 = INSERT
        if record.op_type == 1 {
            if let Ok(name) = String::from_utf8(record.key.clone()) {
                segments.push(name);
            }
        }
    }

    eprintln!(
        "tgrep: WAL recovery found {} committed segment(s)",
        segments.len()
    );

    // Clean up orphaned segment files (in segments/ but not committed in WAL
    // and not in the current manifest)
    cleanup_orphaned_segments(state_dir, &segments);

    segments
}

/// Remove segment files that are neither in the committed WAL list nor
/// in the current manifest. These are leftovers from crashed builds.
fn cleanup_orphaned_segments(state_dir: &Path, wal_committed: &[String]) {
    let segments_dir = state_dir.join("segments");
    let manifest_segments: Vec<String> = store::load_manifest(state_dir)
        .map(|m| m.segments)
        .unwrap_or_default();

    let wal_set: std::collections::HashSet<&String> = wal_committed.iter().collect();
    let manifest_set: std::collections::HashSet<&String> = manifest_segments.iter().collect();

    if let Ok(entries) = fs::read_dir(&segments_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if !name.ends_with(".tgs") {
                    continue;
                }
                let name = name.to_string();
                if !wal_set.contains(&name) && !manifest_set.contains(&name) {
                    eprintln!("tgrep: cleaning orphaned segment: {}", name);
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// Checkpoint the WAL after a successful manifest publication.
/// This truncates old WAL entries, keeping the log bounded.
fn wal_checkpoint(state_dir: &Path) {
    let wal_dir = state_dir.join("wal");
    if !wal_dir.exists() {
        return;
    }
    if let Ok(wal) = Wal::create(&wal_dir) {
        let lsn = wal.current_lsn();
        if let Err(e) = wal.checkpoint(lsn) {
            eprintln!("tgrep: warning: WAL checkpoint failed: {}", e);
        }
    }
}

// ── Writer lock ──────────────────────────────────────────────────────

struct WriterLock {
    _file: fs::File,
    _path: PathBuf,
}

impl WriterLock {
    fn acquire(state_dir: &Path) -> std::io::Result<Self> {
        let lock_path = state_dir.join("writer.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)?;
        // Try exclusive lock (non-blocking)
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let rc = unsafe { libc_flock(fd, 2 | 4) }; // LOCK_EX | LOCK_NB
        if rc != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ResourceBusy,
                "another build/compaction is running (writer.lock held)",
            ));
        }
        Ok(WriterLock {
            _file: file,
            _path: lock_path,
        })
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        let _ = unsafe { libc_flock(self._file.as_raw_fd(), 8) }; // LOCK_UN
    }
}

// Minimal libc flock binding (avoid adding a libc crate dependency)
extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}
unsafe fn libc_flock(fd: i32, op: i32) -> i32 {
    flock(fd, op)
}

// ── Build state ──────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct BuildState {
    roots: Vec<String>,
    threads: usize,
    max_memory: String,
    io_limit: String,
    status: String, // "running", "paused", "completed"
    files_indexed: u64,
    bytes_indexed: u64,
    segments_written: u64,
    started_at: u64,
    updated_at: u64,
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_build_state(state_dir: &Path, bs: &BuildState) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(bs)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let path = state_dir.join("build.json");
    native::atomic_write(&path, json.as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    Ok(())
}

// ── Build pipeline ───────────────────────────────────────────────────

pub fn start_build(roots: &[String], threads: usize, max_memory: &str, io_limit: &str) {
    let state_dir = default_state_dir();
    if let Err(e) = run_build(roots, threads, max_memory, io_limit, &state_dir) {
        eprintln!("tgrep: build failed: {}", e);
        std::process::exit(1);
    }
}

fn run_build(
    roots: &[String],
    threads: usize,
    max_memory: &str,
    io_limit: &str,
    state_dir: &Path,
) -> std::io::Result<()> {
    ensure_state_dirs(state_dir)?;
    let _lock = WriterLock::acquire(state_dir)?;

    // Clear old segments and WAL from any previous build.
    // The build always re-indexes the entire corpus, so old segments
    // from interrupted builds would cause duplicate/incorrect results.
    let segments_dir = state_dir.join("segments");
    let tmp_dir = state_dir.join("tmp");
    let wal_dir = state_dir.join("wal");
    if let Ok(entries) = fs::read_dir(&segments_dir) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
    if let Ok(entries) = fs::read_dir(&tmp_dir) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
    if let Ok(entries) = fs::read_dir(&wal_dir) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }

    // Open WAL for crash-safe segment publication
    let wal =
        Wal::create(&wal_dir).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let mut txn_id: u64 = 1;

    let started_at = now_unix_secs();
    let mut build_state = BuildState {
        roots: roots.to_vec(),
        threads,
        max_memory: max_memory.to_string(),
        io_limit: io_limit.to_string(),
        status: "running".into(),
        files_indexed: 0,
        bytes_indexed: 0,
        segments_written: 0,
        started_at,
        updated_at: started_at,
    };
    write_build_state(state_dir, &build_state)?;

    // Fresh build — start from generation 1 with no segments
    let mut generation: u64 = 1;
    let mut all_segment_names: Vec<String> = Vec::new();

    let mut keystone =
        KeystoneIndex::new(4096).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    let mut hash_index = native::HashIndex::create(4096)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    let mut qwi_index = native::QihseWordIndex::create()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    let mut doc_records: Vec<DocRecord> = Vec::new();
    let mut batch_source_bytes: u64 = 0;
    let mut batch_start = Instant::now();
    let mut doc_id: u32 = 0;
    let mut root_id: u32 = 0;
    let mut cached_files: Vec<CachedFileMeta> = Vec::new();

    for root_str in roots {
        let root_path = PathBuf::from(root_str);
        if !root_path.exists() {
            eprintln!("tgrep: warning: root does not exist: {}", root_str);
            continue;
        }

        let walker = ignore::WalkBuilder::new(&root_path)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .threads(threads)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("tgrep: walk error: {}", e);
                    continue;
                }
            };

            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("tgrep: stat error {}: {}", path.display(), e);
                    continue;
                }
            };

            let file_size = meta.len();
            if file_size > MAX_FILE_SIZE {
                // Skip large files — they'll be scanned at search time
                continue;
            }

            // Read file content
            let content = match fs::read(path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("tgrep: read error {}: {}", path.display(), e);
                    continue;
                }
            };

            // Get relative path from root
            let rel_path = path
                .strip_prefix(&root_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            // Feed to KEYSTONE streaming ingestion
            let stream_result = {
                let mut stream = match keystone.begin_document(Some(&rel_path)) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("tgrep: keystone error {}: {}", path.display(), e);
                        continue;
                    }
                };

                // Feed in chunks
                let mut fed_ok = true;
                for chunk in content.chunks(READ_CHUNK_SIZE) {
                    if let Err(e) = stream.feed(chunk) {
                        eprintln!("tgrep: feed error {}: {}", path.display(), e);
                        fed_ok = false;
                        break;
                    }
                }

                if fed_ok {
                    stream.finish(Some(&mut doc_id))
                } else {
                    // stream.cancel() via Drop
                    Err("feed failed".into())
                }
            };

            if let Err(e) = stream_result {
                eprintln!("tgrep: index error {}: {}", path.display(), e);
                continue;
            }

            // Tokenize file content and add unique tokens to hash index
            // for O(1) whole-word exact-match queries.
            add_tokens_to_hash_index(&mut hash_index, &content, doc_id);
            add_tokens_to_qwi_index(&mut qwi_index, &content, doc_id);

            // Collect doc record
            let mtime_ns = meta.mtime() * 1_000_000_000 + meta.mtime_nsec();
            let ctime_ns = meta.ctime() * 1_000_000_000 + meta.ctime_nsec();
            doc_records.push(DocRecord {
                local_id: doc_id,
                root_id,
                path: rel_path.into_bytes(),
                byte_length: file_size,
                device: meta.dev(),
                inode: meta.ino(),
                mtime_ns,
                ctime_ns,
            });

            // Collect file metadata for the file list cache
            cached_files.push(CachedFileMeta {
                path: path.to_string_lossy().to_string(),
                size: file_size,
                mtime_ns,
                inode: meta.ino(),
                device: meta.dev(),
            });

            doc_id += 1;
            build_state.files_indexed += 1;
            build_state.bytes_indexed += file_size;
            batch_source_bytes += file_size;

            // Check flush thresholds
            let elapsed = batch_start.elapsed().as_secs();
            let ks_mem = keystone.memory_usage();
            let should_flush = batch_source_bytes >= FLUSH_SOURCE_BYTES
                || ks_mem as u64 >= FLUSH_SOURCE_BYTES * 2
                || elapsed >= FLUSH_TIME_SECS
                || doc_id > 0 && doc_id % 10000 == 0;

            if should_flush && !doc_records.is_empty() {
                // WAL: log segment publication as a transaction
                wal.begin(txn_id);

                let flush_result = flush_batch(
                    &mut keystone,
                    &mut hash_index,
                    &mut qwi_index,
                    &doc_records,
                    generation,
                    state_dir,
                );

                match flush_result {
                    Ok(seg_name) => {
                        // Log the segment in the WAL
                        let metadata = format!(
                            "{{\"generation\":{},\"files\":{},\"bytes\":{}}}",
                            generation,
                            doc_records.len(),
                            batch_source_bytes
                        );
                        wal.log_segment(txn_id, &seg_name, metadata.as_bytes());

                        all_segment_names.push(seg_name);
                        generation += 1;
                        build_state.segments_written += 1;
                        build_state.updated_at = now_unix_secs();
                        write_build_state(state_dir, &build_state)?;

                        // Publish manifest with new segment
                        store::publish_manifest(state_dir, &all_segment_names, generation - 1)?;

                        // Commit the WAL transaction
                        if let Err(e) = wal.commit(txn_id) {
                            eprintln!("tgrep: warning: WAL commit failed: {}", e);
                        }

                        // Reset for next batch
                        doc_records.clear();
                        batch_source_bytes = 0;
                        batch_start = Instant::now();
                        doc_id = 0;

                        eprintln!(
                            "tgrep: flushed segment {} ({} files, {} MB source)",
                            build_state.segments_written,
                            build_state.files_indexed,
                            build_state.bytes_indexed / 1_000_000,
                        );
                    }
                    Err(e) => {
                        // Abort the WAL transaction
                        let _ = wal.abort(txn_id);
                        return Err(e);
                    }
                }
                txn_id += 1;
            }
        }

        root_id += 1;
    }

    // Flush remaining docs
    if !doc_records.is_empty() {
        wal.begin(txn_id);
        let flush_result = flush_batch(
            &mut keystone,
            &mut hash_index,
            &mut qwi_index,
            &doc_records,
            generation,
            state_dir,
        );
        match flush_result {
            Ok(seg_name) => {
                let metadata = format!(
                    "{{\"generation\":{},\"files\":{},\"bytes\":{}}}",
                    generation,
                    doc_records.len(),
                    batch_source_bytes
                );
                wal.log_segment(txn_id, &seg_name, metadata.as_bytes());
                all_segment_names.push(seg_name);
                build_state.segments_written += 1;
            }
            Err(e) => {
                let _ = wal.abort(txn_id);
                return Err(e);
            }
        }
    }

    // Final manifest publication
    store::publish_manifest(state_dir, &all_segment_names, generation)?;

    // Commit final WAL transaction and checkpoint
    if !doc_records.is_empty() {
        if let Err(e) = wal.commit(txn_id) {
            eprintln!("tgrep: warning: final WAL commit failed: {}", e);
        }
    }
    wal_checkpoint(state_dir);

    build_state.status = "completed".into();
    build_state.updated_at = now_unix_secs();
    write_build_state(state_dir, &build_state)?;

    // Save file list cache for search-time walk optimization
    let cache = FileListCache {
        roots: roots.to_vec(),
        files: cached_files,
        built_at: started_at,
        hidden: true, // build always includes hidden files
    };
    if let Err(e) = save_file_list_cache(state_dir, &cache) {
        eprintln!("tgrep: warning: failed to save file list cache: {}", e);
    }

    eprintln!(
        "tgrep: build complete — {} files, {} MB, {} segments",
        build_state.files_indexed,
        build_state.bytes_indexed / 1_000_000,
        build_state.segments_written,
    );

    Ok(())
}

/// Finalize KEYSTONE, visit postings, build and flush a segment.
fn flush_batch(
    keystone: &mut KeystoneIndex,
    hash_index: &mut native::HashIndex,
    qwi_index: &mut native::QihseWordIndex,
    doc_records: &[DocRecord],
    generation: u64,
    state_dir: &Path,
) -> std::io::Result<String> {
    keystone
        .finalize()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    // Collect postings from KEYSTONE visitor
    let postings = collect_postings(keystone);

    // Build segment
    let seg_path = state_dir
        .join("tmp")
        .join(format!("seg_{:08}.tgs", generation));
    let mut writer = SegmentWriter::new(seg_path, generation);
    for doc in doc_records {
        writer.add_doc_record(doc.clone());
    }
    writer.set_postings(postings);

    let segments_dir = state_dir.join("segments");
    writer.flush(&segments_dir)?;

    let seg_name = format!("seg_{:08}.tgs", generation);

    // Finalize and save hash index alongside the segment
    if let Err(e) = hash_index.finalize() {
        eprintln!("tgrep: warning: hash index finalize failed: {}", e);
    } else {
        let hash_path = segments_dir.join(format!("seg_{:08}.thi", generation));
        if let Err(e) = hash_index.save(&hash_path) {
            eprintln!("tgrep: warning: hash index save failed: {}", e);
        }
    }

    // Save QIHSE btree word index alongside the segment
    let qwi_path = segments_dir.join(format!("seg_{:08}.qwi", generation));
    if let Err(e) = qwi_index.save(&qwi_path) {
        eprintln!("tgrep: warning: qihse word index save failed: {}", e);
    }

    // Reset KEYSTONE for next batch
    // (We create a new index since KEYSTONE doesn't have a reset API)
    *keystone =
        KeystoneIndex::new(4096).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    // Reset hash index for next batch
    *hash_index = native::HashIndex::create(4096)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    // Reset QIHSE word index for next batch
    *qwi_index = native::QihseWordIndex::create()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    Ok(seg_name)
}

/// Collect all postings from KEYSTONE via the visitor trampoline.
fn collect_postings(idx: &KeystoneIndex) -> BTreeMap<u32, Vec<u32>> {
    idx.collect_postings().unwrap_or_else(|e| {
        eprintln!("tgrep: warning: posting collection failed: {}", e);
        BTreeMap::new()
    })
}

/// Tokenize file content and add unique tokens to the hash index.
/// Tokens are sequences of ASCII word characters (alphanumeric + underscore).
/// Each unique token in the file is added with the file's doc_id.
fn add_tokens_to_hash_index(hash: &mut native::HashIndex, content: &[u8], doc_id: u32) {
    let mut seen = std::collections::HashSet::new();
    let mut start = None;
    for (i, &b) in content.iter().enumerate() {
        let is_word = b.is_ascii_alphanumeric() || b == b'_';
        if is_word {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start {
            let token = &content[s..i];
            if token.len() >= 2 && seen.insert(token.to_vec()) {
                let _ = hash.add(token, doc_id);
            }
            start = None;
        }
    }
    if let Some(s) = start {
        let token = &content[s..];
        if token.len() >= 2 && seen.insert(token.to_vec()) {
            let _ = hash.add(token, doc_id);
        }
    }
}

/// Tokenize file content and add unique tokens to the QIHSE word index.
/// Same tokenization as add_tokens_to_hash_index, but inserts into the
/// persistent B+ tree backed by QIHSE.
fn add_tokens_to_qwi_index(qwi: &mut native::QihseWordIndex, content: &[u8], doc_id: u32) {
    let mut seen = std::collections::HashSet::new();
    let mut start = None;
    for (i, &b) in content.iter().enumerate() {
        let is_word = b.is_ascii_alphanumeric() || b == b'_';
        if is_word {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start {
            let token = &content[s..i];
            if token.len() >= 2 && seen.insert(token.to_vec()) {
                let _ = qwi.add(token, doc_id);
            }
            start = None;
        }
    }
    if let Some(s) = start {
        let token = &content[s..];
        if token.len() >= 2 && seen.insert(token.to_vec()) {
            let _ = qwi.add(token, doc_id);
        }
    }
}

// ── Other commands (stubs for now) ──────────────────────────────────

pub fn pause() {
    eprintln!("tgrep: pause not yet implemented");
}

pub fn resume() {
    eprintln!("tgrep: resume not yet implemented");
}

pub fn refresh() {
    eprintln!("tgrep: refresh not yet implemented");
}

pub fn compact() {
    eprintln!("tgrep: compact not yet implemented");
}

pub fn rebuild() {
    eprintln!("tgrep: rebuild not yet implemented");
}
