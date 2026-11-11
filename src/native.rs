// FFI bindings to KEYSTONE and QIHSE C helpers.
//
// Raw extern "C" declarations are in the `ffi` module. Safe RAII wrappers
// are in the `keystone` and `qihse` modules below.

use std::ffi::c_void;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

fn path_c(path: &Path) -> Result<std::ffi::CString, String> {
    std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())
}

// ── Raw FFI ──────────────────────────────────────────────────────────

pub mod ffi {
    use std::ffi::c_void;

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
        pub fn tgrep_keystone_feed_bytes(stream: *mut c_void, data: *const u8, len: usize) -> i32;
        pub fn tgrep_keystone_end_document(stream: *mut c_void, out_doc_id: *mut u32) -> i32;
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
            out_stats: *mut crate::native::KeystoneStatsRaw,
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
        pub fn tgrep_hash_index_save(handle: *mut c_void, path: *const std::ffi::c_char) -> i32;
        pub fn tgrep_hash_index_load(path: *const std::ffi::c_char) -> *mut c_void;

        // QIHSE btree-backed persistent word index (mmap search path)
        pub fn tgrep_word_index_create() -> *mut c_void;
        pub fn tgrep_word_index_destroy(handle: *mut c_void);
        pub fn tgrep_word_index_add(
            handle: *mut c_void,
            token: *const std::ffi::c_char,
            token_len: usize,
            doc_id: u64,
        ) -> i32;
        pub fn tgrep_word_index_save(handle: *mut c_void, path: *const std::ffi::c_char) -> i32;
        pub fn tgrep_word_index_save_for_segment(
            handle: *mut c_void,
            path: *const std::ffi::c_char,
            generation: u64,
            segment_fingerprint: u64,
        ) -> i32;
        // mmap-based load (returns qwi_mmap_t handle, not a btree)
        pub fn tgrep_word_index_load(path: *const std::ffi::c_char) -> *mut c_void;
        pub fn tgrep_word_index_load_for_segment(
            path: *const std::ffi::c_char,
            generation: u64,
            segment_fingerprint: u64,
        ) -> *mut c_void;
        pub fn tgrep_word_index_unload(handle: *mut c_void);
        pub fn tgrep_word_index_search(
            handle: *mut c_void,
            token: *const std::ffi::c_char,
            token_len: usize,
            out_doc_ids: *mut u64,
            max_results: usize,
        ) -> usize;
        pub fn tgrep_word_index_size(handle: *const c_void) -> usize;
        pub fn tgrep_word_index_import_block(
            builder: *mut c_void,
            mapped: *const c_void,
            block: usize,
            old_ids: *const u32,
            count: usize,
            first_document: u32,
        ) -> i32;

        // Phase 9: Anchor seeding + batch search
        pub fn tgrep_keystone_anchor_table_create() -> *mut c_void;
        pub fn tgrep_keystone_anchor_table_destroy(table: *mut c_void);
        pub fn tgrep_keystone_anchor_seed_batch(
            arr: *const i64,
            n: usize,
            table: *mut c_void,
            anchor_count: usize,
        ) -> usize;
        pub fn tgrep_keystone_search_batch_auto(
            arr: *const i64,
            n: usize,
            items: *mut crate::native::BatchItemRaw,
            num_items: usize,
            table: *mut c_void,
            tol: usize,
            config: *const crate::native::ParallelConfigRaw,
        ) -> usize;
        pub fn tgrep_keystone_detect_cpu_features() -> u32;

        // Phase 10: QIHSE optimization DB
        pub fn tgrep_optimization_create(
            max_entries: usize,
            storage_path: *const std::ffi::c_char,
        ) -> *mut crate::native::OptimizationDb;
        pub fn tgrep_optimization_free(db: *mut crate::native::OptimizationDb);
        pub fn tgrep_optimization_record(
            db: *mut crate::native::OptimizationDb,
            sig: *const crate::native::DataSignature,
            pipeline: i32,
            dimensions: usize,
            speedup: f64,
            confidence: f64,
            threads: i32,
            backend: i32,
        );
        pub fn tgrep_optimization_get_config(
            db: *mut crate::native::OptimizationDb,
            sig: *const crate::native::DataSignature,
            min_samples: usize,
            out: *mut crate::native::OptimizedConfig,
        ) -> i32;
        pub fn tgrep_optimization_record_anchor(
            db: *mut crate::native::OptimizationDb,
            sig: *const crate::native::DataSignature,
            anchor_count: usize,
            hit_rate: f64,
            speedup: f64,
            workload_type: i32,
        );
        pub fn tgrep_optimization_save(db: *mut crate::native::OptimizationDb) -> i32;
        pub fn tgrep_optimization_load(db: *mut crate::native::OptimizationDb) -> i32;
        pub fn tgrep_optimization_count(db: *const crate::native::OptimizationDb) -> usize;
        pub fn tgrep_optimization_set_learning(db: *mut crate::native::OptimizationDb, enable: i32);

        // Phase 11: QIHSE search-result cache (table store wrapper)
        pub fn tgrep_cache_create() -> *mut c_void;
        pub fn tgrep_cache_destroy(cache: *mut c_void);
        pub fn tgrep_cache_store_results(
            cache: *mut c_void,
            pattern: *const std::ffi::c_char,
            flags: i32,
            generation: i64,
            file_paths: *const *const std::ffi::c_char,
            num_paths: usize,
        ) -> i32;
        pub fn tgrep_cache_lookup(
            cache: *mut c_void,
            pattern: *const std::ffi::c_char,
            flags: i32,
            generation: i64,
            out_paths: *mut *mut *mut std::ffi::c_char,
            out_count: *mut usize,
        ) -> i32;
        pub fn tgrep_cache_free_paths(paths: *mut *mut std::ffi::c_char, count: usize);
        pub fn tgrep_cache_invalidate(
            cache: *mut c_void,
            pattern: *const std::ffi::c_char,
            flags: i32,
        ) -> i32;
        pub fn tgrep_cache_count(cache: *mut c_void) -> usize;
        pub fn tgrep_cache_save(cache: *mut c_void, path: *const std::ffi::c_char) -> i32;
        pub fn tgrep_cache_load_file(cache: *mut c_void, path: *const std::ffi::c_char) -> i32;
        pub fn tgrep_cache_get_entries(
            cache: *mut c_void,
            out_entries: *mut super::CacheEntryInfo,
            max_entries: usize,
            out_count: *mut usize,
        ) -> i32;
        pub fn tgrep_cache_free_entries(entries: *mut super::CacheEntryInfo, count: usize);

        // KEYSTONE performance stats
        pub fn keystone_get_performance_stats(stats: *mut super::KeystonePerformanceStats) -> i32;
        pub fn keystone_reset_performance_stats();
        pub fn keystone_set_performance_tracking(enabled: i32);
        pub fn keystone_detect_cpu_features() -> u32;
    }
}

// ── Phase 9 raw structs ────────────────────────────────────────────

// ── KEYSTONE performance stats (from keystone.h) ───────────────────

#[repr(C)]
#[derive(Default, Debug)]
pub struct KeystonePerformanceStats {
    pub total_time_ns: u64,
    pub search_time_ns: u64,
    pub total_searches: u64,
    pub successful_searches: u64,
    pub avg_search_time_ns: f64,
    pub search_success_rate: f64,
    pub speedup_vs_binary: f64,
    pub peak_memory_usage: usize,
    pub avg_memory_usage: usize,
    pub anchors_learned: u64,
    pub anchors_pruned: u64,
    pub cpu_features_used: u32,
    pub vectorization_efficiency: f64,
    pub memory_allocation_failures: u64,
    pub archive_bytes_read: u64,
    pub archive_decompress_time_ns: u64,
    pub archive_parse_time_ns: u64,
    pub archive_members_searched: u64,
}

