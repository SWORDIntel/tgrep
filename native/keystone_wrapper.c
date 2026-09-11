/**
 * keystone_wrapper.c — tgrep narrow FFI wrapper around KEYSTONE trigram API.
 *
 * This file wraps the KEYSTONE C API into a simpler interface suitable for
 * Rust FFI. All functions take opaque pointers (void*) and return plain
 * integer types. String lengths are passed explicitly.
 */

#include <stdint.h>
#include <stddef.h>
#include <string.h>

#include "keystone_trigram.h"
#include "keystone.h"

/* ══════════════════════════════════════════════════════════════════
 * Index lifecycle
 * ══════════════════════════════════════════════════════════════════ */

/* Create a new trigram index. Returns opaque handle or NULL on failure.
 * initial_doc_capacity: hint for doc table pre-allocation (0 = default). */
void* tgrep_keystone_index_create(size_t initial_doc_capacity) {
    return (void*)keystone_trigram_index_create(initial_doc_capacity);
}

/* Destroy a trigram index. NULL-safe. */
void tgrep_keystone_index_destroy(void* handle) {
    keystone_trigram_index_destroy((keystone_trigram_index_t*)handle);
}

/* Finalize the index (sorts posting lists — now a no-op since they're
 * already sorted by construction, but still marks the index as ready
 * for queries). Returns 0 on success, error code on failure. */
int tgrep_keystone_index_finalize(void* handle) {
    return keystone_trigram_index_finalize((keystone_trigram_index_t*)handle);
}

/* Get document count. */
size_t tgrep_keystone_doc_count(void* handle) {
    return keystone_trigram_index_document_count((const keystone_trigram_index_t*)handle);
}

/* Get approximate memory usage in bytes. */
size_t tgrep_keystone_memory_usage(void* handle) {
    return keystone_trigram_index_memory_usage((const keystone_trigram_index_t*)handle);
}

/* ══════════════════════════════════════════════════════════════════
 * Streaming ingestion
 * ══════════════════════════════════════════════════════════════════ */

/* Begin a streaming document. Returns stream handle or NULL on failure.
 * name: optional document name (may be NULL, copied internally). */
void* tgrep_keystone_begin_document(void* handle, const char* name) {
    return (void*)keystone_trigram_begin_document(
        (keystone_trigram_index_t*)handle, name);
}

/* Feed bytes to the current streaming document. Returns 0 on success. */
int tgrep_keystone_feed_bytes(void* stream, const uint8_t* data, size_t len) {
    return keystone_trigram_feed_bytes(
        (keystone_trigram_stream_t*)stream, (const char*)data, len);
}

/* Finish the streaming document and add to index.
 * Sets *out_doc_id if non-NULL. Returns 0 on success.
 * The stream handle is consumed (freed) regardless of success/failure. */
int tgrep_keystone_end_document(void* stream, uint32_t* out_doc_id) {
    return keystone_trigram_end_document(
        (keystone_trigram_stream_t*)stream, out_doc_id);
}

/* Abort a streaming document without indexing. Frees the stream. */
void tgrep_keystone_cancel_document(void* stream) {
    keystone_trigram_cancel_document((keystone_trigram_stream_t*)stream);
}

/* ══════════════════════════════════════════════════════════════════
 * Trigram extraction and frequency
 * ══════════════════════════════════════════════════════════════════ */

/* Extract unique trigrams from a pattern.
 * Returns number of trigrams written to out_trigrams (up to max_trigrams). */
size_t tgrep_keystone_extract_trigrams(
    const char* pattern,
    size_t pattern_len,
    uint32_t* out_trigrams,
    size_t max_trigrams
) {
    return keystone_trigram_extract(
        pattern, pattern_len, out_trigrams, max_trigrams);
}

/* Get document frequency of a trigram. Returns 0 if not found. */
size_t tgrep_keystone_frequency(void* handle, uint32_t gram) {
    return keystone_trigram_index_frequency(
        (const keystone_trigram_index_t*)handle, gram);
}

/* ══════════════════════════════════════════════════════════════════
 * Candidate query (one-shot)
 * ══════════════════════════════════════════════════════════════════ */

/* Get candidates for a pattern. Writes doc IDs into out_candidates.
 * Returns number of candidates found (up to max_candidates). */
size_t tgrep_keystone_get_candidates(
    void* handle,
    const char* pattern,
    size_t pattern_len,
    uint32_t* out_candidates,
    size_t max_candidates
) {
    return keystone_trigram_index_get_candidates(
        (const keystone_trigram_index_t*)handle,
        pattern, pattern_len,
        out_candidates, max_candidates);
}

