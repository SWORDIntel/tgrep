/**
 * qihse_opt_wrapper.c — tgrep FFI wrapper for QIHSE optimization DB.
 *
 * Self-contained implementation of the QIHSE optimization database surface
 * that tgrep needs (Phase 10). This avoids pulling in the full QIHSE
 * quantum-inspired search infrastructure (Grover amplification, Hilbert
 * space expansion, heterogeneous compute) which tgrep does not use.
 *
 * The optimization DB records per-data-signature performance and recommends
 * the best-known configuration (pipeline type, dimensions, anchor count,
 * thread count) for future searches with similar data signatures.
 *
 * Storage format: simple binary file with magic + version + count + entries.
 * Thread-safe via a single mutex.
 */

#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <time.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <pthread.h>

/* ── Pipeline type (matches QIHSE enum) ──────────────────────────── */
typedef enum {
    TGREP_PIPELINE_FAST = 0,
    TGREP_PIPELINE_BALANCED = 1,
    TGREP_PIPELINE_ACCURATE = 2,
    TGREP_PIPELINE_LEARNED = 3
} tgrep_pipeline_type_t;

/* ── Data signature ──────────────────────────────────────────────── */
typedef struct {
    uint64_t data_hash;       /* hash of the data (e.g. trigram key set) */
    size_t   array_size;      /* size of the array (e.g. posting list length) */
    int      data_type;       /* 0 = int64, 1 = uint64, etc. */
    double   entropy;         /* entropy of the data distribution */
    double   gap_variance;    /* variance of gaps between elements */
} tgrep_data_signature_t;

/* ── Optimization entry ───────────────────────────────────────────── */
typedef struct {
    tgrep_data_signature_t signature;
    int      best_pipeline;      /* tgrep_pipeline_type_t */
    size_t   optimal_dimensions; /* recommended dimension count */
    double   avg_speedup;        /* EMA of speedup vs baseline */
    double   avg_confidence;     /* EMA of confidence */
    size_t   samples;            /* number of recorded samples */
    uint64_t last_updated;       /* unix timestamp */
    int      use_anchor_search;  /* whether anchor search is recommended */
    size_t   optimal_anchor_count;
    double   anchor_hit_rate;
    double   anchor_speedup;
    int      workload_type;
    /* tgrep-specific extensions */
    int      optimal_threads;    /* recommended thread count */
    int      optimal_backend;    /* 0=scalar, 1=SSE4.2, 2=AVX2, 3=AVX512 */
} tgrep_optimization_entry_t;

/* ── Optimization DB ──────────────────────────────────────────────── */
typedef struct {
    tgrep_optimization_entry_t* entries;
    size_t   num_entries;
    size_t   max_entries;
    char*    storage_path;
    int      enable_learning;
    pthread_mutex_t mutex;
} tgrep_optimization_db_t;

/* ── Magic + version for storage format ──────────────────────────── */
#define TGREP_OPT_MAGIC   0x54475044u  /* "TGPD" */
#define TGREP_OPT_VERSION 1u

/* Forward declarations */
int tgrep_optimization_load(tgrep_optimization_db_t* db);

/* ══════════════════════════════════════════════════════════════════
 * Lifecycle
 * ══════════════════════════════════════════════════════════════════ */

/* Initialize an optimization DB. Returns 0 on success.
 * storage_path: path to the persistence file (may be NULL for in-memory). */
int tgrep_optimization_init(
    tgrep_optimization_db_t* db,
    size_t max_entries,
    const char* storage_path
) {
    if (!db) return -1;
    memset(db, 0, sizeof(*db));
    db->max_entries = max_entries;
    db->enable_learning = 1;
    pthread_mutex_init(&db->mutex, NULL);
    if (max_entries > 0) {
        db->entries = calloc(max_entries, sizeof(tgrep_optimization_entry_t));
        if (!db->entries) return -1;
    }
    if (storage_path) {
        db->storage_path = strdup(storage_path);
        if (db->entries) {
            tgrep_optimization_load(db);
        }
    }
    return 0;
}

/* Destroy an optimization DB. NULL-safe. */
void tgrep_optimization_destroy(tgrep_optimization_db_t* db) {
    if (!db) return;
    pthread_mutex_lock(&db->mutex);
    free(db->entries);
    free(db->storage_path);
    db->entries = NULL;
    db->storage_path = NULL;
    db->num_entries = 0;
    db->max_entries = 0;
    pthread_mutex_unlock(&db->mutex);
    pthread_mutex_destroy(&db->mutex);
    memset(db, 0, sizeof(*db));
}

