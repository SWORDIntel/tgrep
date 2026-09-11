// FFI bindings to KEYSTONE and QIHSE C helpers.
//
// Raw extern "C" declarations are in the `ffi` module. Safe RAII wrappers
// are in the `keystone` and `qihse` modules below.

use std::ffi::c_void;
use std::path::Path;

// ── Raw FFI ──────────────────────────────────────────────────────────

pub mod ffi {
    use std::ffi::c_void;
    use crate::native::KeystoneStatsRaw;

    #[link(name = "tgrep_native")]
    extern "C" {
        // Index lifecycle
        pub fn tgrep_keystone_index_create(initial_doc_capacity: usize) -> *mut c_void;
        pub fn tgrep_keystone_index_destroy(handle: *mut c_void);
        pub fn tgrep_keystone_index_finalize(handle: *mut c_void) -> i32;
        pub fn tgrep_keystone_doc_count(handle: *mut c_void) -> usize;
        pub fn tgrep_keystone_memory_usage(handle: *mut c_void) -> usize;

        // Streaming ingestion
        pub fn tgrep_keystone_begin_document(
            handle: *mut c_void,
            name: *const std::ffi::c_char,
        ) -> *mut c_void;
        pub fn tgrep_keystone_feed_bytes(
            stream: *mut c_void,
            data: *const u8,
            len: usize,
        ) -> i32;
        pub fn tgrep_keystone_end_document(
            stream: *mut c_void,
            out_doc_id: *mut u32,
        ) -> i32;
        pub fn tgrep_keystone_cancel_document(stream: *mut c_void);

        // Trigram extraction and frequency
        pub fn tgrep_keystone_extract_trigrams(
            pattern: *const std::ffi::c_char,
            pattern_len: usize,
            out_trigrams: *mut u32,
            max_trigrams: usize,
        ) -> usize;
        pub fn tgrep_keystone_frequency(handle: *mut c_void, gram: u32) -> usize;

        // Candidate query (one-shot)
        pub fn tgrep_keystone_get_candidates(
            handle: *mut c_void,
            pattern: *const std::ffi::c_char,
            pattern_len: usize,
            out_candidates: *mut u32,
            max_candidates: usize,
        ) -> usize;

        // Candidate query (paginated)
        pub fn tgrep_keystone_candidates_begin(
            handle: *mut c_void,
            pattern: *const std::ffi::c_char,
            pattern_len: usize,
        ) -> *mut c_void;
        pub fn tgrep_keystone_candidates_next(
            iter: *mut c_void,
            out_buf: *mut u32,
            capacity: usize,
            out_count: *mut usize,
            out_exhausted: *mut i32,
        ) -> i32;
        pub fn tgrep_keystone_candidates_free(iter: *mut c_void);

        // Posting export visitor
        pub fn tgrep_keystone_visit_postings(
            handle: *mut c_void,
            visitor: *const c_void,
            ctx: *mut c_void,
        ) -> i32;

        // Posting collection (trampoline)
        pub fn tgrep_keystone_collect_postings(
            handle: *mut c_void,
            out_grams: *mut u32,
            out_offsets: *mut u32,
            out_doc_ids: *mut u32,
            max_grams: usize,
            max_doc_ids: usize,
            out_num_grams: *mut usize,
            out_num_doc_ids: *mut usize,
            out_truncated: *mut i32,
        ) -> i32;

        // Stats
        pub fn tgrep_keystone_get_stats(
            handle: *mut c_void,
            out_stats: *mut KeystoneStatsRaw,
        ) -> i32;

        // QIHSE file helpers
        pub fn tgrep_qihse_file_sync(fd: i32) -> i32;
        pub fn tgrep_qihse_dir_sync(path: *const std::ffi::c_char) -> i32;
        pub fn tgrep_qihse_atomic_write(
            target_path: *const std::ffi::c_char,
            data: *const u8,
            len: usize,
        ) -> i32;

        // QIHSE WAL
        pub fn tgrep_wal_create(directory: *const std::ffi::c_char) -> *mut c_void;
        pub fn tgrep_wal_destroy(wal: *mut c_void);
        pub fn tgrep_wal_begin(wal: *mut c_void, txn_id: u64) -> u64;
        pub fn tgrep_wal_log_segment(
            wal: *mut c_void,
            txn_id: u64,
            segment_name: *const std::ffi::c_char,
            metadata: *const u8,
            metadata_len: u32,
        ) -> u64;
        pub fn tgrep_wal_commit(wal: *mut c_void, txn_id: u64) -> i32;
        pub fn tgrep_wal_abort(wal: *mut c_void, txn_id: u64) -> i32;
        pub fn tgrep_wal_flush(wal: *mut c_void) -> i32;
        pub fn tgrep_wal_current_lsn(wal: *mut c_void) -> u64;
        pub fn tgrep_wal_checkpoint(wal: *mut c_void, lsn: u64) -> i32;

        // Opaque callback type for replay — we pass a function pointer
        pub fn tgrep_wal_replay(
            wal: *mut c_void,
            callback: *const c_void,
            user_data: *mut c_void,
        ) -> i32;

        // Hash index (dsmil_hash_index)
        pub fn tgrep_hash_index_create(initial_capacity: usize) -> *mut c_void;
        pub fn tgrep_hash_index_destroy(handle: *mut c_void);
        pub fn tgrep_hash_index_add(
            handle: *mut c_void,
            str_: *const std::ffi::c_char,
            len: usize,
            doc_id: u64,
        ) -> i32;
        pub fn tgrep_hash_index_finalize(handle: *mut c_void) -> i32;
        pub fn tgrep_hash_index_count(handle: *mut c_void) -> usize;
        pub fn tgrep_hash_index_search_all(
            handle: *mut c_void,
            query_str: *const std::ffi::c_char,
            query_len: usize,
            out_doc_ids: *mut u64,
            max_results: usize,
        ) -> usize;
        pub fn tgrep_hash_index_save(
            handle: *mut c_void,
            path: *const std::ffi::c_char,
        ) -> i32;
        pub fn tgrep_hash_index_load(path: *const std::ffi::c_char) -> *mut c_void;
    }
}

