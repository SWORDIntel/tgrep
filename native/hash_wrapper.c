/**
 * hash_wrapper.c — tgrep FFI wrapper around KEYSTONE dsmil_hash_index API.
 *
 * Wraps the hash index for O(1) exact whole-token lookups.
 * All functions take opaque pointers (void*) and return plain integer types.
 */

#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "dsmil_hash_indexer.h"

/* ══════════════════════════════════════════════════════════════════
 * Hash index lifecycle
 * ══════════════════════════════════════════════════════════════════ */

/* Create a new hash index. Returns opaque handle or NULL on failure. */
void *tgrep_hash_index_create(size_t initial_capacity)
{
    return (void *)dsmil_hash_index_create(initial_capacity);
}

/* Destroy a hash index. NULL-safe. */
void tgrep_hash_index_destroy(void *handle)
{
    dsmil_hash_index_destroy((dsmil_hash_index_t *)handle);
}

/* Add a token with its doc ID as the "byte offset". Returns 0 on success. */
int tgrep_hash_index_add(void *handle, const char *str, size_t len, uint64_t doc_id)
{
    return dsmil_hash_index_add((dsmil_hash_index_t *)handle, str, len, doc_id);
}

/* Finalize (sort) the index for searching. Returns 0 on success. */
int tgrep_hash_index_finalize(void *handle)
{
    return dsmil_hash_index_finalize((dsmil_hash_index_t *)handle);
}

/* Get entry count. */
size_t tgrep_hash_index_count(void *handle)
{
    dsmil_hash_index_t *idx = (dsmil_hash_index_t *)handle;
    return idx ? idx->count : 0;
}

/* ══════════════════════════════════════════════════════════════════
 * Search
 * ══════════════════════════════════════════════════════════════════ */

/* Search for all doc IDs matching a token. Return the total matches (0 = not found), which may
 * exceed max_results, or SIZE_MAX on an invalid request/allocation failure. Only the first
 * max_results IDs are written to out_doc_ids. */
size_t tgrep_hash_index_search_all(void *handle, const char *query_str, size_t query_len,
                                   uint64_t *out_doc_ids, size_t max_results)
{
    if (!handle || !query_str || query_len == SIZE_MAX || (max_results > 0 && !out_doc_ids))
        return SIZE_MAX;

    /* The lower search API accepts C strings. Preserve counted binary tokens without
     * changing the normal NUL-free hash lookup path. */
    if (memchr(query_str, '\0', query_len)) {
        dsmil_hash_index_t *idx = (dsmil_hash_index_t *)handle;
        if (!idx->is_sorted)
            return 0;
        size_t count = 0;
        for (size_t i = 0; i < idx->count; i++) {
            if (idx->string_lens[i] == query_len &&
                memcmp(idx->strings[i], query_str, query_len) == 0) {
                if (count < max_results)
                    out_doc_ids[count] = idx->offsets[i];
                count++;
            }
        }
        return count;
    }

    /* Make a NUL-terminated copy for the C API */
    char buf[4096];
    char *tmp = NULL;
    const char *query;

    if (query_len < sizeof(buf)) {
        memcpy(buf, query_str, query_len);
        buf[query_len] = '\0';
        query = buf;
    } else {
        tmp = (char *)malloc(query_len + 1);
        if (!tmp)
            return SIZE_MAX;
        memcpy(tmp, query_str, query_len);
        tmp[query_len] = '\0';
        query = tmp;
    }

    size_t count = 0;
    dsmil_hash_index_search_all((dsmil_hash_index_t *)handle, query, out_doc_ids, max_results,
                                &count);

    if (tmp)
        free(tmp);
    return count;
}

/* ══════════════════════════════════════════════════════════════════
 * Serialization
 * ══════════════════════════════════════════════════════════════════ */

/* Save hash index to a file. Returns 0 on success. */
int tgrep_hash_index_save(void *handle, const char *path)
{
    return dsmil_hash_index_save((dsmil_hash_index_t *)handle, path);
}

/* Load hash index from a file. Returns opaque handle or NULL on failure. */
void *tgrep_hash_index_load(const char *path)
{
    return (void *)dsmil_hash_index_load(path);
}