/* ══════════════════════════════════════════════════════════════════
 * Candidate query (paginated iterator)
 * ══════════════════════════════════════════════════════════════════ */

/* Create a candidate iterator. Returns iterator handle or NULL on failure. */
void* tgrep_keystone_candidates_begin(
    void* handle,
    const char* pattern,
    size_t pattern_len
) {
    return (void*)keystone_trigram_candidates_begin(
        (const keystone_trigram_index_t*)handle,
        pattern, pattern_len);
}

/* Fetch next batch of candidates.
 * Writes up to capacity doc IDs into out_buf.
 * Sets *out_count to number written, *out_exhausted to 1 when done.
 * Returns 0 on success. */
int tgrep_keystone_candidates_next(
    void* iter,
    uint32_t* out_buf,
    size_t capacity,
    size_t* out_count,
    int* out_exhausted
) {
    return keystone_trigram_candidates_next(
        (keystone_trigram_candidate_iter_t*)iter,
        out_buf, capacity, out_count, out_exhausted);
}

/* Free a candidate iterator. NULL-safe. */
void tgrep_keystone_candidates_free(void* iter) {
    keystone_trigram_candidates_free(
        (keystone_trigram_candidate_iter_t*)iter);
}

/* ══════════════════════════════════════════════════════════════════
 * Posting export visitor
 * ══════════════════════════════════════════════════════════════════ */

/* Visitor callback type: called per (gram, doc_ids, count).
 * Return non-zero to stop iteration. */
typedef int (*tgrep_posting_visitor_t)(
    uint32_t gram,
    const uint32_t* doc_ids,
    size_t count,
    void* ctx
);

/* Visit all posting lists. Returns 0 on success. */
int tgrep_keystone_visit_postings(
    void* handle,
    tgrep_posting_visitor_t visitor,
    void* ctx
) {
    return keystone_trigram_visit_postings(
        (const keystone_trigram_index_t*)handle,
        (keystone_trigram_posting_visitor_t)visitor,
        ctx);
}

/* ══════════════════════════════════════════════════════════════════
 * Posting collection (trampoline for Rust)
 * ══════════════════════════════════════════════════════════════════ */

/* Collector context for the trampoline */
typedef struct {
    uint32_t *out_grams;        /* output: gram values */
    uint32_t *out_offsets;      /* output: offset into doc_ids buffer per gram */
    uint32_t *out_doc_ids;      /* output: flat doc ID buffer */
    size_t   max_grams;         /* capacity of grams/offsets arrays */
    size_t   max_doc_ids;       /* capacity of doc_ids array */
    size_t   num_grams;         /* actual number of grams written */
    size_t   num_doc_ids;       /* actual number of doc IDs written */
    int      truncated;         /* set if capacity exceeded */
} posting_collector_t;

static int collector_visitor(
    uint32_t gram,
    const uint32_t *doc_ids,
    size_t count,
    void *ctx
) {
    posting_collector_t *c = (posting_collector_t*)ctx;
    if (c->num_grams >= c->max_grams) {
        c->truncated = 1;
        return 1; /* stop */
    }
    if (c->num_doc_ids + count > c->max_doc_ids) {
        c->truncated = 1;
        return 1; /* stop */
    }
    c->out_grams[c->num_grams] = gram;
    c->out_offsets[c->num_grams] = (uint32_t)c->num_doc_ids;
    for (size_t i = 0; i < count; i++) {
        c->out_doc_ids[c->num_doc_ids + i] = doc_ids[i];
    }
    c->num_grams++;
    c->num_doc_ids += count;
    return 0;
}

/* Collect all postings into flat buffers. Returns 0 on success.
 * Sets *out_num_grams and *out_num_doc_ids to actual counts.
 * Sets *out_truncated to 1 if capacity was exceeded. */
int tgrep_keystone_collect_postings(
    void *handle,
    uint32_t *out_grams,        /* capacity max_grams */
    uint32_t *out_offsets,      /* capacity max_grams */
    uint32_t *out_doc_ids,      /* capacity max_doc_ids */
    size_t max_grams,
    size_t max_doc_ids,
    size_t *out_num_grams,
    size_t *out_num_doc_ids,
    int *out_truncated
) {
    posting_collector_t collector = {
        out_grams, out_offsets, out_doc_ids,
        max_grams, max_doc_ids,
        0, 0, 0
    };
    int rc = keystone_trigram_visit_postings(
        (const keystone_trigram_index_t*)handle,
        (keystone_trigram_posting_visitor_t)collector_visitor,
        &collector
    );
    *out_num_grams = collector.num_grams;
    *out_num_doc_ids = collector.num_doc_ids;
    *out_truncated = collector.truncated;
    return rc;
}