// ── Cache entry info (from qihse_cache_wrapper.c) ──────────────────

#[repr(C)]
#[derive(Clone, Default)]
pub struct CacheEntryInfo {
    pub pattern: *mut std::ffi::c_char,
    pub flags: i32,
    pub generation: i64,
    pub file_count: usize,
}

#[repr(C)]
pub struct BatchItemRaw {
    pub key: i64,
    pub result: usize, // KEYSTONE_NOT_FOUND or index
    pub ordinal: usize,
}

#[repr(C)]
pub struct ParallelConfigRaw {
    pub num_threads: i32,
    pub use_thread_pool: i32,
    pub batch_chunk: usize,
}

/// KEYSTONE_NOT_FOUND sentinel (matches C definition).
pub const KEYSTONE_NOT_FOUND: usize = usize::MAX;

// ── Phase 10: QIHSE optimization DB structs ─────────────────────────

/// Data signature for optimization DB lookup.
/// Identifies a class of data (e.g. a trigram posting list) by its
/// statistical properties so that past performance can guide future config.
/// NOTE: The `_pad` field ensures the struct layout matches the C side
/// exactly, including padding between `data_type` and `entropy`.
#[repr(C)]
pub struct DataSignature {
    pub data_hash: u64,
    pub array_size: usize,
    pub data_type: i32,
    _pad: u32,
    pub entropy: f64,
    pub gap_variance: f64,
}

impl DataSignature {
    pub fn new(
        data_hash: u64,
        array_size: usize,
        data_type: i32,
        entropy: f64,
        gap_variance: f64,
    ) -> Self {
        Self {
            data_hash,
            array_size,
            data_type,
            _pad: 0,
            entropy,
            gap_variance,
        }
    }
}

/// Opaque C-owned optimization DB; its mutex must never move after initialization.
#[repr(C)]
pub struct OptimizationDb {
    _private: [u8; 0],
}

/// Result of an optimized config lookup.
#[repr(C)]
pub struct OptimizedConfig {
    pub found: i32,
    pub best_pipeline: i32,
    pub optimal_dimensions: usize,
    pub avg_speedup: f64,
    pub avg_confidence: f64,
    pub samples: usize,
    pub use_anchor_search: i32,
    pub optimal_anchor_count: usize,
    pub optimal_threads: i32,
    pub optimal_backend: i32,
}

impl Default for OptimizedConfig {
    fn default() -> Self {
        Self {
            found: 0,
            best_pipeline: 0,
            optimal_dimensions: 0,
            avg_speedup: 0.0,
            avg_confidence: 0.0,
            samples: 0,
            use_anchor_search: 0,
            optimal_anchor_count: 0,
            optimal_threads: 0,
            optimal_backend: 0,
        }
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
    pub fn begin_document(&mut self, name: Option<&str>) -> Result<DocumentStream<'_>, String> {
        let name_c = match name.map(std::ffi::CString::new).transpose() {
            Ok(c) => c,
            Err(_) => return Err("document name contains NUL byte".into()),
        };
        let name_ptr = name_c
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null());
        let stream = unsafe { ffi::tgrep_keystone_begin_document(self.handle, name_ptr) };
        if stream.is_null() {
            Err("failed to begin document".into())
        } else {
            Ok(DocumentStream {
                stream,
                _index: std::marker::PhantomData,
            })
        }
    }

    /// Extract unique trigrams from a pattern.
    pub fn extract_trigrams(pattern: &[u8]) -> Vec<u32> {
        let max_trigrams = pattern.len().saturating_sub(2);
        let mut buf = vec![0u32; max_trigrams];
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
    pub fn candidates(&self, pattern: &[u8]) -> Result<CandidateIter<'_>, String> {
        let iter = unsafe {
            ffi::tgrep_keystone_candidates_begin(
                self.handle,
                pattern.as_ptr() as *const std::ffi::c_char,
                pattern.len(),
            )
        };
        if iter.is_null() {
            Err("candidate iterator unavailable (index not finalized or allocation failed)".into())
        } else {
            Ok(CandidateIter {
                iter,
                _index: std::marker::PhantomData,
            })
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
        let max_grams = usize::try_from(stats.unique_trigrams)
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or("gram count exceeds addressable memory")?;
        let max_doc_ids = usize::try_from(stats.total_postings)
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or("posting count exceeds addressable memory")?;

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
///
/// The index remains exclusively borrowed until the stream is consumed.
///
/// ```compile_fail
/// use tgrep::native::KeystoneIndex;
/// let mut index = KeystoneIndex::new(1).unwrap();
/// let stream = index.begin_document(None).unwrap();
/// index.finalize().unwrap();
/// stream.cancel();
/// ```
pub struct DocumentStream<'a> {
    stream: *mut c_void,
    _index: std::marker::PhantomData<&'a mut KeystoneIndex>,
}

impl DocumentStream<'_> {
    /// Feed bytes to the current document.
    pub fn feed(&mut self, data: &[u8]) -> Result<(), String> {
        let rc = unsafe { ffi::tgrep_keystone_feed_bytes(self.stream, data.as_ptr(), data.len()) };
        if rc != KS_OK {
            Err(ks_err_str(rc).into())
        } else {
            Ok(())
        }
    }

    /// Finish the document and add to index. Returns assigned doc_id.
    pub fn finish(mut self, out_doc_id: Option<&mut u32>) -> Result<(), String> {
        let mut id: u32 = 0;
        let id_ptr = match out_doc_id {
            Some(p) => p as *mut u32,
            None => &mut id as *mut u32,
        };
        let rc = unsafe { ffi::tgrep_keystone_end_document(self.stream, id_ptr) };
        self.stream = std::ptr::null_mut(); // C consumes the stream on every return path.
        if rc != KS_OK {
            Err(ks_err_str(rc).into())
        } else {
            Ok(())
        }
    }

    /// Abort the document without indexing.
    pub fn cancel(mut self) {
        unsafe { ffi::tgrep_keystone_cancel_document(self.stream) };
        self.stream = std::ptr::null_mut();
    }
}

impl Drop for DocumentStream<'_> {
    fn drop(&mut self) {
        // If not explicitly finished or cancelled, cancel to avoid leak
        if !self.stream.is_null() {
            unsafe { ffi::tgrep_keystone_cancel_document(self.stream) };
        }
    }
}

/// RAII handle to a paginated candidate iterator.
///
/// ```compile_fail
/// use tgrep::native::KeystoneIndex;
/// let mut index = KeystoneIndex::new(1).unwrap();
/// index.finalize().unwrap();
/// let mut iter = index.candidates(b"abc").unwrap();
/// drop(index);
/// iter.next_batch(1).unwrap();
/// ```
pub struct CandidateIter<'a> {
    iter: *mut c_void,
    _index: std::marker::PhantomData<&'a KeystoneIndex>,
}

impl CandidateIter<'_> {
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

impl Drop for CandidateIter<'_> {
    fn drop(&mut self) {
        if !self.iter.is_null() {
            unsafe { ffi::tgrep_keystone_candidates_free(self.iter) };
        }
    }
}

// ── QIHSE safe wrappers ─────────────────────────────────────────────

/// Atomically write data to a file (tmp + fsync + rename + dir fsync).
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
    let c_path = path_c(path)?;
    let rc = unsafe { ffi::tgrep_qihse_atomic_write(c_path.as_ptr(), data.as_ptr(), data.len()) };
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
    let c_path = path_c(path)?;
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

