/*
 * qihse_wal_wrapper.c — thin C wrappers around QIHSE WAL for tgrep.
 *
 * Provides void* handles and simple functions for:
 *   - Creating/destroying a WAL
 *   - Logging segment publications as transactions
 *   - Replaying committed transactions on startup
 */

#include "qihse_wal.h"

#include <stdlib.h>
#include <string.h>

/* ── Lifecycle ──────────────────────────────────────────────────────── */

void* tgrep_wal_create(const char* directory) {
    return (void*)qihse_wal_create(directory, 0, QIHSE_WAL_DURABILITY_FDATASYNC);
}

void tgrep_wal_destroy(void* wal) {
    if (wal) qihse_wal_destroy((qihse_wal_t*)wal);
}

/* ── Transaction helpers ────────────────────────────────────────────── */

uint64_t tgrep_wal_begin(void* wal, uint64_t txn_id) {
    if (!wal) return 0;
    return qihse_wal_append_begin((qihse_wal_t*)wal, txn_id);
}

uint64_t tgrep_wal_log_segment(void* wal, uint64_t txn_id,
                                const char* segment_name,
                                const void* metadata, uint32_t metadata_len) {
    if (!wal || !segment_name) return 0;
    return qihse_wal_append((qihse_wal_t*)wal, txn_id, 0,
                            QIHSE_WAL_OP_INSERT,
                            segment_name, (uint32_t)strlen(segment_name),
                            metadata, metadata_len);
}

int tgrep_wal_commit(void* wal, uint64_t txn_id) {
    if (!wal) return -1;
    uint64_t lsn = qihse_wal_append_commit((qihse_wal_t*)wal, txn_id);
    if (lsn == 0) return -1;
    return qihse_wal_flush((qihse_wal_t*)wal);
}

int tgrep_wal_abort(void* wal, uint64_t txn_id) {
    if (!wal) return -1;
    uint64_t lsn = qihse_wal_append_abort((qihse_wal_t*)wal, txn_id);
    if (lsn == 0) return -1;
    return qihse_wal_flush((qihse_wal_t*)wal);
}

int tgrep_wal_flush(void* wal) {
    if (!wal) return -1;
    return qihse_wal_flush((qihse_wal_t*)wal);
}

uint64_t tgrep_wal_current_lsn(void* wal) {
    if (!wal) return 0;
    return qihse_wal_current_lsn((qihse_wal_t*)wal);
}

int tgrep_wal_checkpoint(void* wal, uint64_t lsn) {
    if (!wal) return -1;
    return qihse_wal_checkpoint((qihse_wal_t*)wal, lsn);
}

/* ── Replay ─────────────────────────────────────────────────────────── */

/* Callback type: called for each committed record during replay.
 * Returns true to continue, false to stop. */
typedef bool (*tgrep_wal_replay_cb)(const char* key, uint32_t key_len,
                                     const void* value, uint32_t value_len,
                                     uint8_t op_type, uint64_t txn_id,
                                     void* user_data);

/* Internal context for filtered replay (only committed transactions) */
struct replay_ctx {
    uint64_t* committed_txns;
    int committed_count;
    int committed_capacity;
    bool alloc_failed;
    tgrep_wal_replay_cb user_cb;
    void* user_data;
};

/* First pass: collect committed transaction IDs */
static bool collect_committed_cb(const qihse_wal_record_t* record,
                                  const void* key, uint32_t key_len,
                                  const void* value, uint32_t value_len,
                                  void* user_data) {
    (void)key; (void)key_len; (void)value; (void)value_len;
    struct replay_ctx* ctx = (struct replay_ctx*)user_data;

    if (record->op_type == QIHSE_WAL_OP_COMMIT) {
        if (ctx->committed_count >= ctx->committed_capacity) {
            int new_cap = ctx->committed_capacity * 2;
            if (new_cap < 16) new_cap = 16;
            uint64_t* new_arr = (uint64_t*)realloc(ctx->committed_txns,
                                                    new_cap * sizeof(uint64_t));
            if (!new_arr) {
                ctx->alloc_failed = true;
                return false;
            }
            ctx->committed_txns = new_arr;
            ctx->committed_capacity = new_cap;
        }
        ctx->committed_txns[ctx->committed_count++] = record->txn_id;
    }
    return true;
}

/* Second pass: replay only records from committed transactions */
static bool replay_committed_cb(const qihse_wal_record_t* record,
                                 const void* key, uint32_t key_len,
                                 const void* value, uint32_t value_len,
                                 void* user_data) {
    struct replay_ctx* ctx = (struct replay_ctx*)user_data;

    /* Skip BEGIN/COMMIT/ABORT/CHECKPOINT — only replay data ops */
    if (record->op_type != QIHSE_WAL_OP_INSERT &&
        record->op_type != QIHSE_WAL_OP_UPDATE &&
        record->op_type != QIHSE_WAL_OP_DELETE) {
        return true;
    }

    /* Check if this transaction was committed */
    bool is_committed = false;
    for (int i = 0; i < ctx->committed_count; i++) {
        if (ctx->committed_txns[i] == record->txn_id) {
            is_committed = true;
            break;
        }
    }

    if (is_committed && ctx->user_cb) {
        return ctx->user_cb((const char*)key, key_len,
                            value, value_len,
                            record->op_type, record->txn_id,
                            ctx->user_data);
    }

    return true;
}

/* Replay the WAL, calling user_cb for each record from a committed transaction.
 * Returns the number of records replayed, or -1 on error. */
int tgrep_wal_replay(void* wal, tgrep_wal_replay_cb user_cb, void* user_data) {
    if (!wal) return -1;
    struct replay_ctx ctx = {0};
    ctx.user_cb = user_cb;
    ctx.user_data = user_data;

    /* First pass: collect committed transaction IDs */
    int n1 = qihse_wal_replay((qihse_wal_t*)wal, 0, collect_committed_cb, &ctx);
    if (n1 < 0 || ctx.alloc_failed) {
        free(ctx.committed_txns);
        return -1;
    }

    /* Second pass: replay records from committed transactions */
    int n2 = qihse_wal_replay((qihse_wal_t*)wal, 0, replay_committed_cb, &ctx);
    free(ctx.committed_txns);

    return n2;
}
