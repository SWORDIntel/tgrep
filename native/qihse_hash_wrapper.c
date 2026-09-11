/**
 * qihse_hash_wrapper.c — tgrep FFI wrapper around QIHSE btree for
 * persistent whole-token → doc-ID lookups.
 *
 * Uses qihse_btree with fixed-width 16-byte keys:
 *   key = FNV-1a(token) (8 bytes, big-endian) + doc_id (8 bytes, big-endian)
 *
 * Fixed-width keys avoid the btree's variable-length page overflow issue.
 * Prefix scan on the 8-byte hash returns all doc IDs for that token.
 *
 * Hash collisions are acceptable: the search results are candidates that
 * are verified by actual file content scanning, so a false positive from
 * a hash collision just means an extra file is scanned (rare, 2^-64).
 *
 * Persistence via qihse_btree_save / qihse_btree_load.
 */

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

#include "qihse_btree.h"

/* ══════════════════════════════════════════════════════════════════
 * FNV-1a hash (matches KEYSTONE's dsmil_hash_indexer)
 * ══════════════════════════════════════════════════════════════════ */

static uint64_t fnv1a(const void* data, size_t len) {
    const unsigned char* p = (const unsigned char*)data;
    uint64_t h = 1469598103934665603ULL;
    for (size_t i = 0; i < len; i++) {
        h ^= p[i];
        h *= 1099511628211ULL;
    }
    return h;
}

/* ══════════════════════════════════════════════════════════════════
 * Lifecycle
 * ══════════════════════════════════════════════════════════════════ */

void* tgrep_qihse_word_index_create(void) {
    return (void*)qihse_btree_create(0);
}

void tgrep_qihse_word_index_destroy(void* handle) {
    if (handle) qihse_btree_destroy((qihse_btree_t*)handle);
}

/* ══════════════════════════════════════════════════════════════════
 * Insert: key = hash(token) (8B big-endian) + doc_id (8B big-endian)
 * ══════════════════════════════════════════════════════════════════ */

int tgrep_qihse_word_index_add(void* handle,
                                const char* token, size_t token_len,
                                uint64_t doc_id) {
    if (!handle || !token || token_len == 0) return -1;

    uint64_t h = fnv1a(token, token_len);

    unsigned char key[16];
    for (int i = 0; i < 8; i++)
        key[i] = (unsigned char)(h >> (56 - 8 * i));
    for (int i = 0; i < 8; i++)
        key[8 + i] = (unsigned char)(doc_id >> (56 - 8 * i));

    return qihse_btree_insert((qihse_btree_t*)handle, key, 16, doc_id) ? 0 : -1;
}

/* ══════════════════════════════════════════════════════════════════
 * Search: prefix scan on 8-byte hash to get all matching doc IDs
 * ══════════════════════════════════════════════════════════════════ */

size_t tgrep_qihse_word_index_search(void* handle,
                                      const char* token, size_t token_len,
                                      uint64_t* out_doc_ids,
                                      size_t max_results) {
    if (!handle || !token || token_len == 0 || !out_doc_ids || max_results == 0)
        return 0;

    uint64_t h = fnv1a(token, token_len);

    /* Prefix = 8-byte big-endian hash */
    unsigned char prefix[8];
    for (int i = 0; i < 8; i++)
        prefix[i] = (unsigned char)(h >> (56 - 8 * i));

    qihse_btree_cursor_t* cur = qihse_btree_prefix_open(
        (qihse_btree_t*)handle, prefix, 8);
    if (!cur) return 0;

    size_t count = 0;
    const void* key;
    size_t key_len;
    uint64_t row_id;

    while (count < max_results && qihse_btree_cursor_get(cur, &key, &key_len, &row_id)) {
        out_doc_ids[count++] = row_id;
        if (!qihse_btree_cursor_next(cur)) break;
    }

    qihse_btree_cursor_close(cur);
    return count;
}

/* ══════════════════════════════════════════════════════════════════
 * Persistence
 * ══════════════════════════════════════════════════════════════════ */

int tgrep_qihse_word_index_save(const void* handle, const char* path) {
    return qihse_btree_save((const qihse_btree_t*)handle, path);
}

void* tgrep_qihse_word_index_load(const char* path) {
    return (void*)qihse_btree_load(path);
}

/* Entry count */
size_t tgrep_qihse_word_index_size(const void* handle) {
    return qihse_btree_size((const qihse_btree_t*)handle);
}
