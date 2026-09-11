/*
 * tgrep search-result cache wrapper around QIHSE table store.
 *
 * Caches the file list matching a (pattern, flags, generation) tuple so
 * repeated identical searches are O(1) instead of re-running the trigram
 * intersection + verification pipeline.
 *
 * Schema:
 *   search_cache (
 *     pattern      STRING   — search pattern
 *     flags        INT32    — bitfield: case_insensitive|word_regexp|fixed
 *     generation   INT64    — manifest generation when cached
 *     file_path    STRING   — matching file path (one row per match)
 *     cached_at    INT64    — unix timestamp of cache entry
 *   )
 *
 * Persistence: binary dump to .qsc file (magic "TGSC", version 1).
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <pthread.h>

#include "qihse_table_store.h"

/* ── Cache flags (must match Rust side) ─────────────────────────────── */
#define TGREP_CACHE_FLAG_CASE_INSENSITIVE 0x01
#define TGREP_CACHE_FLAG_WORD_REGEXP       0x02
#define TGREP_CACHE_FLAG_FIXED_STRINGS     0x04

/* ── Persistence format ─────────────────────────────────────────────── */
#define TGSC_MAGIC   0x53434754u  /* "TGSC" little-endian */
#define TGSC_VERSION 1u

/* ── Callback context for scanning ──────────────────────────────────── */
typedef struct {
    const char* target_pattern;
    int32_t     target_flags;
    int64_t     target_generation;
    /* Output buffer */
    char**      out_paths;
    size_t      out_count;
    size_t      out_cap;
} cache_scan_ctx_t;

/* ── Wrapper handle ─────────────────────────────────────────────────── */
typedef struct tgrep_cache {
    qihse_table_store_t* store;
    qihse_table_t*      table;
    pthread_mutex_t     lock;
} tgrep_cache_t;

/* ── Forward declarations ──────────────────────────────────────────── */
static int cache_save(tgrep_cache_t* cache, const char* path);
static int cache_load(tgrep_cache_t* cache, const char* path);

/* ── Create / destroy ──────────────────────────────────────────────── */

tgrep_cache_t* tgrep_cache_create(void) {
    tgrep_cache_t* cache = calloc(1, sizeof(tgrep_cache_t));
    if (!cache) return NULL;

    cache->store = qihse_table_store_create();
    if (!cache->store) {
        free(cache);
        return NULL;
    }

    /* Define schema: pattern, flags, generation, file_path, cached_at */
    qihse_col_def_t cols[5];
    cols[0].name = strdup("pattern");
    cols[0].type = QIHSE_TS_STRING;
    cols[1].name = strdup("flags");
    cols[1].type = QIHSE_TS_INT32;
    cols[2].name = strdup("generation");
    cols[2].type = QIHSE_TS_INT64;
    cols[3].name = strdup("file_path");
    cols[3].type = QIHSE_TS_STRING;
    cols[4].name = strdup("cached_at");
    cols[4].type = QIHSE_TS_INT64;

    cache->table = qihse_table_store_create_table(cache->store, "search_cache", cols, 5);
    for (int i = 0; i < 5; i++) free(cols[i].name);

    if (!cache->table) {
        qihse_table_store_destroy(cache->store);
        free(cache);
        return NULL;
    }

    pthread_mutex_init(&cache->lock, NULL);
    return cache;
}

void tgrep_cache_destroy(tgrep_cache_t* cache) {
    if (!cache) return;
    if (cache->store) qihse_table_store_destroy(cache->store);
    pthread_mutex_destroy(&cache->lock);
    free(cache);
}

/* ── Insert matching files for a query ─────────────────────────────── */

int tgrep_cache_store_results(
    tgrep_cache_t* cache,
    const char* pattern,
    int32_t flags,
    int64_t generation,
    const char* const* file_paths,
    size_t num_paths
) {
    if (!cache || !pattern || !file_paths) return -1;
    pthread_mutex_lock(&cache->lock);

    int64_t now = (int64_t)time(NULL);
    int inserted = 0;

    for (size_t i = 0; i < num_paths; i++) {
        qihse_col_value_t row[5];
        row[0].type = QIHSE_TS_STRING;
        row[0].v.str = (char*)pattern;
        row[1].type = QIHSE_TS_INT32;
        row[1].v.i32 = flags;
        row[2].type = QIHSE_TS_INT64;
        row[2].v.i64 = generation;
        row[3].type = QIHSE_TS_STRING;
        row[3].v.str = (char*)file_paths[i];
        row[4].type = QIHSE_TS_INT64;
        row[4].v.i64 = now;

        if (qihse_table_insert(cache->table, row, 5) >= 0) {
            inserted++;
        }
    }

    pthread_mutex_unlock(&cache->lock);
    return inserted;
}

