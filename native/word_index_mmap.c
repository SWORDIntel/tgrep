/**
 * word_index_mmap.c — mmap-backed flat sorted array for whole-word lookups.
 *
 * Format (.qwi v2):
 *   Header: magic(4) + version(4) + count(8) = 16 bytes
 *   Entries: key(16) + row_id(8) = 24 bytes per entry, sorted by key
 *
 * Key = FNV-1a(token) (8B big-endian) + doc_id (8B big-endian)
 *
 * Search: binary search for 8-byte hash prefix, scan forward for all
 * matching doc IDs. Only touches ~log2(count) pages for the binary
 * search plus the matching entries — not the entire file.
 *
 * Build: insert into QIHSE btree, then flush sorted entries to flat file.
 */

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>

#include "qihse_btree.h"

/* ══════════════════════════════════════════════════════════════════
 * FNV-1a hash
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
 * Flat file format
 * ══════════════════════════════════════════════════════════════════ */

#define TGREP_QWI_MAGIC   0x54575149  /* "TWQI" */
#define TGREP_QWI_VERSION  2           /* v2 = flat fixed-width */
#define TGREP_QWI_KEY_LEN  16
#define TGREP_QWI_ENTRY_SIZE  24       /* 16-byte key + 8-byte row_id */

typedef struct {
    uint32_t magic;
    uint32_t version;
    uint64_t count;
} __attribute__((packed)) qwi_header_t;

/* Mmap'd index handle */
typedef struct {
    void*       mmap_base;
    size_t      mmap_size;
    qwi_header_t* header;
    const unsigned char* entries;  /* pointer to first entry */
    size_t      count;
} qwi_mmap_t;

/* ══════════════════════════════════════════════════════════════════
 * Build path: insert into btree, flush to flat file
 * ══════════════════════════════════════════════════════════════════ */

void* tgrep_word_index_create(void) {
    return (void*)qihse_btree_create(0);
}

void tgrep_word_index_destroy(void* handle) {
    if (handle) qihse_btree_destroy((qihse_btree_t*)handle);
}