#[derive(Default)]
struct ReplayCollector {
    records: Vec<WalReplayRecord>,
    invalid_record: bool,
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
    // The C WAL parser consumes attacker-writable files: never trust a
    // (ptr, len) pair — a null key with nonzero len is UB via from_raw_parts.
    let collector = unsafe { &mut *(user_data as *mut ReplayCollector) };
    if key.is_null() || key_len == 0 || (value.is_null() && value_len != 0) {
        collector.invalid_record = true;
        return false;
    }
    let key_slice = unsafe { std::slice::from_raw_parts(key as *const u8, key_len as usize) };
    let value_slice = if value.is_null() || value_len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(value, value_len as usize) }.to_vec()
    };
    collector.records.push(WalReplayRecord {
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
        let c_path = path_c(directory)?;
        let handle = unsafe { ffi::tgrep_wal_create(c_path.as_ptr()) };
        if handle.is_null() {
            Err(format!(
                "qihse_wal_create failed: {}",
                std::io::Error::last_os_error()
            ))
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
        let Ok(metadata_len) = u32::try_from(metadata.len()) else {
            return 0;
        };
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
                metadata_len,
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
        let mut collector = ReplayCollector::default();
        let cb_ptr = replay_trampoline as *const c_void;
        let user_data = &mut collector as *mut ReplayCollector as *mut c_void;
        let n = unsafe { ffi::tgrep_wal_replay(self.handle, cb_ptr, user_data) };
        if n < 0 || collector.invalid_record {
            Err(format!("wal_replay failed (rc={})", n))
        } else {
            Ok(collector.records)
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
    /// An empty Vec means no match; allocation failures and invalid IDs are errors.
    pub fn search_all(&self, token: &[u8]) -> Result<Vec<u32>, String> {
        let mut buf = vec![0u64; 4096];
        loop {
            let count = unsafe {
                ffi::tgrep_hash_index_search_all(
                    self.handle,
                    token.as_ptr() as *const std::ffi::c_char,
                    token.len(),
                    buf.as_mut_ptr(),
                    buf.len(),
                )
            };
            if count == usize::MAX {
                return Err("hash_index_search_all failed".into());
            }
            if count > buf.len() {
                buf.resize(count, 0);
                continue;
            }
            buf.truncate(count);
            return buf
                .into_iter()
                .map(|id| {
                    u32::try_from(id).map_err(|_| "hash index document ID exceeds u32".into())
                })
                .collect();
        }
    }

    /// Save the hash index to a file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let path_c = path_c(path)?;
        let rc = unsafe { ffi::tgrep_hash_index_save(self.handle, path_c.as_ptr()) };
        if rc != 0 {
            Err(format!("hash_index_save failed (rc={})", rc))
        } else {
            Ok(())
        }
    }

    /// Load a hash index from a file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let path_c = path_c(path)?;
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

// ── QIHSE btree-backed persistent word index ──────────────────────

/// Build-time wrapper: inserts tokens into a QIHSE btree, then saves
/// as a flat sorted file for mmap-based search.
pub struct WordIndexBuilder {
    handle: *mut c_void,
}

impl WordIndexBuilder {
    pub fn create() -> Result<Self, String> {
        let handle = unsafe { ffi::tgrep_word_index_create() };
        if handle.is_null() {
            Err("word_index_create returned null".into())
        } else {
            Ok(WordIndexBuilder { handle })
        }
    }

    pub fn add(&mut self, token: &[u8], doc_id: u32) -> Result<(), String> {
        let rc = unsafe {
            ffi::tgrep_word_index_add(
                self.handle,
                token.as_ptr() as *const std::ffi::c_char,
                token.len(),
                doc_id as u64,
            )
        };
        if rc != 0 {
            Err(format!("word_index_add failed (rc={})", rc))
        } else {
            Ok(())
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.save_for_segment(path, 0, 0)
    }

    /// Copy all checked word postings from an immutable snapshot, remapping
    /// sparse source IDs to the dense range starting at `first_document`.
    /// Any failure invalidates this unpublished builder; discard it.
    pub(crate) fn import_snapshot(
        &mut self,
        source: &MmapWordIndex,
        old_ids: &[u32],
        first_document: u32,
        checkpoint: impl FnMut() -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        source.import_or_validate(self.handle, old_ids, first_document, checkpoint)
    }

    /// Bind the compact sidecar to the segment generation and metadata checksum.
    pub fn save_for_segment(
        &self,
        path: &Path,
        generation: u64,
        segment_fingerprint: u64,
    ) -> Result<(), String> {
        let path_c = path_c(path)?;
        let rc = unsafe {
            ffi::tgrep_word_index_save_for_segment(
                self.handle,
                path_c.as_ptr(),
                generation,
                segment_fingerprint,
            )
        };
        if rc != 0 {
            Err(format!("word_index_save failed (rc={})", rc))
        } else {
            Ok(())
        }
    }
}

impl Drop for WordIndexBuilder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::tgrep_word_index_destroy(self.handle) };
        }
    }
}

/// Validate QWI v3 metadata and only the payload blocks touched by a query.
/// Unprotected legacy QWI is rejected so callers can fall back safely.
pub struct MmapWordIndex {
    handle: *mut c_void,
}

impl MmapWordIndex {
    pub(crate) fn validate_documents(
        &self,
        old_ids: &[u32],
        checkpoint: impl FnMut() -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        self.import_or_validate(std::ptr::null_mut(), old_ids, 0, checkpoint)
    }

    fn import_or_validate(
        &self,
        builder: *mut c_void,
        old_ids: &[u32],
        first_document: u32,
        mut checkpoint: impl FnMut() -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        if old_ids.windows(2).any(|ids| ids[0] >= ids[1])
            || old_ids.len() as u64 > u64::from(u32::MAX) + 1 - u64::from(first_document)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid word-index document remapping",
            ));
        }
        // QWI v3 stores exactly 512 entries per block except the final block.
        let blocks = self.size().div_ceil(512);
        for block in 0..blocks {
            checkpoint()?;
            let rc = unsafe {
                ffi::tgrep_word_index_import_block(
                    builder,
                    self.handle,
                    block,
                    old_ids.as_ptr(),
                    old_ids.len(),
                    first_document,
                )
            };
            if rc != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "word-index import failed: corrupt payload, unknown document, or allocation failure",
                ));
            }
        }
        checkpoint()
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        Self::load_for_segment(path, 0, 0)
    }

    pub fn load_for_segment(
        path: &Path,
        generation: u64,
        segment_fingerprint: u64,
    ) -> Result<Self, String> {
        let path_c = path_c(path)?;
        let handle = unsafe {
            ffi::tgrep_word_index_load_for_segment(path_c.as_ptr(), generation, segment_fingerprint)
        };
        if handle.is_null() {
            Err("word index is legacy, corrupt, unreadable, or bound to another segment".into())
        } else {
            Ok(MmapWordIndex { handle })
        }
    }

    pub fn search(&self, token: &[u8], max_results: usize) -> Result<Vec<u64>, String> {
        if token.is_empty() || max_results == 0 {
            return Ok(Vec::new());
        }
        // Most tokens have few matching files. Avoid allocating from the total
        // sidecar entry count (which can be hundreds of thousands per segment)
        // for every query. Grow only when the result buffer fills; a full buffer
        // may mean either an exact fit or that more matches remain, so probe at
        // a larger capacity until a call returns fewer results than it can hold.
        let mut capacity = max_results.clamp(1, 4096);
        loop {
            let mut out = Vec::new();
            out.try_reserve_exact(capacity)
                .map_err(|error| format!("word-index result allocation failed: {error}"))?;
            out.resize(capacity, 0u64);
            let count = unsafe {
                ffi::tgrep_word_index_search(
                    self.handle,
                    token.as_ptr() as *const std::ffi::c_char,
                    token.len(),
                    out.as_mut_ptr(),
                    capacity,
                )
            };
            if count == usize::MAX {
                return Err("word-index payload failed integrity validation".into());
            }
            if count > capacity {
                return Err("word-index search returned more results than requested".into());
            }
            out.truncate(count);
            if count < capacity || capacity == max_results {
                return Ok(out);
            }
            capacity = capacity.saturating_mul(2).min(max_results);
        }
    }

    pub fn size(&self) -> usize {
        unsafe { ffi::tgrep_word_index_size(self.handle) }
    }
}