/* ══════════════════════════════════════════════════════════════════
 * Stats
 * ══════════════════════════════════════════════════════════════════ */

/* Stats struct mirrored from KEYSTONE for FFI. */
typedef struct {
    uint64_t total_documents;
    uint64_t bytes_indexed;
    uint64_t unique_trigrams;
    uint64_t total_postings;
    uint64_t build_time_ns;
    uint64_t total_searches;
    uint64_t candidate_docs_evaluated;
    uint64_t candidate_docs_rejected;
} tgrep_keystone_stats_t;

/* Get stats. Returns 0 on success. */
int tgrep_keystone_get_stats(void* handle, tgrep_keystone_stats_t* out_stats) {
    if (!out_stats) return -1;
    keystone_trigram_stats_t stats;
    keystone_trigram_index_get_stats(
        (const keystone_trigram_index_t*)handle, &stats);
    out_stats->total_documents = stats.total_documents;
    out_stats->bytes_indexed = stats.bytes_indexed;
    out_stats->unique_trigrams = stats.unique_trigrams;
    out_stats->total_postings = stats.total_postings;
    out_stats->build_time_ns = stats.build_time_ns;
    out_stats->total_searches = stats.total_searches;
    out_stats->candidate_docs_evaluated = stats.candidate_docs_evaluated;
    out_stats->candidate_docs_rejected = stats.candidate_docs_rejected;
    return 0;
}

/* ══════════════════════════════════════════════════════════════════
 * Phase 9: Anchor seeding + batch search
 * ══════════════════════════════════════════════════════════════════ */

/* Pre-populate an anchor table with evenly-spaced anchors from a sorted
 * int64 array.  Returns the number of anchors inserted.
 * This warms up KEYSTONE's interpolation search so the first lookup
 * benefits from good anchor coverage without learning from misses. */
size_t tgrep_keystone_anchor_seed_batch(
    const int64_t* arr,
    size_t n,
    void* table,
    size_t anchor_count
) {
    return keystone_anchor_seed_batch(
        arr, n,
        (keystone_anchor_table_t*)table,
        anchor_count
    );
}

/* Create an anchor table. Returns opaque handle or NULL on failure. */
void* tgrep_keystone_anchor_table_create(void) {
    return (void*)keystone_anchor_table_create();
}

/* Destroy an anchor table. NULL-safe. */
void tgrep_keystone_anchor_table_destroy(void* table) {
    if (table) keystone_anchor_table_destroy((keystone_anchor_table_t*)table);
}

/* Batch search: look up multiple keys in a sorted int64 array.
 * Uses keystone_search_batch_auto with automatic backend selection.
 * Writes results into items[].result (KEYSTONE_NOT_FOUND for misses).
 * Returns the number of successful lookups. */
typedef struct {
    int64_t key;
    size_t  result;   /* KEYSTONE_NOT_FOUND or index in arr */
    size_t  ordinal;
} tgrep_batch_item_t;

typedef struct {
    int  num_threads;
    int  use_thread_pool;
    size_t batch_chunk;
} tgrep_parallel_config_t;

size_t tgrep_keystone_search_batch_auto(
    const int64_t* arr,
    size_t n,
    tgrep_batch_item_t* items,
    size_t num_items,
    void* table,
    size_t tol,
    const tgrep_parallel_config_t* config
) {
    /* Map tgrep_batch_item_t to keystone_batch_item_t (same layout) */
    keystone_parallel_config_t pcfg;
    const keystone_parallel_config_t* pcfg_ptr = NULL;
    if (config) {
        pcfg.num_threads = config->num_threads;
        pcfg.use_thread_pool = config->use_thread_pool;
        pcfg.batch_chunk = config->batch_chunk;
        pcfg_ptr = &pcfg;
    }
    return keystone_search_batch_auto(
        arr, n,
        (keystone_batch_item_t*)items,
        num_items,
        (keystone_anchor_table_t*)table,
        tol,
        pcfg_ptr
    );
}

/* Detect CPU features (bitmask). */
uint32_t tgrep_keystone_detect_cpu_features(void) {
    return keystone_detect_cpu_features();
}