// ── Raw structs ─────────────────────────────────────────────────────

#[repr(C)]
pub struct KeystoneStatsRaw {
    pub total_documents: u64,
    pub bytes_indexed: u64,
    pub unique_trigrams: u64,
    pub total_postings: u64,
    pub build_time_ns: u64,
    pub total_searches: u64,
    pub candidate_docs_evaluated: u64,
    pub candidate_docs_rejected: u64,
}

// ── Error codes (mirror KEYSTONE_TRIGRAM_* constants) ──────────────

pub const KS_OK: i32 = 0;
pub const KS_EINVAL: i32 = -1;
pub const KS_ENOMEM: i32 = -2;
pub const KS_EOVERFLOW: i32 = -3;
pub const KS_ESTATE: i32 = -4;

pub fn ks_err_str(code: i32) -> &'static str {
    match code {
        KS_OK => "ok",
        KS_EINVAL => "invalid argument",
        KS_ENOMEM => "out of memory",
        KS_EOVERFLOW => "overflow",
        KS_ESTATE => "invalid state",
        _ => "unknown error",
    }
}

// ── Safe RAII wrappers ──────────────────────────────────────────────

/// RAII handle to a KEYSTONE trigram index.
pub struct KeystoneIndex {
    handle: *mut c_void,
}

impl KeystoneIndex {
    /// Create a new trigram index.
    pub fn new(initial_doc_capacity: usize) -> Result<Self, String> {
        let handle = unsafe { ffi::tgrep_keystone_index_create(initial_doc_capacity) };
        if handle.is_null() {
            Err("failed to create index".into())
        } else {
            Ok(Self { handle })
        }
    }

    /// Finalize the index (mark as ready for queries).
    pub fn finalize(&mut self) -> Result<(), String> {
        let rc = unsafe { ffi::tgrep_keystone_index_finalize(self.handle) };
        if rc != KS_OK {
            Err(ks_err_str(rc).into())
        } else {
            Ok(())
        }
    }

    /// Get the number of documents in the index.
    pub fn doc_count(&self) -> usize {
        unsafe { ffi::tgrep_keystone_doc_count(self.handle) }
    }

    /// Get approximate memory usage in bytes.
    pub fn memory_usage(&self) -> usize {
        unsafe { ffi::tgrep_keystone_memory_usage(self.handle) }
    }

