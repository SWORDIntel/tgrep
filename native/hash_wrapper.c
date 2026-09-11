/**
 * hash_wrapper.c — tgrep FFI wrapper around KEYSTONE dsmil_hash_index API.
 *
 * Wraps the hash index for O(1) exact whole-token lookups.
 * All functions take opaque pointers (void*) and return plain integer types.
 */

#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

#include "dsmil_hash_indexer.h"

/* ══════════════════════════════════════════════════════════════════
 * Hash index lifecycle
 * ══════════════════════════════════════════════════════════════════ */

/* Create a new hash index. Returns opaque handle or NULL on failure. */
void* tgrep_hash_index_create(size_t initial_capacity) {
    return (void*)dsmil_hash_index_create(initial_capacity);
}

/* Destroy a hash index. NULL-safe. */
void tgrep_hash_index_destroy(void* handle) {
    dsmil_hash_index_destroy((dsmil_hash_index_t*)handle);
}

/* Add a token with its doc ID as the "byte offset". Returns 0 on success. */
int tgrep_hash_index_add(void* handle, const char* str, size_t len, uint64_t doc_id) {
    return dsmil_hash_index_add((dsmil_hash_index_t*)handle, str, len, doc_id);
}

/* Finalize (sort) the index for searching. Returns 0 on success. */
int tgrep_hash_index_finalize(void* handle) {
    return dsmil_hash_index_finalize((dsmil_hash_index_t*)handle);
}

/* Get entry count. */
size_t tgrep_hash_index_count(void* handle) {
    dsmil_hash_index_t* idx = (dsmil_hash_index_t*)handle;
    return idx ? idx->count : 0;
}

/* ══════════════════════════════════════════════════════════════════
 * Search
 * ══════════════════════════════════════════════════════════════════ */

/* Search for all doc IDs matching a token.
 * Returns the number of matches found (0 = not found).
 * Results are written to out_doc_ids (up to max_results). */
size_t tgrep_hash_index_search_all(
    void* handle,
    const char* query_str,
    size_t query_len,
    uint64_t* out_doc_ids,
    size_t max_results
) {
    if (!handle || !query_str) return 0;

    /* Make a NUL-terminated copy for the C API */
    char buf[4096];
    char* tmp = NULL;
    const char* query;

    if (query_len < sizeof(buf)) {
        memcpy(buf, query_str, query_len);
        buf[query_len] = '\0';
        query = buf;
    } else {
        tmp = (char*)malloc(query_len + 1);
        if (!tmp) return 0;
        memcpy(tmp, query_str, query_len);
        tmp[query_len] = '\0';
        query = tmp;
    }

    size_t count = 0;
    dsmil_hash_index_search_all(
        (dsmil_hash_index_t*)handle, query,
        out_doc_ids, max_results, &count
    );

    if (tmp) free(tmp);
    return count;
}

/* ══════════════════════════════════════════════════════════════════
 * Serialization
 * ══════════════════════════════════════════════════════════════════ */

/* Save hash index to a file. Returns 0 on success. */
int tgrep_hash_index_save(void* handle, const char* path) {
    return dsmil_hash_index_save((dsmil_hash_index_t*)handle, path);
}

/* Load hash index from a file. Returns opaque handle or NULL on failure. */
void* tgrep_hash_index_load(const char* path) {
    return (void*)dsmil_hash_index_load(path);
}