/* ── Scan callback: collect matching file paths ─────────────────────── */

static bool cache_scan_cb(const qihse_col_value_t* values, size_t num_cols, void* ctx) {
    cache_scan_ctx_t* c = (cache_scan_ctx_t*)ctx;
    if (num_cols < 5) return true;

    /* Check pattern (col 0) */
    if (values[0].type != QIHSE_TS_STRING || !values[0].v.str) return true;
    if (strcmp(values[0].v.str, c->target_pattern) != 0) return true;

    /* Check flags (col 1) */
    if (values[1].type != QIHSE_TS_INT32) return true;
    if (values[1].v.i32 != c->target_flags) return true;

    /* Check generation (col 2) */
    if (values[2].type != QIHSE_TS_INT64) return true;
    if (values[2].v.i64 != c->target_generation) return true;

    /* Collect file_path (col 3) */
    if (values[3].type != QIHSE_TS_STRING || !values[3].v.str) return true;

    if (c->out_count >= c->out_cap) {
        size_t new_cap = c->out_cap == 0 ? 64 : c->out_cap * 2;
        char** new_paths = realloc(c->out_paths, new_cap * sizeof(char*));
        if (!new_paths) return false;  /* stop scan on OOM */
        c->out_paths = new_paths;
        c->out_cap = new_cap;
    }
    c->out_paths[c->out_count] = strdup(values[3].v.str);
    if (!c->out_paths[c->out_count]) return false;
    c->out_count++;

    return true;  /* continue scan */
}

/* ── Lookup cached results for a query ─────────────────────────────── */

int tgrep_cache_lookup(
    tgrep_cache_t* cache,
    const char* pattern,
    int32_t flags,
    int64_t generation,
    char*** out_paths,
    size_t* out_count
) {
    if (!cache || !pattern || !out_paths || !out_count) return -1;
    *out_paths = NULL;
    *out_count = 0;

    pthread_mutex_lock(&cache->lock);

    cache_scan_ctx_t ctx = {
        .target_pattern = pattern,
        .target_flags = flags,
        .target_generation = generation,
        .out_paths = NULL,
        .out_count = 0,
        .out_cap = 0,
    };

    qihse_table_scan(cache->table, cache_scan_cb, &ctx);

    pthread_mutex_unlock(&cache->lock);

    *out_paths = ctx.out_paths;
    *out_count = ctx.out_count;
    return (int)ctx.out_count;
}

/* ── Free paths returned by tgrep_cache_lookup ──────────────────────── */

void tgrep_cache_free_paths(char** paths, size_t count) {
    if (!paths) return;
    for (size_t i = 0; i < count; i++) {
        free(paths[i]);
    }
    free(paths);
}

/* ── Invalidate entries for a pattern+flags (any generation) ───────── */

int tgrep_cache_invalidate(
    tgrep_cache_t* cache,
    const char* pattern,
    int32_t flags
) {
    if (!cache || !pattern) return -1;
    /* Generation-based invalidation makes explicit deletion unnecessary.
     * Old entries are ignored on lookup because generation won't match. */
    (void)flags;
    return 0;
}

/* ── Row count ─────────────────────────────────────────────────────── */

size_t tgrep_cache_count(tgrep_cache_t* cache) {
    if (!cache || !cache->table) return 0;
    return qihse_table_row_count(cache->table);
}

/* ── Cache stats: enumerate unique patterns ────────────────────────── */

typedef struct {
    char**  patterns;
    int32_t* flags_arr;
    int64_t* generations;
    size_t*  counts;
    size_t  unique_count;
    size_t  cap;
} pattern_enumerate_ctx_t;