    /// Begin a streaming document for chunked ingestion.
    pub fn begin_document(&mut self, name: Option<&str>) -> Result<DocumentStream, String> {
        let name_c = name.map(|n| std::ffi::CString::new(n).unwrap());
        let name_ptr = name_c.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null());
        let stream =
            unsafe { ffi::tgrep_keystone_begin_document(self.handle, name_ptr) };
        if stream.is_null() {
            Err("failed to begin document".into())
        } else {
            Ok(DocumentStream {
                stream,
                _name: name_c,
            })
        }
    }

    /// Extract unique trigrams from a pattern.
    pub fn extract_trigrams(pattern: &[u8]) -> Vec<u32> {
        let mut buf = vec![0u32; 64];
        let n = unsafe {
            ffi::tgrep_keystone_extract_trigrams(
                pattern.as_ptr() as *const std::ffi::c_char,
                pattern.len(),
                buf.as_mut_ptr(),
                buf.len(),
            )
        };
        buf.truncate(n);
        buf
    }

    /// Get document frequency of a trigram.
    pub fn frequency(&self, gram: u32) -> usize {
        unsafe { ffi::tgrep_keystone_frequency(self.handle, gram) }
    }

    /// Get candidates for a pattern (one-shot). Returns doc IDs.
    pub fn get_candidates(&self, pattern: &[u8]) -> Vec<u32> {
        let max = self.doc_count();
        if max == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u32; max];
        let n = unsafe {
            ffi::tgrep_keystone_get_candidates(
                self.handle,
                pattern.as_ptr() as *const std::ffi::c_char,
                pattern.len(),
                buf.as_mut_ptr(),
                buf.len(),
            )
        };
        buf.truncate(n);
        buf
    }

    /// Create a paginated candidate iterator.
    pub fn candidates(&self, pattern: &[u8]) -> Result<CandidateIter, String> {
        let iter = unsafe {
            ffi::tgrep_keystone_candidates_begin(
                self.handle,
                pattern.as_ptr() as *const std::ffi::c_char,
                pattern.len(),
            )
        };
        if iter.is_null() {
            Err("no candidates (pattern too short or trigrams not found)".into())
        } else {
            Ok(CandidateIter { iter })
        }
    }

    /// Get index stats.
    pub fn stats(&self) -> KeystoneStatsRaw {
        let mut s = KeystoneStatsRaw {
            total_documents: 0,
            bytes_indexed: 0,
            unique_trigrams: 0,
            total_postings: 0,
            build_time_ns: 0,
            total_searches: 0,
            candidate_docs_evaluated: 0,
            candidate_docs_rejected: 0,
        };
        unsafe { ffi::tgrep_keystone_get_stats(self.handle, &mut s) };
        s
    }

    /// Collect all postings from the index via the C visitor trampoline.
    /// Returns a BTreeMap of gram -> sorted doc IDs.
    /// Requires a finalized index.
    pub fn collect_postings(&self) -> Result<std::collections::BTreeMap<u32, Vec<u32>>, String> {
        let stats = self.stats();
        let max_grams = stats.unique_trigrams as usize + 1;
        let max_doc_ids = stats.total_postings as usize + 1;

        let mut grams = vec![0u32; max_grams];
        let mut offsets = vec![0u32; max_grams];
        let mut doc_ids = vec![0u32; max_doc_ids];
        let mut num_grams: usize = 0;
        let mut num_doc_ids: usize = 0;
        let mut truncated: i32 = 0;

        let rc = unsafe {
            ffi::tgrep_keystone_collect_postings(
                self.handle,
                grams.as_mut_ptr(),
                offsets.as_mut_ptr(),
                doc_ids.as_mut_ptr(),
                max_grams,
                max_doc_ids,
                &mut num_grams,
                &mut num_doc_ids,
                &mut truncated,
            )
        };
        if rc != KS_OK {
            return Err(ks_err_str(rc).into());
        }
        if truncated != 0 {
            return Err("posting collection truncated (capacity exceeded)".into());
        }

        let mut result = std::collections::BTreeMap::new();
        for i in 0..num_grams {
            let gram = grams[i];
            let start = offsets[i] as usize;
            let end = if i + 1 < num_grams {
                offsets[i + 1] as usize
            } else {
                num_doc_ids
            };
            result.insert(gram, doc_ids[start..end].to_vec());
        }
        Ok(result)
    }
}

impl Drop for KeystoneIndex {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::tgrep_keystone_index_destroy(self.handle) };
        }
    }
}

/// RAII handle to a streaming document being ingested.
pub struct DocumentStream {
    stream: *mut c_void,
    // Keep the CString alive so the name persists
    _name: Option<std::ffi::CString>,
}