int tgrep_word_index_add(void* handle,
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

/* Save as flat sorted file via btree cursor iteration */
int tgrep_word_index_save(void* handle, const char* path) {
    if (!handle || !path) return -1;

    qihse_btree_t* tree = (qihse_btree_t*)handle;
    size_t count = qihse_btree_size(tree);

    /* Empty tree is not an error — segments with only binary files
     * (NUL bytes) have no word index entries. Skip save entirely. */
    if (count == 0) return 0;

    FILE* f = fopen(path, "wb");
    if (!f) return -1;

    /* Header */
    qwi_header_t hdr;
    hdr.magic = TGREP_QWI_MAGIC;
    hdr.version = TGREP_QWI_VERSION;
    hdr.count = count;
    fwrite(&hdr, sizeof(hdr), 1, f);

    /* Iterate all entries in sorted order via btree cursor */
    /* Use a range scan from min (NULL) to max (NULL) = full scan */
    qihse_btree_cursor_t* cur = qihse_btree_range_open(tree, NULL, 0, NULL, 0);
    if (!cur) {
        fprintf(stderr, "tgrep: qwi save diag: range_open returned NULL (count=%zu)\n", count);
        fclose(f);
        return -1;
    }

    size_t written = 0;
    const void* key;
    size_t key_len;
    uint64_t row_id;

    while (qihse_btree_cursor_get(cur, &key, &key_len, &row_id)) {
        /* Write fixed-width: 16-byte key + 8-byte row_id (big-endian) */
        if (key_len == 16) {
            fwrite(key, 16, 1, f);
            unsigned char rid_bytes[8];
            for (int i = 0; i < 8; i++)
                rid_bytes[i] = (unsigned char)(row_id >> (56 - 8 * i));
            fwrite(rid_bytes, 8, 1, f);
            written++;
        }
        if (!qihse_btree_cursor_next(cur)) break;
    }
    qihse_btree_cursor_close(cur);

    fclose(f);

    if (written != count) {
        fprintf(stderr, "tgrep: qwi save diag: count=%zu written=%zu diff=%zu\n",
                count, written, count - written);
        return -1;
    }
    return 0;
}

/* ══════════════════════════════════════════════════════════════════
 * Search path: mmap + binary search
 * ══════════════════════════════════════════════════════════════════ */

void* tgrep_word_index_load(const char* path) {
    if (!path) return NULL;

    int fd = open(path, O_RDONLY);
    if (fd < 0) return NULL;

    struct stat st;
    if (fstat(fd, &st) < 0) {
        close(fd);
        return NULL;
    }

    size_t file_size = (size_t)st.st_size;
    if (file_size < sizeof(qwi_header_t)) {
        close(fd);
        return NULL;
    }

    void* base = mmap(NULL, file_size, PROT_READ, MAP_PRIVATE, fd, 0);
    close(fd);
    if (base == MAP_FAILED) return NULL;

    qwi_mmap_t* idx = (qwi_mmap_t*)calloc(1, sizeof(qwi_mmap_t));
    if (!idx) {
        munmap(base, file_size);
        return NULL;
    }

    idx->mmap_base = base;
    idx->mmap_size = file_size;
    idx->header = (qwi_header_t*)base;

    if (idx->header->magic != TGREP_QWI_MAGIC || idx->header->version != TGREP_QWI_VERSION) {
        munmap(base, file_size);
        free(idx);
        return NULL;
    }

    idx->count = (size_t)idx->header->count;
    idx->entries = (const unsigned char*)base + sizeof(qwi_header_t);

    /* Verify file size matches — divide first so count*ENTRY_SIZE cannot wrap:
     * count entries must actually fit in the mapping beyond the header. */
    if (file_size < sizeof(qwi_header_t) ||
        (uint64_t)idx->count > (file_size - sizeof(qwi_header_t)) / TGREP_QWI_ENTRY_SIZE) {
        munmap(base, file_size);
        free(idx);
        return NULL;
    }

    /* Advise the kernel: will need this, sequential for build, random for search */
    posix_madvise(base, file_size, POSIX_MADV_RANDOM);

    return (void*)idx;
}

void tgrep_word_index_unload(void* handle) {
    if (!handle) return;
    qwi_mmap_t* idx = (qwi_mmap_t*)handle;
    if (idx->mmap_base && idx->mmap_base != MAP_FAILED) {
        munmap(idx->mmap_base, idx->mmap_size);
    }
    free(idx);
}

/* Get entry at index i */
static inline const unsigned char* entry_at(const qwi_mmap_t* idx, size_t i) {
    return idx->entries + i * TGREP_QWI_ENTRY_SIZE;
}

/* Extract 8-byte hash prefix from an entry */
static inline uint64_t entry_hash(const unsigned char* e) {
    uint64_t h = 0;
    for (int i = 0; i < 8; i++)
        h = (h << 8) | e[i];
    return h;
}

/* Extract row_id from an entry (bytes 16-23) */
static inline uint64_t entry_row_id(const unsigned char* e) {
    uint64_t r = 0;
    for (int i = 0; i < 8; i++)
        r = (r << 8) | e[16 + i];
    return r;
}

size_t tgrep_word_index_search(void* handle,
                                const char* token, size_t token_len,
                                uint64_t* out_doc_ids,
                                size_t max_results) {
    if (!handle || !token || token_len == 0 || !out_doc_ids || max_results == 0)
        return 0;

    qwi_mmap_t* idx = (qwi_mmap_t*)handle;
    if (idx->count == 0) return 0;

    uint64_t target_hash = fnv1a(token, token_len);

    /* Binary search for the first entry with matching 8-byte hash prefix */
    size_t lo = 0, hi = idx->count;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        if (entry_hash(entry_at(idx, mid)) < target_hash)
            lo = mid + 1;
        else
            hi = mid;
    }

    /* Scan forward from lo, collecting all entries with matching hash */
    size_t count = 0;
    for (size_t i = lo; i < idx->count && count < max_results; i++) {
        const unsigned char* e = entry_at(idx, i);
        uint64_t h = entry_hash(e);
        if (h != target_hash) break;  /* sorted, so we're past all matches */
        out_doc_ids[count++] = entry_row_id(e);
    }

    return count;
}

size_t tgrep_word_index_size(const void* handle) {
    if (!handle) return 0;
    const qwi_mmap_t* idx = (const qwi_mmap_t*)handle;
    return idx->count;
}