impl Drop for MmapWordIndex {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::tgrep_word_index_unload(self.handle) };
        }
    }
}

// ── Phase 9: Anchor table + batch search safe wrappers ─────────────

/// Safe RAII wrapper around a KEYSTONE anchor table.
/// Pre-populated with evenly-spaced anchors for fast interpolation search.
pub struct AnchorTable {
    handle: *mut c_void,
}

impl AnchorTable {
    /// Create a new anchor table.
    pub fn create() -> Result<Self, String> {
        let handle = unsafe { ffi::tgrep_keystone_anchor_table_create() };
        if handle.is_null() {
            Err("anchor_table_create returned null".into())
        } else {
            Ok(AnchorTable { handle })
        }
    }

    /// Seed the anchor table with evenly-spaced anchors from a sorted i64 array.
    /// Returns the number of anchors actually inserted.
    pub fn seed_batch(&mut self, arr: &[i64], anchor_count: usize) -> usize {
        unsafe {
            ffi::tgrep_keystone_anchor_seed_batch(
                arr.as_ptr(),
                arr.len(),
                self.handle,
                anchor_count,
            )
        }
    }

    /// Get the raw handle for passing to batch search.
    pub fn handle(&self) -> *mut c_void {
        self.handle
    }
}

impl Drop for AnchorTable {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::tgrep_keystone_anchor_table_destroy(self.handle) };
        }
    }
}

/// Detect CPU SIMD features (bitmask).
pub fn detect_cpu_features() -> u32 {
    unsafe { ffi::tgrep_keystone_detect_cpu_features() }
}

/// Batch search multiple keys in a sorted i64 array using KEYSTONE's
/// auto-backend router (scalar/SSE4.2/OpenMP).
/// Returns a Vec of indices (KEYSTONE_NOT_FOUND for misses).
pub fn batch_search_auto(
    arr: &[i64],
    keys: &[i64],
    table: Option<&AnchorTable>,
    threads: usize,
) -> Vec<usize> {
    if keys.is_empty() {
        return Vec::new();
    }
    let mut items: Vec<BatchItemRaw> = keys
        .iter()
        .enumerate()
        .map(|(i, &k)| BatchItemRaw {
            key: k,
            result: KEYSTONE_NOT_FOUND,
            ordinal: i,
        })
        .collect();

    let table_ptr = table.map(|t| t.handle()).unwrap_or(std::ptr::null_mut());

    let pcfg = i32::try_from(threads)
        .ok()
        .filter(|count| *count > 0)
        .map(|num_threads| ParallelConfigRaw {
            num_threads,
            use_thread_pool: 1,
            batch_chunk: 256,
        });

    unsafe {
        ffi::tgrep_keystone_search_batch_auto(
            arr.as_ptr(),
            arr.len(),
            items.as_mut_ptr(),
            items.len(),
            table_ptr,
            4, // tolerance
            pcfg.as_ref()
                .map(|p| p as *const _)
                .unwrap_or(std::ptr::null()),
        );
    }

    // Sort results by ordinal and extract
    let mut results = vec![KEYSTONE_NOT_FOUND; keys.len()];
    for item in &items {
        if item.ordinal < results.len() {
            results[item.ordinal] = item.result;
        }
    }
    results
}

// ── Phase 10: QIHSE optimization DB safe wrapper ────────────────────

/// Safe RAII wrapper around the QIHSE optimization database.
///
/// Records per-data-signature performance and recommends the best-known
/// configuration (pipeline, dimensions, threads, backend, anchor count)
/// for future searches with similar data signatures.
///
/// Persisted as a binary file at the given storage path.
/// The C object uses a mutex; this wrapper requires mutable access for writes.
pub struct OptimizationDatabase {
    db: *mut OptimizationDb,
}

impl OptimizationDatabase {
    /// Create a new optimization DB with the given capacity and optional
    /// storage path. If a storage path is given and the file exists, entries
    /// are loaded automatically.
    pub fn create(max_entries: usize, storage_path: Option<&str>) -> Result<Self, String> {
        let c_path = match storage_path.map(std::ffi::CString::new).transpose() {
            Ok(p) => p,
            Err(_) => return Err("storage_path contains NUL byte".into()),
        };
        let path_ptr = c_path
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null());
        let db = unsafe { ffi::tgrep_optimization_create(max_entries, path_ptr) };
        if db.is_null() {
            return Err("optimization_create failed".into());
        }
        Ok(OptimizationDatabase { db })
    }

    /// Record performance for a data signature.
    #[allow(clippy::too_many_arguments)] // Mirrors the existing native optimizer ABI.
    pub fn record(
        &mut self,
        sig: &DataSignature,
        pipeline: i32,
        dimensions: usize,
        speedup: f64,
        confidence: f64,
        threads: i32,
        backend: i32,
    ) {
        unsafe {
            ffi::tgrep_optimization_record(
                self.db, sig, pipeline, dimensions, speedup, confidence, threads, backend,
            );
        }
    }

    /// Record anchor performance for a data signature.
    pub fn record_anchor(
        &mut self,
        sig: &DataSignature,
        anchor_count: usize,
        hit_rate: f64,
        speedup: f64,
        workload_type: i32,
    ) {
        unsafe {
            ffi::tgrep_optimization_record_anchor(
                self.db,
                sig,
                anchor_count,
                hit_rate,
                speedup,
                workload_type,
            );
        }
    }

    /// Get the optimized config for a data signature.
    /// Returns Some(config) if a matching entry with enough samples exists.
    pub fn get_config(
        &mut self,
        sig: &DataSignature,
        min_samples: usize,
    ) -> Option<OptimizedConfig> {
        let mut out = OptimizedConfig::default();
        let found =
            unsafe { ffi::tgrep_optimization_get_config(self.db, sig, min_samples, &mut out) };
        if found != 0 {
            Some(out)
        } else {
            None
        }
    }

    /// Save the DB to disk.
    pub fn save(&mut self) -> Result<(), String> {
        let rc = unsafe { ffi::tgrep_optimization_save(self.db) };
        if rc != 0 {
            Err(format!("optimization_save failed with code {}", rc))
        } else {
            Ok(())
        }
    }

    /// Load the DB from disk.
    pub fn load(&mut self) -> Result<(), String> {
        let rc = unsafe { ffi::tgrep_optimization_load(self.db) };
        if rc != 0 {
            Err(format!("optimization_load failed with code {}", rc))
        } else {
            Ok(())
        }
    }

    /// Number of entries in the DB.
    pub fn count(&self) -> usize {
        unsafe { ffi::tgrep_optimization_count(self.db) }
    }

    /// Enable or disable learning.
    pub fn set_learning(&mut self, enable: bool) {
        unsafe { ffi::tgrep_optimization_set_learning(self.db, if enable { 1 } else { 0 }) };
    }
}