impl DocumentStream {
    /// Feed bytes to the current document.
    pub fn feed(&mut self, data: &[u8]) -> Result<(), String> {
        let rc = unsafe {
            ffi::tgrep_keystone_feed_bytes(self.stream, data.as_ptr(), data.len())
        };
        if rc != KS_OK {
            Err(ks_err_str(rc).into())
        } else {
            Ok(())
        }
    }

    /// Finish the document and add to index. Returns assigned doc_id.
    pub fn finish(self, out_doc_id: Option<&mut u32>) -> Result<(), String> {
        let mut id: u32 = 0;
        let id_ptr = match out_doc_id {
            Some(p) => p as *mut u32,
            None => &mut id as *mut u32,
        };
        let rc = unsafe { ffi::tgrep_keystone_end_document(self.stream, id_ptr) };
        // Stream is consumed regardless of result
        std::mem::forget(self); // prevent Drop from running (stream already freed in C)
        if rc != KS_OK {
            Err(ks_err_str(rc).into())
        } else {
            Ok(())
        }
    }

    /// Abort the document without indexing.
    pub fn cancel(self) {
        unsafe { ffi::tgrep_keystone_cancel_document(self.stream) };
        std::mem::forget(self);
    }
}

impl Drop for DocumentStream {
    fn drop(&mut self) {
        // If not explicitly finished or cancelled, cancel to avoid leak
        if !self.stream.is_null() {
            unsafe { ffi::tgrep_keystone_cancel_document(self.stream) };
        }
    }
}

/// RAII handle to a paginated candidate iterator.
pub struct CandidateIter {
    iter: *mut c_void,
}

impl CandidateIter {
    /// Fetch the next batch of candidates. Returns (doc_ids, exhausted).
    pub fn next_batch(&mut self, capacity: usize) -> Result<(Vec<u32>, bool), String> {
        let mut buf = vec![0u32; capacity];
        let mut count: usize = 0;
        let mut exhausted: i32 = 0;
        let rc = unsafe {
            ffi::tgrep_keystone_candidates_next(
                self.iter,
                buf.as_mut_ptr(),
                capacity,
                &mut count,
                &mut exhausted,
            )
        };
        if rc != KS_OK {
            return Err(ks_err_str(rc).into());
        }
        buf.truncate(count);
        Ok((buf, exhausted != 0))
    }

    /// Collect all candidates into a Vec. Convenience method.
    pub fn collect_all(mut self) -> Result<Vec<u32>, String> {
        let mut all = Vec::new();
        loop {
            let (batch, exhausted) = self.next_batch(4096)?;
            all.extend_from_slice(&batch);
            if exhausted {
                break;
            }
        }
        Ok(all)
    }
}

impl Drop for CandidateIter {
    fn drop(&mut self) {
        if !self.iter.is_null() {
            unsafe { ffi::tgrep_keystone_candidates_free(self.iter) };
        }
    }
}

// ── QIHSE safe wrappers ─────────────────────────────────────────────

/// Atomically write data to a file (tmp + fsync + rename + dir fsync).
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
    let c_path = std::ffi::CString::new(path.to_str().ok_or("invalid path")?)
        .map_err(|e| e.to_string())?;
    let rc = unsafe {
        ffi::tgrep_qihse_atomic_write(
            c_path.as_ptr(),
            data.as_ptr(),
            data.len(),
        )
    };
    if rc != 0 {
        Err(format!("atomic_write failed (rc={})", rc))
    } else {
        Ok(())
    }
}

/// fsync a file descriptor.
pub fn file_sync(fd: i32) -> Result<(), String> {
    let rc = unsafe { ffi::tgrep_qihse_file_sync(fd) };
    if rc != 0 {
        Err(format!("fsync failed (rc={})", rc))
    } else {
        Ok(())
    }
}

/// Open a directory and fsync it.
pub fn dir_sync(path: &Path) -> Result<(), String> {
    let c_path = std::ffi::CString::new(path.to_str().ok_or("invalid path")?)
        .map_err(|e| e.to_string())?;
    let rc = unsafe { ffi::tgrep_qihse_dir_sync(c_path.as_ptr()) };
    if rc != 0 {
        Err(format!("dir_sync failed (rc={})", rc))
    } else {
        Ok(())
    }
}

// ── QIHSE WAL safe wrapper ──────────────────────────────────────────

/// A record recovered during WAL replay.
#[derive(Debug, Clone)]
pub struct WalReplayRecord {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub op_type: u8,
    pub txn_id: u64,
}

/// RAII handle to a QIHSE write-ahead log.
pub struct Wal {
    handle: *mut c_void,
}