/* ══════════════════════════════════════════════════════════════════
 * Entry lookup
 * ══════════════════════════════════════════════════════════════════ */

static uint64_t hash_signature(const tgrep_data_signature_t* sig) {
    uint64_t h = sig->data_hash;
    h = h * 31 + (uint64_t)sig->array_size;
    h = h * 31 + (uint64_t)sig->data_type;
    h = h * 31 + (uint64_t)(sig->entropy * 1000.0);
    h = h * 31 + (uint64_t)(sig->gap_variance * 1000.0);
    return h;
}

static tgrep_optimization_entry_t* find_entry(
    tgrep_optimization_db_t* db,
    const tgrep_data_signature_t* sig
) {
    uint64_t h = hash_signature(sig);
    for (size_t i = 0; i < db->num_entries; i++) {
        if (hash_signature(&db->entries[i].signature) == h &&
            memcmp(&db->entries[i].signature, sig, sizeof(*sig)) == 0) {
            return &db->entries[i];
        }
    }
    return NULL;
}

static tgrep_optimization_entry_t* get_or_create_entry(
    tgrep_optimization_db_t* db,
    const tgrep_data_signature_t* sig
) {
    tgrep_optimization_entry_t* e = find_entry(db, sig);
    if (e) return e;
    if (db->num_entries >= db->max_entries) {
        /* Evict oldest */
        size_t oldest = 0;
        for (size_t i = 1; i < db->num_entries; i++) {
            if (db->entries[i].last_updated < db->entries[oldest].last_updated) {
                oldest = i;
            }
        }
        e = &db->entries[oldest];
    } else {
        e = &db->entries[db->num_entries++];
    }
    memset(e, 0, sizeof(*e));
    e->signature = *sig;
    return e;
}

/* ══════════════════════════════════════════════════════════════════
 * Performance recording
 * ══════════════════════════════════════════════════════════════════ */

/* Record performance for a data signature.
 * pipeline: tgrep_pipeline_type_t
 * dimensions: recommended dimension count
 * speedup: speedup vs baseline (e.g. 2.5 = 2.5x faster)
 * confidence: 0.0–1.0 confidence in the result
 * threads: thread count used
 * backend: 0=scalar, 1=SSE4.2, 2=AVX2, 3=AVX512 */
void tgrep_optimization_record(
    tgrep_optimization_db_t* db,
    const tgrep_data_signature_t* sig,
    int pipeline,
    size_t dimensions,
    double speedup,
    double confidence,
    int threads,
    int backend
) {
    if (!db || !db->enable_learning || !sig) return;
    pthread_mutex_lock(&db->mutex);
    tgrep_optimization_entry_t* e = get_or_create_entry(db, sig);
    if (!e) { pthread_mutex_unlock(&db->mutex); return; }
    double alpha = 0.1;  /* EMA factor */
    e->avg_speedup = e->avg_speedup * (1.0 - alpha) + speedup * alpha;
    e->avg_confidence = e->avg_confidence * (1.0 - alpha) + confidence * alpha;
    e->samples++;
    if (speedup * confidence > e->avg_speedup * e->avg_confidence || e->samples == 1) {
        e->best_pipeline = pipeline;
        e->optimal_dimensions = dimensions;
        e->optimal_threads = threads;
        e->optimal_backend = backend;
    }
    e->last_updated = (uint64_t)time(NULL);
    pthread_mutex_unlock(&db->mutex);
}

/* ══════════════════════════════════════════════════════════════════
 * Config retrieval
 * ══════════════════════════════════════════════════════════════════ */

/* Result struct for optimized config lookup */
typedef struct {
    int      found;             /* 1 if a matching entry with enough samples was found */
    int      best_pipeline;
    size_t   optimal_dimensions;
    double   avg_speedup;
    double   avg_confidence;
    size_t   samples;
    int      use_anchor_search;
    size_t   optimal_anchor_count;
    int      optimal_threads;
    int      optimal_backend;
} tgrep_optimized_config_t;

/* Get the optimized config for a data signature.
 * Requires at least min_samples (default 5) recorded samples.
 * Returns 1 if found, 0 if no recommendation available. */