impl Drop for OptimizationDatabase {
    fn drop(&mut self) {
        unsafe { ffi::tgrep_optimization_free(self.db) };
    }
}

/// Compute a data signature for a trigram query.
/// The hash is based on the trigram keys and the total posting-list size.
pub fn compute_query_signature(grams: &[u32], total_postings: usize) -> DataSignature {
    let mut hash: u64 = 0xCBF29CE484222325; // FNV-1a offset basis
    for &g in grams {
        hash ^= g as u64;
        hash = hash.wrapping_mul(0x100000001B3); // FNV-1a prime
    }
    hash ^= total_postings as u64;
    hash = hash.wrapping_mul(0x100000001B3);
    DataSignature::new(hash, total_postings, 0, 0.0, 0.0)
}

// ── Phase 11: QIHSE search-result cache safe wrapper ─────────────────

/// Cache flags (must match C side).
pub const CACHE_FLAG_CASE_INSENSITIVE: i32 = 0x01;
pub const CACHE_FLAG_WORD_REGEXP: i32 = 0x02;
pub const CACHE_FLAG_FIXED_STRINGS: i32 = 0x04;

/// A unique cache entry (pattern + flags + generation → file count).
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub pattern: String,
    pub flags: i32,
    pub generation: i64,
    pub file_count: usize,
}

/// Safe RAII wrapper around the QIHSE table-store-backed search cache.
///
/// Caches the file list matching a (pattern, flags, generation) tuple so
/// repeated identical searches are O(1) instead of re-running the trigram
/// intersection + verification pipeline. Cache invalidation is automatic:
/// a new build increments the manifest generation, so old entries don't
/// match and are ignored on lookup.
pub struct SearchCache {
    handle: *mut c_void,
}

impl SearchCache {
    /// Create a new empty cache.
    pub fn create() -> Result<Self, String> {
        let handle = unsafe { ffi::tgrep_cache_create() };
        if handle.is_null() {
            Err("cache_create returned null".into())
        } else {
            Ok(SearchCache { handle })
        }
    }

    /// Store search results in the cache. One row per matching file path.
    pub fn store_results(
        &self,
        pattern: &str,
        flags: i32,
        generation: i64,
        file_paths: &[String],
    ) -> i32 {
        let pattern_c = match std::ffi::CString::new(pattern) {
            Ok(c) => c,
            Err(_) => return -1,
        };
        // Build C string array
        let mut cstrings = Vec::with_capacity(file_paths.len());
        for s in file_paths {
            match std::ffi::CString::new(s.as_str()) {
                Ok(c) => cstrings.push(c),
                Err(_) => return -1,
            }
        }
        let ptrs: Vec<*const std::ffi::c_char> = cstrings.iter().map(|s| s.as_ptr()).collect();
        unsafe {
            ffi::tgrep_cache_store_results(
                self.handle,
                pattern_c.as_ptr(),
                flags,
                generation,
                ptrs.as_ptr(),
                ptrs.len(),
            )
        }
    }

    /// Look up cached results for a query.
    /// Returns Some(paths) if the cache has entries for this
    /// (pattern, flags, generation), or None if no cache hit.
    pub fn lookup(&self, pattern: &str, flags: i32, generation: i64) -> Option<Vec<String>> {
        let pattern_c = std::ffi::CString::new(pattern).ok()?;
        let mut out_paths: *mut *mut std::ffi::c_char = std::ptr::null_mut();
        let mut out_count: usize = 0;
        let rc = unsafe {
            ffi::tgrep_cache_lookup(
                self.handle,
                pattern_c.as_ptr(),
                flags,
                generation,
                &mut out_paths,
                &mut out_count,
            )
        };
        if rc < 0 || out_count == 0 {
            return None;
        }
        let mut paths = Vec::with_capacity(out_count);
        unsafe {
            for i in 0..out_count {
                let ptr = *out_paths.add(i);
                if !ptr.is_null() {
                    paths.push(std::ffi::CStr::from_ptr(ptr).to_string_lossy().to_string());
                }
            }
            ffi::tgrep_cache_free_paths(out_paths, out_count);
        }
        Some(paths)
    }

    /// Invalidate entries for a pattern+flags.
    /// (Generation-based invalidation makes this largely unnecessary.)
    pub fn invalidate(&self, pattern: &str, flags: i32) -> i32 {
        let pattern_c = match std::ffi::CString::new(pattern) {
            Ok(c) => c,
            Err(_) => return -1,
        };
        unsafe { ffi::tgrep_cache_invalidate(self.handle, pattern_c.as_ptr(), flags) }
    }

    /// Number of cached rows.
    pub fn count(&self) -> usize {
        unsafe { ffi::tgrep_cache_count(self.handle) }
    }

    /// Get unique cache entries (pattern, flags, generation, file_count).
    pub fn get_entries(&self, max_entries: usize) -> Vec<CacheEntry> {
        if max_entries == 0 {
            return Vec::new();
        }
        let mut entries: Vec<CacheEntryInfo> = Vec::with_capacity(max_entries);
        // Safety: CacheEntryInfo is repr(C) and we allocate zeroed entries
        entries.resize(
            max_entries,
            CacheEntryInfo {
                pattern: std::ptr::null_mut(),
                flags: 0,
                generation: 0,
                file_count: 0,
            },
        );
        let mut out_count: usize = 0;
        let rc = unsafe {
            ffi::tgrep_cache_get_entries(
                self.handle,
                entries.as_mut_ptr(),
                max_entries,
                &mut out_count,
            )
        };
        if rc < 0 {
            return Vec::new();
        }
        let mut result = Vec::with_capacity(out_count);
        for e in entries.iter().take(out_count) {
            let pattern = if e.pattern.is_null() {
                String::new()
            } else {
                unsafe { std::ffi::CStr::from_ptr(e.pattern) }
                    .to_string_lossy()
                    .to_string()
            };
            result.push(CacheEntry {
                pattern,
                flags: e.flags,
                generation: e.generation,
                file_count: e.file_count,
            });
        }
        unsafe {
            ffi::tgrep_cache_free_entries(entries.as_mut_ptr(), out_count);
        }
        result
    }

    /// Save cache to a .qsc file.
    pub fn save(&self, path: &str) -> Result<i32, String> {
        let c_path = std::ffi::CString::new(path).map_err(|e| e.to_string())?;
        let rc = unsafe { ffi::tgrep_cache_save(self.handle, c_path.as_ptr()) };
        if rc < 0 {
            Err(format!("cache_save failed (rc={})", rc))
        } else {
            Ok(rc)
        }
    }

    /// Load cache from a .qsc file.
    pub fn load(&self, path: &str) -> Result<i32, String> {
        let c_path = std::ffi::CString::new(path).map_err(|e| e.to_string())?;
        let rc = unsafe { ffi::tgrep_cache_load_file(self.handle, c_path.as_ptr()) };
        if rc < 0 {
            Err(format!("cache_load failed (rc={})", rc))
        } else {
            Ok(rc)
        }
    }
}

impl Drop for SearchCache {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::tgrep_cache_destroy(self.handle) };
        }
    }
}

/// Compute cache flags from search config.
pub fn compute_cache_flags(case_insensitive: bool, word_regexp: bool, fixed_strings: bool) -> i32 {
    let mut flags = 0i32;
    if case_insensitive {
        flags |= CACHE_FLAG_CASE_INSENSITIVE;
    }
    if word_regexp {
        flags |= CACHE_FLAG_WORD_REGEXP;
    }
    if fixed_strings {
        flags |= CACHE_FLAG_FIXED_STRINGS;
    }
    flags
}