extern "C" fn replay_trampoline(
    key: *const std::ffi::c_char,
    key_len: u32,
    value: *const u8,
    value_len: u32,
    op_type: u8,
    txn_id: u64,
    user_data: *mut c_void,
) -> bool {
    let records = unsafe { &mut *(user_data as *mut Vec<WalReplayRecord>) };
    let key_slice = unsafe { std::slice::from_raw_parts(key as *const u8, key_len as usize) };
    let value_slice = if value.is_null() || value_len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(value, value_len as usize) }.to_vec()
    };
    records.push(WalReplayRecord {
        key: key_slice.to_vec(),
        value: value_slice,
        op_type,
        txn_id,
    });
    true
}

impl Wal {
    /// Create or open a WAL in the given directory.
    pub fn create(directory: &Path) -> Result<Self, String> {
        let c_path = std::ffi::CString::new(directory.to_str().ok_or("invalid path")?)
            .map_err(|e| e.to_string())?;
        let handle = unsafe { ffi::tgrep_wal_create(c_path.as_ptr()) };
        if handle.is_null() {
            Err("qihse_wal_create returned null".to_string())
        } else {
            Ok(Wal { handle })
        }
    }

    /// Begin a transaction. Returns the assigned LSN (0 on failure).
    pub fn begin(&self, txn_id: u64) -> u64 {
        unsafe { ffi::tgrep_wal_begin(self.handle, txn_id) }
    }

    /// Log a segment publication within a transaction.
    pub fn log_segment(&self, txn_id: u64, segment_name: &str, metadata: &[u8]) -> u64 {
        let c_name = match std::ffi::CString::new(segment_name) {
            Ok(s) => s,
            Err(_) => return 0,
        };
        unsafe {
            ffi::tgrep_wal_log_segment(
                self.handle,
                txn_id,
                c_name.as_ptr(),
                metadata.as_ptr(),
                metadata.len() as u32,
            )
        }
    }

    /// Commit a transaction (appends COMMIT + flushes).
    pub fn commit(&self, txn_id: u64) -> Result<(), String> {
        let rc = unsafe { ffi::tgrep_wal_commit(self.handle, txn_id) };
        if rc != 0 {
            Err(format!("wal_commit failed (rc={})", rc))
        } else {
            Ok(())
        }
    }

    /// Abort a transaction (appends ABORT + flushes).
    pub fn abort(&self, txn_id: u64) -> Result<(), String> {
        let rc = unsafe { ffi::tgrep_wal_abort(self.handle, txn_id) };
        if rc != 0 {
            Err(format!("wal_abort failed (rc={})", rc))
        } else {
            Ok(())
        }
    }

    /// Flush pending WAL writes to disk.
    pub fn flush(&self) -> Result<(), String> {
        let rc = unsafe { ffi::tgrep_wal_flush(self.handle) };
        if rc != 0 {
            Err(format!("wal_flush failed (rc={})", rc))
        } else {
            Ok(())
        }
    }

    /// Get the current LSN (next to be assigned).
    pub fn current_lsn(&self) -> u64 {
        unsafe { ffi::tgrep_wal_current_lsn(self.handle) }
    }

    /// Record a checkpoint and truncate old WAL segments.
    pub fn checkpoint(&self, lsn: u64) -> Result<(), String> {
        let rc = unsafe { ffi::tgrep_wal_checkpoint(self.handle, lsn) };
        if rc != 0 {
            Err(format!("wal_checkpoint failed (rc={})", rc))
        } else {
            Ok(())
        }
    }

    /// Replay all committed WAL records. Returns records from committed
    /// transactions only (uncommitted/partial writes are skipped).
    pub fn replay(&self) -> Result<Vec<WalReplayRecord>, String> {
        let mut records: Vec<WalReplayRecord> = Vec::new();
        let cb_ptr = replay_trampoline as *const c_void;
        let user_data = &mut records as *mut Vec<WalReplayRecord> as *mut c_void;
        let n = unsafe { ffi::tgrep_wal_replay(self.handle, cb_ptr, user_data) };
        if n < 0 {
            Err(format!("wal_replay failed (rc={})", n))
        } else {
            Ok(records)
        }
    }
}

impl Drop for Wal {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::tgrep_wal_destroy(self.handle) };
        }
    }
}

// ── Hash index safe wrapper ─────────────────────────────────────────