static bool cache_enum_cb(const qihse_col_value_t* values, size_t num_cols, void* ctx) {
    pattern_enumerate_ctx_t* c = (pattern_enumerate_ctx_t*)ctx;
    if (num_cols < 5) return true;

    if (values[0].type != QIHSE_TS_STRING || !values[0].v.str) return true;
    if (values[1].type != QIHSE_TS_INT32) return true;
    if (values[2].type != QIHSE_TS_INT64) return true;

    const char* pat = values[0].v.str;
    int32_t fl = values[1].v.i32;
    int64_t gen = values[2].v.i64;

    /* Check if this pattern+flags+gen is already in our list */
    for (size_t i = 0; i < c->unique_count; i++) {
        if (strcmp(c->patterns[i], pat) == 0
            && c->flags_arr[i] == fl
            && c->generations[i] == gen) {
            c->counts[i]++;
            return true;
        }
    }

    /* Add new unique entry */
    if (c->unique_count >= c->cap) {
        size_t new_cap = c->cap == 0 ? 64 : c->cap * 2;
        char** np = realloc(c->patterns, new_cap * sizeof(char*));
        if (!np) return false;
        c->patterns = np;
        int32_t* nf = realloc(c->flags_arr, new_cap * sizeof(int32_t));
        if (!nf) return false;
        c->flags_arr = nf;
        int64_t* ng = realloc(c->generations, new_cap * sizeof(int64_t));
        if (!ng) return false;
        c->generations = ng;
        size_t* nc = realloc(c->counts, new_cap * sizeof(size_t));
        if (!nc) return false;
        c->counts = nc;
        c->cap = new_cap;
    }

    c->patterns[c->unique_count] = strdup(pat);
    if (!c->patterns[c->unique_count]) return false;
    c->flags_arr[c->unique_count] = fl;
    c->generations[c->unique_count] = gen;
    c->counts[c->unique_count] = 1;
    c->unique_count++;
    return true;
}

typedef struct {
    char* pattern;
    int32_t flags;
    int64_t generation;
    size_t file_count;
} tgrep_cache_entry_info_t;

int tgrep_cache_get_entries(
    tgrep_cache_t* cache,
    tgrep_cache_entry_info_t* out_entries,
    size_t max_entries,
    size_t* out_count
) {
    if (!cache || !out_count) return -1;
    *out_count = 0;
    if (!out_entries || max_entries == 0) return -1;

    pthread_mutex_lock(&cache->lock);

    pattern_enumerate_ctx_t ctx = {
        .patterns = NULL, .flags_arr = NULL, .generations = NULL,
        .counts = NULL, .unique_count = 0, .cap = 0,
    };
    qihse_table_scan(cache->table, cache_enum_cb, &ctx);

    pthread_mutex_unlock(&cache->lock);

    size_t n = ctx.unique_count < max_entries ? ctx.unique_count : max_entries;
    for (size_t i = 0; i < n; i++) {
        out_entries[i].pattern = ctx.patterns[i];
        out_entries[i].flags = ctx.flags_arr[i];
        out_entries[i].generation = ctx.generations[i];
        out_entries[i].file_count = ctx.counts[i];
    }

    /* Free strings that we didn't transfer to out_entries */
    for (size_t i = n; i < ctx.unique_count; i++) {
        free(ctx.patterns[i]);
    }
    free(ctx.patterns);
    free(ctx.flags_arr);
    free(ctx.generations);
    free(ctx.counts);

    *out_count = n;
    return (int)n;
}

void tgrep_cache_free_entries(tgrep_cache_entry_info_t* entries, size_t count) {
    if (!entries) return;
    for (size_t i = 0; i < count; i++) {
        free(entries[i].pattern);
        entries[i].pattern = NULL;
    }
}

/* ── Persistence ───────────────────────────────────────────────────── */

/* Save callback context */
typedef struct {
    FILE* f;
    size_t written;
} save_ctx_t;

static bool cache_save_cb(const qihse_col_value_t* values, size_t num_cols, void* ctx) {
    save_ctx_t* s = (save_ctx_t*)ctx;
    if (num_cols < 5) return true;

    /* pattern */
    const char* pat = (values[0].type == QIHSE_TS_STRING) ? values[0].v.str : "";
    uint32_t pat_len = (uint32_t)strlen(pat);
    fwrite(&pat_len, 4, 1, s->f);
    fwrite(pat, 1, pat_len, s->f);

    /* flags */
    int32_t flags = (values[1].type == QIHSE_TS_INT32) ? values[1].v.i32 : 0;
    fwrite(&flags, 4, 1, s->f);

    /* generation */
    int64_t gen = (values[2].type == QIHSE_TS_INT64) ? values[2].v.i64 : 0;
    fwrite(&gen, 8, 1, s->f);

    /* file_path */
    const char* fp = (values[3].type == QIHSE_TS_STRING) ? values[3].v.str : "";
    uint32_t fp_len = (uint32_t)strlen(fp);
    fwrite(&fp_len, 4, 1, s->f);
    fwrite(fp, 1, fp_len, s->f);

    /* cached_at */
    int64_t ts = (values[4].type == QIHSE_TS_INT64) ? values[4].v.i64 : 0;
    fwrite(&ts, 8, 1, s->f);

    s->written++;
    return true;
}