int tgrep_optimization_get_config(
    tgrep_optimization_db_t* db,
    const tgrep_data_signature_t* sig,
    size_t min_samples,
    tgrep_optimized_config_t* out
) {
    if (!db || !sig || !out) return 0;
    memset(out, 0, sizeof(*out));
    pthread_mutex_lock(&db->mutex);
    tgrep_optimization_entry_t* e = find_entry(db, sig);
    int found = 0;
    if (e) {
        if (e->samples >= min_samples) {
            out->found = 1;
            out->best_pipeline = e->best_pipeline;
            out->optimal_dimensions = e->optimal_dimensions;
            out->avg_speedup = e->avg_speedup;
            out->avg_confidence = e->avg_confidence;
            out->samples = e->samples;
            out->use_anchor_search = e->use_anchor_search;
            out->optimal_anchor_count = e->optimal_anchor_count;
            out->optimal_threads = e->optimal_threads;
            out->optimal_backend = e->optimal_backend;
            found = 1;
        }
    }
    pthread_mutex_unlock(&db->mutex);
    return found;
}

/* ══════════════════════════════════════════════════════════════════
 * Anchor performance recording
 * ══════════════════════════════════════════════════════════════════ */

void tgrep_optimization_record_anchor(
    tgrep_optimization_db_t* db,
    const tgrep_data_signature_t* sig,
    size_t anchor_count,
    double hit_rate,
    double speedup,
    int workload_type
) {
    if (!db || !db->enable_learning || !sig) return;
    pthread_mutex_lock(&db->mutex);
    tgrep_optimization_entry_t* e = get_or_create_entry(db, sig);
    if (!e) { pthread_mutex_unlock(&db->mutex); return; }
    e->use_anchor_search = 1;
    e->optimal_anchor_count = anchor_count;
    e->anchor_hit_rate = hit_rate;
    e->anchor_speedup = speedup;
    e->workload_type = workload_type;
    e->last_updated = (uint64_t)time(NULL);
    e->samples++;
    pthread_mutex_unlock(&db->mutex);
}

/* ══════════════════════════════════════════════════════════════════
 * Persistence
 * ══════════════════════════════════════════════════════════════════ */

/* Save the optimization DB to disk. Returns 0 on success. */
int tgrep_optimization_save(tgrep_optimization_db_t* db) {
    if (!db || !db->storage_path) return -1;
    pthread_mutex_lock(&db->mutex);
    int fd = open(db->storage_path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (fd < 0) { pthread_mutex_unlock(&db->mutex); return -errno; }
    FILE* fp = fdopen(fd, "wb");
    if (!fp) { close(fd); pthread_mutex_unlock(&db->mutex); return -errno; }
    uint32_t magic = TGREP_OPT_MAGIC, version = TGREP_OPT_VERSION;
    fwrite(&magic, 4, 1, fp);
    fwrite(&version, 4, 1, fp);
    fwrite(&db->num_entries, sizeof(size_t), 1, fp);
    fwrite(db->entries, sizeof(tgrep_optimization_entry_t), db->num_entries, fp);
    fclose(fp);
    pthread_mutex_unlock(&db->mutex);
    return 0;
}

/* Load the optimization DB from disk. Returns 0 on success. */
int tgrep_optimization_load(tgrep_optimization_db_t* db) {
    if (!db || !db->storage_path) return -1;
    pthread_mutex_lock(&db->mutex);
    int fd = open(db->storage_path, O_RDONLY);
    if (fd < 0) { pthread_mutex_unlock(&db->mutex); return -errno; }
    FILE* fp = fdopen(fd, "rb");
    if (!fp) { close(fd); pthread_mutex_unlock(&db->mutex); return -errno; }
    uint32_t magic, version;
    size_t n;
    if (fread(&magic, 4, 1, fp) != 1 || fread(&version, 4, 1, fp) != 1 ||
        magic != TGREP_OPT_MAGIC || version != TGREP_OPT_VERSION) {
        fclose(fp); pthread_mutex_unlock(&db->mutex); return -1;
    }
    if (fread(&n, sizeof(size_t), 1, fp) != 1) {
        fclose(fp); pthread_mutex_unlock(&db->mutex); return -1;
    }
    db->num_entries = (n > db->max_entries) ? db->max_entries : n;
    size_t read_count = fread(db->entries, sizeof(tgrep_optimization_entry_t), db->num_entries, fp);
    (void)read_count; /* partial read is acceptable */
    fclose(fp);
    pthread_mutex_unlock(&db->mutex);
    return 0;
}

/* Get the number of entries in the DB. */
size_t tgrep_optimization_count(const tgrep_optimization_db_t* db) {
    if (!db) return 0;
    return db->num_entries;
}

/* Enable or disable learning. */
void tgrep_optimization_set_learning(tgrep_optimization_db_t* db, int enable) {
    if (!db) return;
    pthread_mutex_lock(&db->mutex);
    db->enable_learning = enable;
    pthread_mutex_unlock(&db->mutex);
}