/// Safe RAII wrapper around the KEYSTONE dsmil_hash_index.
/// Maps tokens to doc IDs for O(1) whole-token exact-match queries.
pub struct HashIndex {
    handle: *mut c_void,
}

impl HashIndex {
    /// Create a new hash index with the given initial capacity.
    pub fn create(initial_capacity: usize) -> Result<Self, String> {
        let handle = unsafe { ffi::tgrep_hash_index_create(initial_capacity) };
        if handle.is_null() {
            Err("hash_index_create returned null".into())
        } else {
            Ok(HashIndex { handle })
        }
    }

    /// Add a token with its doc ID to the index.
    pub fn add(&mut self, token: &[u8], doc_id: u32) -> Result<(), String> {
        let rc = unsafe {
            ffi::tgrep_hash_index_add(
                self.handle,
                token.as_ptr() as *const std::ffi::c_char,
                token.len(),
                doc_id as u64,
            )
        };
        if rc != 0 {
            Err(format!("hash_index_add failed (rc={})", rc))
        } else {
            Ok(())
        }
    }

    /// Finalize (sort) the index for searching.
    pub fn finalize(&mut self) -> Result<(), String> {
        let rc = unsafe { ffi::tgrep_hash_index_finalize(self.handle) };
        if rc != 0 {
            Err(format!("hash_index_finalize failed (rc={})", rc))
        } else {
            Ok(())
        }
    }

    /// Get the number of entries in the index.
    pub fn count(&self) -> usize {
        unsafe { ffi::tgrep_hash_index_count(self.handle) }
    }

    /// Search for all doc IDs matching a token.
    /// Returns a Vec of doc IDs. Empty if not found.
    pub fn search_all(&self, token: &[u8]) -> Vec<u32> {
        let max_results = 4096;
        let mut buf = vec![0u64; max_results];
        let count = unsafe {
            ffi::tgrep_hash_index_search_all(
                self.handle,
                token.as_ptr() as *const std::ffi::c_char,
                token.len(),
                buf.as_mut_ptr(),
                max_results,
            )
        };
        buf.truncate(count);
        buf.into_iter().map(|v| v as u32).collect()
    }

    /// Save the hash index to a file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let path_c = std::ffi::CString::new(path.to_str().ok_or("invalid path")?)
            .map_err(|e| format!("CString error: {}", e))?;
        let rc = unsafe { ffi::tgrep_hash_index_save(self.handle, path_c.as_ptr()) };
        if rc != 0 {
            Err(format!("hash_index_save failed (rc={})", rc))
        } else {
            Ok(())
        }
    }

    /// Load a hash index from a file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let path_c = std::ffi::CString::new(path.to_str().ok_or("invalid path")?)
            .map_err(|e| format!("CString error: {}", e))?;
        let handle = unsafe { ffi::tgrep_hash_index_load(path_c.as_ptr()) };
        if handle.is_null() {
            Err("hash_index_load returned null".into())
        } else {
            Ok(HashIndex { handle })
        }
    }
}