/* Save: write all rows to a binary file.
 * Format: magic(4) + version(4) + row_count(8) + rows...
 * Each row: pattern_len(4) + pattern + flags(4) + gen(8) +
 *          path_len(4) + path + cached_at(8) */

static int cache_save(tgrep_cache_t* cache, const char* path) {
    if (!cache || !path) return -1;

    FILE* f = fopen(path, "wb");
    if (!f) return -1;

    pthread_mutex_lock(&cache->lock);

    uint32_t magic = TGSC_MAGIC;
    uint32_t version = TGSC_VERSION;
    size_t row_count = qihse_table_row_count(cache->table);

    fwrite(&magic, 4, 1, f);
    fwrite(&version, 4, 1, f);
    fwrite(&row_count, sizeof(size_t), 1, f);

    save_ctx_t sctx = { .f = f, .written = 0 };
    qihse_table_scan(cache->table, cache_save_cb, &sctx);

    pthread_mutex_unlock(&cache->lock);
    fclose(f);
    return (int)sctx.written;
}

/* Load: read rows from binary file and insert into table. */
static int cache_load(tgrep_cache_t* cache, const char* path) {
    if (!cache || !path) return -1;

    FILE* f = fopen(path, "rb");
    if (!f) return -1;

    uint32_t magic, version;
    if (fread(&magic, 4, 1, f) != 1 || magic != TGSC_MAGIC) {
        fclose(f);
        return -1;
    }
    if (fread(&version, 4, 1, f) != 1 || version != TGSC_VERSION) {
        fclose(f);
        return -1;
    }

    size_t row_count;
    if (fread(&row_count, sizeof(size_t), 1, f) != 1) {
        fclose(f);
        return -1;
    }

    pthread_mutex_lock(&cache->lock);
    int loaded = 0;

    for (size_t i = 0; i < row_count; i++) {
        /* pattern */
        uint32_t pat_len;
        if (fread(&pat_len, 4, 1, f) != 1) break;
        char* pattern = malloc(pat_len + 1);
        if (!pattern) break;
        if (fread(pattern, 1, pat_len, f) != pat_len) { free(pattern); break; }
        pattern[pat_len] = '\0';

        /* flags */
        int32_t flags;
        if (fread(&flags, 4, 1, f) != 1) { free(pattern); break; }

        /* generation */
        int64_t gen;
        if (fread(&gen, 8, 1, f) != 1) { free(pattern); break; }

        /* file_path */
        uint32_t fp_len;
        if (fread(&fp_len, 4, 1, f) != 1) { free(pattern); break; }
        char* file_path = malloc(fp_len + 1);
        if (!file_path) { free(pattern); break; }
        if (fread(file_path, 1, fp_len, f) != fp_len) { free(pattern); free(file_path); break; }
        file_path[fp_len] = '\0';

        /* cached_at */
        int64_t ts;
        if (fread(&ts, 8, 1, f) != 1) { free(pattern); free(file_path); break; }

        /* Insert into table */
        qihse_col_value_t row[5];
        row[0].type = QIHSE_TS_STRING;
        row[0].v.str = pattern;
        row[1].type = QIHSE_TS_INT32;
        row[1].v.i32 = flags;
        row[2].type = QIHSE_TS_INT64;
        row[2].v.i64 = gen;
        row[3].type = QIHSE_TS_STRING;
        row[3].v.str = file_path;
        row[4].type = QIHSE_TS_INT64;
        row[4].v.i64 = ts;

        if (qihse_table_insert(cache->table, row, 5) >= 0) {
            loaded++;
        }

        free(pattern);
        free(file_path);
    }

    pthread_mutex_unlock(&cache->lock);
    fclose(f);
    return loaded;
}

/* ── Public save/load wrappers ─────────────────────────────────────── */

int tgrep_cache_save(tgrep_cache_t* cache, const char* path) {
    return cache_save(cache, path);
}

int tgrep_cache_load_file(tgrep_cache_t* cache, const char* path) {
    return cache_load(cache, path);
}