// ── KEYSTONE performance stats safe wrapper ─────────────────────────

/// Get KEYSTONE global performance statistics.
pub fn get_keystone_performance_stats() -> Option<KeystonePerformanceStats> {
    let mut stats = KeystonePerformanceStats::default();
    let rc = unsafe { ffi::keystone_get_performance_stats(&mut stats) };
    if rc == 0 {
        Some(stats)
    } else {
        None
    }
}

/// Reset KEYSTONE performance statistics.
pub fn reset_keystone_performance_stats() {
    unsafe { ffi::keystone_reset_performance_stats() };
}

/// Enable or disable KEYSTONE performance tracking.
pub fn set_keystone_performance_tracking(enabled: bool) {
    unsafe { ffi::keystone_set_performance_tracking(if enabled { 1 } else { 0 }) };
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

        assert_eq!(
            idx.candidates(b"a").unwrap().collect_all().unwrap().len(),
            100
        );
    }

    #[test]
    fn test_collect_postings_after_finalize() {
        let mut idx = KeystoneIndex::new(2).unwrap();
        for content in [b"abc".as_slice(), b"abd".as_slice()] {
            let mut stream = idx.begin_document(None).unwrap();
            stream.feed(content).unwrap();
            stream.finish(None).unwrap();
        }
        idx.finalize().unwrap();
        let postings = idx.collect_postings().unwrap();
        let abc = ((b'a' as u32) << 16) | ((b'b' as u32) << 8) | b'c' as u32;
        let abd = ((b'a' as u32) << 16) | ((b'b' as u32) << 8) | b'd' as u32;
        assert_eq!(postings.get(&abc), Some(&vec![0]));
        assert_eq!(postings.get(&abd), Some(&vec![1]));
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
    fn trigram_extraction_handles_patterns_with_more_than_64_unique_windows() {
        let pattern: Vec<u8> = (0..=120).collect();
        let trigrams = KeystoneIndex::extract_trigrams(&pattern);
        assert_eq!(trigrams.len(), pattern.len() - 2);
        assert_eq!(trigrams, crate::search::extract_unique_trigrams(&pattern));
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
    fn test_native_paths_preserve_non_utf8_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let mut name = format!("tgrep-native-{}-", std::process::id()).into_bytes();
        name.push(0xff);
        let path = std::env::temp_dir().join(std::ffi::OsString::from_vec(name));
        atomic_write(&path, b"content").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"content");
        let mut index = HashIndex::create(2).unwrap();
        index.add(b"word", 7).unwrap();
        index.finalize().unwrap();
        index.save(&path).unwrap();
        assert_eq!(
            HashIndex::load(&path).unwrap().search_all(b"word").unwrap(),
            vec![7]
        );
        std::fs::remove_file(path).unwrap();
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
            assert_eq!(
                records.len(),
                0,
                "uncommitted segment should not be replayed"
            );
        }

        std::fs::remove_dir_all(&wal_dir).unwrap();
    }

    #[test]
    fn test_wal_replay_rejects_invalid_pointer_pairs() {
        let mut collector = ReplayCollector::default();
        let context = &mut collector as *mut ReplayCollector as *mut c_void;
        assert!(!replay_trampoline(
            std::ptr::null(),
            1,
            std::ptr::null(),
            0,
            1,
            1,
            context,
        ));
        assert!(collector.invalid_record);
        collector.invalid_record = false;
        assert!(!replay_trampoline(
            c"segment".as_ptr(),
            7,
            std::ptr::null(),
            1,
            1,
            1,
            context,
        ));
        assert!(collector.invalid_record);
        assert!(collector.records.is_empty());
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
            assert_eq!(
                records.len(),
                2,
                "should replay only 2 committed transactions"
            );
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
        let results = idx.search_all(b"hello").unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.contains(&1));
        assert!(results.contains(&2));
        let results = idx.search_all(b"world").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], 3);
    }

    #[test]
    fn test_hash_index_missing() {
        let mut idx = HashIndex::create(16).expect("create");
        idx.add(b"hello", 1).unwrap();
        idx.finalize().unwrap();
        let results = idx.search_all(b"missing").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_hash_index_rejects_out_of_range_document_ids() {
        let mut index = HashIndex::create(2).unwrap();
        let token = b"oversized";
        assert_eq!(
            unsafe {
                ffi::tgrep_hash_index_add(
                    index.handle,
                    token.as_ptr() as *const std::ffi::c_char,
                    token.len(),
                    u32::MAX as u64 + 1,
                )
            },
            0
        );
        index.finalize().unwrap();
        assert!(index.search_all(token).is_err());
        assert_eq!(
            unsafe {
                ffi::tgrep_hash_index_search_all(
                    std::ptr::null_mut(),
                    token.as_ptr() as *const std::ffi::c_char,
                    token.len(),
                    std::ptr::null_mut(),
                    0,
                )
            },
            usize::MAX
        );
    }

    #[test]
    fn test_hash_index_all_matches_across_buffer_boundary() {
        for count in [4095u32, 4096, 4097, 5000] {
            let mut idx = HashIndex::create(8).unwrap();
            for doc in 0..count {
                idx.add(b"common", doc).unwrap();
            }
            idx.finalize().unwrap();
            let mut matches = idx.search_all(b"common").unwrap();
            matches.sort_unstable();
            assert_eq!(matches, (0..count).collect::<Vec<_>>());
        }
    }

    #[test]
    fn test_hash_index_matches_counted_binary_tokens() {
        let mut idx = HashIndex::create(8).unwrap();
        for (token, doc) in [
            (&b"ab"[..], 1),
            (&b"ab\0cd"[..], 2),
            (&b"ab\0de"[..], 3),
            (&b"ab\0cd"[..], 4),
            (&b"\0ab"[..], 5),
            (&b""[..], 6),
            (&b"\xff\0\xfe"[..], 7),
        ] {
            idx.add(token, doc).unwrap();
        }
        idx.finalize().unwrap();
        assert_eq!(idx.search_all(b"ab").unwrap(), vec![1]);
        let mut matches = idx.search_all(b"ab\0cd").unwrap();
        matches.sort_unstable();
        assert_eq!(matches, vec![2, 4]);
        assert!(idx.search_all(b"ab\0missing").unwrap().is_empty());
        assert_eq!(idx.search_all(b"\0ab").unwrap(), vec![5]);
        assert_eq!(idx.search_all(b"").unwrap(), vec![6]);
        assert_eq!(idx.search_all(b"\xff\0\xfe").unwrap(), vec![7]);
    }

    #[test]
    fn test_hash_index_query_across_stack_buffer_boundary() {
        for len in [4095, 4096, 4097] {
            let token = vec![b'a'; len];
            let mut idx = HashIndex::create(8).unwrap();
            idx.add(&token, 42).unwrap();
            idx.finalize().unwrap();
            assert_eq!(idx.search_all(&token).unwrap(), vec![42]);
        }
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
        let results = loaded.search_all(b"alpha").unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.contains(&10));
        assert!(results.contains(&30));
        let results = loaded.search_all(b"beta").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], 20);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_qihse_word_index_basic() {
        let mut idx = WordIndexBuilder::create().expect("create");
        idx.add(b"alpha", 10).unwrap();
        idx.add(b"beta", 20).unwrap();
        idx.add(b"alpha", 30).unwrap();
        idx.add(b"gamma", 40).unwrap();
        let path = std::env::temp_dir().join("tgrep_qwi_basic_test.qwi");
        idx.save(&path).unwrap();
        let loaded = MmapWordIndex::load(&path).unwrap();
        assert!(loaded.size() >= 4);
        let results = loaded.search(b"alpha", 64).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.contains(&10));
        assert!(results.contains(&30));
        let results = loaded.search(b"beta", 64).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], 20);
        assert!(loaded.search(b"missing", 64).unwrap().is_empty());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_qihse_word_index_grows_results_only_for_common_tokens() {
        let path = std::env::temp_dir().join(format!(
            "tgrep_qwi_growth_{}_{}.qwi",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut index = WordIndexBuilder::create().unwrap();
        for doc_id in 0..5000u32 {
            index.add(b"common", doc_id).unwrap();
        }
        index.add(b"rare", 6000).unwrap();
        index.save(&path).unwrap();

        let loaded = MmapWordIndex::load(&path).unwrap();
        assert!(loaded.search(b"absent", loaded.size()).unwrap().is_empty());
        let mut common = loaded.search(b"common", loaded.size()).unwrap();
        common.sort_unstable();
        assert_eq!(common, (0..5000u64).collect::<Vec<_>>());
        assert_eq!(loaded.search(b"rare", loaded.size()).unwrap(), vec![6000]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_qihse_word_index_save_load() {
        let path = std::env::temp_dir().join("tgrep_qwi_test.qwi");
        let _ = std::fs::remove_file(&path);
        {
            let mut idx = WordIndexBuilder::create().expect("create");
            idx.add(b"hello", 1).unwrap();
            idx.add(b"world", 2).unwrap();
            idx.add(b"hello", 3).unwrap();
            idx.add(b"foo", 4).unwrap();
            idx.save(&path).unwrap();
        }
        let loaded = MmapWordIndex::load(&path).unwrap();
        assert!(loaded.size() >= 4);
        let results = loaded.search(b"hello", 64).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.contains(&1));
        assert!(results.contains(&3));
        let results = loaded.search(b"world", 64).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], 2);
        assert!(loaded.search(b"missing", 64).unwrap().is_empty());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_qihse_word_index_binding_and_payload_errors_reach_rust() {
        let path = std::env::temp_dir().join(format!(
            "tgrep_qwi_bound_{}_{}.qwi",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut index = WordIndexBuilder::create().unwrap();
        index.add(b"alpha", 9).unwrap();
        index.save_for_segment(&path, 41, 123).unwrap();
        assert!(MmapWordIndex::load_for_segment(&path, 42, 123).is_err());
        assert!(MmapWordIndex::load_for_segment(&path, 41, 456).is_err());
        let loaded = MmapWordIndex::load_for_segment(&path, 41, 123).unwrap();
        assert_eq!(loaded.search(b"alpha", 2).unwrap(), [9]);
        drop(loaded);
        let mut bytes = std::fs::read(&path).unwrap();
        let blocks = u64::from_le_bytes(bytes[24..32].try_into().unwrap()) as usize;
        bytes[56 + 24 * blocks + 7] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        let loaded = MmapWordIndex::load_for_segment(&path, 41, 123).unwrap();
        assert!(loaded.search(b"alpha", 2).is_err());
        assert!(loaded.search(b"absent", 2).unwrap().is_empty());
        drop(loaded);
        std::fs::remove_file(path).unwrap();
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
            let results = idx.search_all(token.as_bytes()).unwrap();
            assert_eq!(results.len(), 1, "token {} should have 1 result", i);
            assert_eq!(results[0], i);
        }
        assert!(idx.search_all(b"missing_token").unwrap().is_empty());
    }

    #[test]
    fn test_anchor_table_create_and_seed() {
        let mut table = AnchorTable::create().expect("create anchor table");
        // Sorted array of 100 values
        let arr: Vec<i64> = (0..100i64).collect();
        let inserted = table.seed_batch(&arr, 16);
        assert!(inserted > 0, "should insert some anchors");
    }

    #[test]
    fn test_batch_search_auto_basic() {
        let arr: Vec<i64> = vec![1, 5, 10, 15, 20, 25, 30, 35, 40, 45];
        let keys: Vec<i64> = vec![1, 15, 30, 45, 99];
        let results = batch_search_auto(&arr, &keys, None, 0);
        assert_eq!(results.len(), 5);
        assert_eq!(results[0], 0, "key 1 at index 0");
        assert_eq!(results[1], 3, "key 15 at index 3");
        assert_eq!(results[2], 6, "key 30 at index 6");
        assert_eq!(results[3], 9, "key 45 at index 9");
        assert_eq!(results[4], KEYSTONE_NOT_FOUND, "key 99 not found");
        assert_eq!(batch_search_auto(&arr, &keys, None, usize::MAX), results);
    }

    #[test]
    fn test_batch_search_auto_with_anchor_table() {
        let arr: Vec<i64> = (0..1000i64).collect();
        let mut table = AnchorTable::create().expect("create anchor table");
        table.seed_batch(&arr, 32);

        let keys: Vec<i64> = (0..1000i64).step_by(100).collect();
        let results = batch_search_auto(&arr, &keys, Some(&table), 0);
        assert_eq!(results.len(), 10);
        for (i, &r) in results.iter().enumerate() {
            assert_eq!(r, i * 100, "key {} should be at index {}", keys[i], i * 100);
        }
    }

    #[test]
    fn test_detect_cpu_features() {
        let features = detect_cpu_features();
        // SSE4.2 should be present on this CPU
        assert!(features != 0, "should detect some CPU features");
    }

    // ── Phase 10: Optimization DB tests ──

    #[test]
    fn test_optimization_db_create_and_record() {
        let mut db = OptimizationDatabase::create(100, None).expect("create opt db");
        assert_eq!(db.count(), 0);
        let sig = DataSignature::new(42, 1000, 0, 0.5, 0.1);
        db.record(&sig, 1, 64, 2.5, 0.9, 4, 1);
        assert_eq!(db.count(), 1);
        // Not enough samples yet (min_samples=5)
        assert!(db.get_config(&sig, 5).is_none());
    }

    #[test]
    fn test_optimization_db_survives_rust_move() {
        let mut first = OptimizationDatabase::create(4, None).unwrap();
        let mut second = OptimizationDatabase::create(4, None).unwrap();
        let sig = DataSignature::new(42, 100, 0, 0.5, 0.1);
        first.record(&sig, 1, 8, 2.0, 0.9, 2, 0);
        std::mem::swap(&mut first, &mut second);
        assert_eq!(first.count(), 0);
        assert_eq!(second.count(), 1);
        second.record(&sig, 1, 8, 2.0, 0.9, 2, 0);
        assert_eq!(second.get_config(&sig, 2).unwrap().samples, 2);
    }

    #[test]
    fn test_optimization_db_get_config_after_samples() {
        let mut db = OptimizationDatabase::create(100, None).expect("create opt db");
        let sig = DataSignature::new(99, 5000, 0, 0.7, 0.2);
        // Record 6 samples
        for _ in 0..6 {
            db.record(&sig, 2, 128, 3.0, 0.95, 4, 1);
        }
        let cfg = db.get_config(&sig, 5).expect("should have config");
        assert_eq!(cfg.found, 1);
        assert_eq!(cfg.best_pipeline, 2);
        assert_eq!(cfg.optimal_dimensions, 128);
        assert!(cfg.avg_speedup > 0.0);
        assert_eq!(cfg.optimal_threads, 4);
        assert_eq!(cfg.optimal_backend, 1);
    }

    #[test]
    fn test_optimization_db_persistence() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!(
            "tgrep_opt_test_{}_{:?}.qdb",
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_file(&tmp);
        {
            let mut db = OptimizationDatabase::create(100, Some(tmp.to_str().unwrap()))
                .expect("create opt db with path");
            let sig = DataSignature::new(123, 10000, 0, 0.8, 0.3);
            for _ in 0..6 {
                db.record(&sig, 1, 256, 4.0, 0.92, 8, 2);
            }
            assert_eq!(db.count(), 1, "should have 1 entry before save");
            db.save().expect("save");
            assert!(
                std::fs::exists(&tmp).unwrap_or(false),
                "file should exist after save"
            );
            let file_size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
            assert!(
                file_size > 0,
                "file should not be empty, size={}",
                file_size
            );
        }
        // Load into a new DB
        let mut db2 =
            OptimizationDatabase::create(100, Some(tmp.to_str().unwrap())).expect("load opt db");
        assert_eq!(db2.count(), 1, "should have 1 entry after load");
        let sig = DataSignature::new(123, 10000, 0, 0.8, 0.3);
        let cfg = db2
            .get_config(&sig, 5)
            .expect("should have config after load");
        assert_eq!(cfg.found, 1);
        assert_eq!(cfg.best_pipeline, 1);
        assert_eq!(cfg.optimal_dimensions, 256);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_optimization_db_anchor_recording() {
        let mut db = OptimizationDatabase::create(100, None).expect("create opt db");
        let sig = DataSignature::new(55, 2000, 0, 0.6, 0.15);
        db.record_anchor(&sig, 32, 0.85, 1.5, 0);
        assert_eq!(db.count(), 1);
        // record_anchor increments samples, but we need 5 for get_config
        for _ in 0..5 {
            db.record_anchor(&sig, 32, 0.85, 1.5, 0);
        }
        let cfg = db.get_config(&sig, 5).expect("should have config");
        assert_eq!(cfg.use_anchor_search, 1);
        assert_eq!(cfg.optimal_anchor_count, 32);
    }

    #[test]
    fn test_compute_query_signature() {
        let grams = vec![0x68656C, 0x656C6C, 0x6C6C6F]; // "hel", "ell", "llo"
        let sig = compute_query_signature(&grams, 5000);
        assert_eq!(sig.array_size, 5000);
        assert_ne!(sig.data_hash, 0);
        // Same input should produce same hash
        let sig2 = compute_query_signature(&grams, 5000);
        assert_eq!(sig.data_hash, sig2.data_hash);
        // Different input should produce different hash
        let sig3 = compute_query_signature(&grams, 6000);
        assert_ne!(sig.data_hash, sig3.data_hash);
    }

    // ── Phase 11: Search cache tests ──

    #[test]
    fn test_cache_create_and_store() {
        let cache = SearchCache::create().expect("create cache");
        assert_eq!(cache.count(), 0);
        let paths = vec!["/foo/bar.rs".to_string(), "/baz/qux.c".to_string()];
        let n = cache.store_results("Struct", 0, 1, &paths);
        assert_eq!(n, 2);
        assert_eq!(cache.count(), 2);
    }

    #[test]
    fn test_cache_lookup_hit() {
        let cache = SearchCache::create().expect("create cache");
        let paths = vec!["/foo/bar.rs".to_string(), "/baz/qux.c".to_string()];
        cache.store_results("Struct", 0, 1, &paths);
        let result = cache.lookup("Struct", 0, 1);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"/foo/bar.rs".to_string()));
        assert!(result.contains(&"/baz/qux.c".to_string()));
    }

    #[test]
    fn test_cache_lookup_miss_wrong_pattern() {
        let cache = SearchCache::create().expect("create cache");
        let paths = vec!["/foo/bar.rs".to_string()];
        cache.store_results("Struct", 0, 1, &paths);
        // Different pattern → miss
        assert!(cache.lookup("Return", 0, 1).is_none());
    }

    #[test]
    fn test_cache_lookup_miss_wrong_generation() {
        let cache = SearchCache::create().expect("create cache");
        let paths = vec!["/foo/bar.rs".to_string()];
        cache.store_results("Struct", 0, 1, &paths);
        // Different generation → miss (cache invalidation)
        assert!(cache.lookup("Struct", 0, 2).is_none());
    }

    #[test]
    fn test_cache_lookup_miss_wrong_flags() {
        let cache = SearchCache::create().expect("create cache");
        let paths = vec!["/foo/bar.rs".to_string()];
        cache.store_results("Struct", 0, 1, &paths);
        // Different flags → miss
        assert!(cache.lookup("Struct", CACHE_FLAG_WORD_REGEXP, 1).is_none());
    }

    #[test]
    fn test_cache_persistence_round_trip() {
        let path = format!("/tmp/tgrep_cache_test_{}.qsc", std::process::id());
        // Store
        {
            let cache = SearchCache::create().expect("create cache");
            let paths = vec![
                "/foo/bar.rs".to_string(),
                "/baz/qux.c".to_string(),
                "/quux/main.rs".to_string(),
            ];
            cache.store_results("Fn Main", 0, 42, &paths);
            assert_eq!(cache.count(), 3);
            cache.save(&path).expect("save");
        }
        // Load into a new cache
        {
            let cache = SearchCache::create().expect("create cache 2");
            assert_eq!(cache.count(), 0);
            let loaded = cache.load(&path).expect("load");
            assert_eq!(loaded, 3);
            assert_eq!(cache.count(), 3);
            let result = cache.lookup("Fn Main", 0, 42);
            assert!(result.is_some());
            let result = result.unwrap();
            assert_eq!(result.len(), 3);
            assert!(result.contains(&"/foo/bar.rs".to_string()));
            assert!(result.contains(&"/baz/qux.c".to_string()));
            assert!(result.contains(&"/quux/main.rs".to_string()));
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_cache_multiple_patterns() {
        let cache = SearchCache::create().expect("create cache");
        cache.store_results("Struct", 0, 1, &["/a.rs".to_string(), "/b.rs".to_string()]);
        cache.store_results("Return", 0, 1, &["/c.rs".to_string()]);
        assert_eq!(cache.count(), 3);
        let r1 = cache.lookup("Struct", 0, 1).unwrap();
        assert_eq!(r1.len(), 2);
        let r2 = cache.lookup("Return", 0, 1).unwrap();
        assert_eq!(r2.len(), 1);
    }

    #[test]
    fn test_cache_empty_results() {
        let cache = SearchCache::create().expect("create cache");
        // Store empty results (pattern with no matches)
        let empty: Vec<String> = vec![];
        let n = cache.store_results("Nonexistent", 0, 1, &empty);
        assert_eq!(n, 0);
        // Lookup should return None (no rows stored)
        assert!(cache.lookup("Nonexistent", 0, 1).is_none());
    }

    #[test]
    fn test_compute_cache_flags() {
        assert_eq!(compute_cache_flags(false, false, false), 0);
        assert_eq!(
            compute_cache_flags(true, false, false),
            CACHE_FLAG_CASE_INSENSITIVE
        );
        assert_eq!(
            compute_cache_flags(false, true, false),
            CACHE_FLAG_WORD_REGEXP
        );
        assert_eq!(
            compute_cache_flags(false, false, true),
            CACHE_FLAG_FIXED_STRINGS
        );
        assert_eq!(
            compute_cache_flags(true, true, true),
            CACHE_FLAG_CASE_INSENSITIVE | CACHE_FLAG_WORD_REGEXP | CACHE_FLAG_FIXED_STRINGS
        );
    }
}