impl Drop for HashIndex {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::tgrep_hash_index_destroy(self.handle) };
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_lifecycle() {
        let mut idx = KeystoneIndex::new(16).unwrap();
        assert_eq!(idx.doc_count(), 0);

        // Add a document via streaming
        let mut stream = idx.begin_document(Some("test.txt")).unwrap();
        stream.feed(b"hello world").unwrap();
        stream.finish(None).unwrap();

        assert_eq!(idx.doc_count(), 1);
        idx.finalize().unwrap();
    }

    #[test]
    fn test_streaming_ingestion() {
        let mut idx = KeystoneIndex::new(16).unwrap();

        // Doc 1: "hello world" in chunks
        let mut s1 = idx.begin_document(Some("doc1")).unwrap();
        s1.feed(b"hel").unwrap();
        s1.feed(b"lo ").unwrap();
        s1.feed(b"wor").unwrap();
        s1.feed(b"ld").unwrap();
        s1.finish(None).unwrap();

        // Doc 2: "hello there" in one chunk
        let mut s2 = idx.begin_document(Some("doc2")).unwrap();
        s2.feed(b"hello there").unwrap();
        s2.finish(None).unwrap();

        // Doc 3: cancelled
        let s3 = idx.begin_document(Some("doc3")).unwrap();
        s3.cancel();

        assert_eq!(idx.doc_count(), 2);
        idx.finalize().unwrap();

        // "hello" should match both docs
        let candidates = idx.get_candidates(b"hello");
        assert_eq!(candidates.len(), 2);

        // "world" should match only doc 0
        let candidates = idx.get_candidates(b"world");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], 0);
    }

    #[test]
    fn test_frequency_lookup() {
        let mut idx = KeystoneIndex::new(16).unwrap();

        let mut s = idx.begin_document(None).unwrap();
        s.feed(b"aaaa").unwrap();
        s.finish(None).unwrap();

        let mut s = idx.begin_document(None).unwrap();
        s.feed(b"aaab").unwrap();
        s.finish(None).unwrap();

        idx.finalize().unwrap();

        // "aaa" trigram = 0x616161
        let aaa: u32 = ((b'a' as u32) << 16) | ((b'a' as u32) << 8) | (b'a' as u32);
        assert_eq!(idx.frequency(aaa), 2);

        // "bbb" trigram = 0x626262
        let bbb: u32 = ((b'b' as u32) << 16) | ((b'b' as u32) << 8) | (b'b' as u32);
        assert_eq!(idx.frequency(bbb), 0);
    }

    #[test]
    fn test_candidate_iterator() {
        let mut idx = KeystoneIndex::new(256).unwrap();

        for i in 0..100 {
            let mut s = idx.begin_document(None).unwrap();
            let content = if i % 2 == 0 {
                format!("foobar_{}", i)
            } else {
                format!("bazqux_{}", i)
            };
            s.feed(content.as_bytes()).unwrap();
            s.finish(None).unwrap();
        }

        idx.finalize().unwrap();

        // "foobar" should match 50 docs (even indices)
        let iter = idx.candidates(b"foobar").unwrap();
        let all = iter.collect_all().unwrap();
        assert_eq!(all.len(), 50);
        for &id in &all {
            assert_eq!(id % 2, 0);
        }

        // "bazqux" should match 50 docs (odd indices)
        let iter = idx.candidates(b"bazqux").unwrap();
        let all = iter.collect_all().unwrap();
        assert_eq!(all.len(), 50);
        for &id in &all {
            assert_eq!(id % 2, 1);
        }
    }

    #[test]
    fn test_memory_usage() {
        let idx = KeystoneIndex::new(16).unwrap();
        let empty = idx.memory_usage();
        assert!(empty > 0);
    }

    #[test]
    fn test_trigram_extraction() {
        let trigrams = KeystoneIndex::extract_trigrams(b"hello");
        // "hello" has trigrams: hel, ell, llo = 3
        assert_eq!(trigrams.len(), 3);
    }

    #[test]
    fn test_stats() {
        let mut idx = KeystoneIndex::new(16).unwrap();
        let mut s = idx.begin_document(None).unwrap();
        s.feed(b"test content here").unwrap();
        s.finish(None).unwrap();
        idx.finalize().unwrap();

        let stats = idx.stats();
        assert_eq!(stats.total_documents, 1);
        assert!(stats.bytes_indexed > 0);
        assert!(stats.unique_trigrams > 0);
    }

    #[test]
    fn test_atomic_write() {
        let path = std::env::temp_dir().join("tgrep_atomic_test.txt");
        let data = b"test data for atomic write";
        atomic_write(&path, data).unwrap();
        let read = std::fs::read(&path).unwrap();
        assert_eq!(read, data);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_wal_commit_and_replay() {
        let wal_dir = std::env::temp_dir().join("tgrep_wal_test_commit");
        let _ = std::fs::remove_dir_all(&wal_dir);
        std::fs::create_dir_all(&wal_dir).unwrap();

        // Write a committed transaction
        {
            let wal = Wal::create(&wal_dir).unwrap();
            let txn_id = 100;
            wal.begin(txn_id);
            wal.log_segment(txn_id, "seg_00000001.tgs", b"{\"files\":100}");
            wal.commit(txn_id).unwrap();
        }

        // Reopen and replay — should find the committed segment
        {
            let wal = Wal::create(&wal_dir).unwrap();
            let records = wal.replay().unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].key, b"seg_00000001.tgs");
            assert_eq!(records[0].op_type, 1); // INSERT
            assert_eq!(records[0].txn_id, 100);
        }

        std::fs::remove_dir_all(&wal_dir).unwrap();
    }

    #[test]
    fn test_wal_uncommitted_not_replayed() {
        let wal_dir = std::env::temp_dir().join("tgrep_wal_test_uncommitted");
        let _ = std::fs::remove_dir_all(&wal_dir);
        std::fs::create_dir_all(&wal_dir).unwrap();

        // Write a transaction WITHOUT committing (simulates crash)
        {
            let wal = Wal::create(&wal_dir).unwrap();
            let txn_id = 200;
            wal.begin(txn_id);
            wal.log_segment(txn_id, "seg_00000002.tgs", b"{\"files\":50}");
            // No commit — simulates crash mid-build
            wal.flush().unwrap();
        }

        // Reopen and replay — should NOT find the uncommitted segment
        {
            let wal = Wal::create(&wal_dir).unwrap();
            let records = wal.replay().unwrap();
            assert_eq!(records.len(), 0, "uncommitted segment should not be replayed");
        }

        std::fs::remove_dir_all(&wal_dir).unwrap();
    }

    #[test]
    fn test_wal_multiple_transactions() {
        let wal_dir = std::env::temp_dir().join("tgrep_wal_test_multi");
        let _ = std::fs::remove_dir_all(&wal_dir);
        std::fs::create_dir_all(&wal_dir).unwrap();

        // Write multiple committed transactions
        {
            let wal = Wal::create(&wal_dir).unwrap();

            // Transaction 1: commit
            wal.begin(1);
            wal.log_segment(1, "seg_00000001.tgs", b"meta1");
            wal.commit(1).unwrap();

            // Transaction 2: commit
            wal.begin(2);
            wal.log_segment(2, "seg_00000002.tgs", b"meta2");
            wal.commit(2).unwrap();

            // Transaction 3: no commit (crash)
            wal.begin(3);
            wal.log_segment(3, "seg_00000003.tgs", b"meta3");
            wal.flush().unwrap();
        }

        // Reopen and replay — should find only 2 committed segments
        {
            let wal = Wal::create(&wal_dir).unwrap();
            let records = wal.replay().unwrap();
            assert_eq!(records.len(), 2, "should replay only 2 committed transactions");
            assert!(records.iter().any(|r| r.key == b"seg_00000001.tgs"));
            assert!(records.iter().any(|r| r.key == b"seg_00000002.tgs"));
            assert!(!records.iter().any(|r| r.key == b"seg_00000003.tgs"));
        }

        std::fs::remove_dir_all(&wal_dir).unwrap();
    }

    #[test]
    fn test_hash_index_basic() {
        let mut idx = HashIndex::create(16).expect("create");
        idx.add(b"hello", 1).unwrap();
        idx.add(b"hello", 2).unwrap();
        idx.add(b"world", 3).unwrap();
        idx.finalize().unwrap();
        assert_eq!(idx.count(), 3);
        let results = idx.search_all(b"hello");
        assert_eq!(results.len(), 2);
        assert!(results.contains(&1));
        assert!(results.contains(&2));
        let results = idx.search_all(b"world");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], 3);
    }

    #[test]
    fn test_hash_index_missing() {
        let mut idx = HashIndex::create(16).expect("create");
        idx.add(b"hello", 1).unwrap();
        idx.finalize().unwrap();
        let results = idx.search_all(b"missing");
        assert!(results.is_empty());
    }

    #[test]
    fn test_hash_index_save_load() {
        let path = std::env::temp_dir().join("tgrep_hash_test.thi");
        let _ = std::fs::remove_file(&path);
        {
            let mut idx = HashIndex::create(16).expect("create");
            idx.add(b"alpha", 10).unwrap();
            idx.add(b"beta", 20).unwrap();
            idx.add(b"alpha", 30).unwrap();
            idx.finalize().unwrap();
            idx.save(&path).unwrap();
        }
        let loaded = HashIndex::load(&path).unwrap();
        assert_eq!(loaded.count(), 3);
        let results = loaded.search_all(b"alpha");
        assert_eq!(results.len(), 2);
        assert!(results.contains(&10));
        assert!(results.contains(&30));
        let results = loaded.search_all(b"beta");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], 20);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_hash_index_large_capacity() {
        // Test with many entries to exercise sort + search
        let mut idx = HashIndex::create(1024).expect("create");
        for i in 0..100 {
            let token = format!("token_{}", i);
            idx.add(token.as_bytes(), i).unwrap();
        }
        idx.finalize().unwrap();
        assert_eq!(idx.count(), 100);
        for i in 0..100 {
            let token = format!("token_{}", i);
            let results = idx.search_all(token.as_bytes());
            assert_eq!(results.len(), 1, "token {} should have 1 result", i);
            assert_eq!(results[0], i);
        }
        assert!(idx.search_all(b"missing_token").is_empty());
    }
}
