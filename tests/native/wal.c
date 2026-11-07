/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
#include "support.h"
#include "../../native/qihse_wal_wrapper.c"

struct replayed_records {
    char names[8][64];
    size_t count;
};

static bool collect_name(const char *key, uint32_t key_length, const void *value,
                         uint32_t value_length, uint8_t operation, uint64_t transaction,
                         void *opaque)
{
    (void)value;
    (void)value_length;
    (void)transaction;
    struct replayed_records *records = opaque;
    assert(operation == QIHSE_WAL_OP_INSERT);
    assert(records->count < 8 && key_length < sizeof(records->names[0]));
    memcpy(records->names[records->count], key, key_length);
    records->names[records->count++][key_length] = '\0';
    return true;
}

static void append_segment(void *wal, uint64_t transaction, const char *name)
{
    assert(tgrep_wal_log_segment(wal, transaction, name, NULL, 0) != 0);
}

static void reused_transaction_id_does_not_commit_later_records(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "reused-id");
    void *wal = tgrep_wal_create(path);
    assert(wal);
    assert(tgrep_wal_begin(wal, 1) != 0);
    append_segment(wal, 1, "committed");
    assert(tgrep_wal_commit(wal, 1) == 0);
    assert(tgrep_wal_begin(wal, 1) != 0);
    append_segment(wal, 1, "not-committed");
    assert(tgrep_wal_flush(wal) == 0);
    tgrep_wal_destroy(wal);
    wal = tgrep_wal_create(path);
    assert(wal);
    struct replayed_records records = {0};
    int result = tgrep_wal_replay(wal, collect_name, &records);
    assert(records.count == 1);
    assert(strcmp(records.names[0], "committed") == 0);
    assert(result == 1);
    tgrep_wal_destroy(wal);
}

static void abort_and_missing_commit_are_filtered(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "filtering");
    void *wal = tgrep_wal_create(path);
    assert(wal);
    assert(tgrep_wal_begin(wal, 10) != 0);
    assert(tgrep_wal_begin(wal, 20) != 0);
    append_segment(wal, 10, "committed");
    append_segment(wal, 20, "aborted");
    assert(tgrep_wal_commit(wal, 10) == 0);
    assert(tgrep_wal_abort(wal, 20) == 0);
    assert(tgrep_wal_begin(wal, 30) != 0);
    append_segment(wal, 30, "unfinished");
    assert(tgrep_wal_flush(wal) == 0);
    struct replayed_records records = {0};
    assert(tgrep_wal_replay(wal, collect_name, &records) == 1);
    assert(records.count == 1 && strcmp(records.names[0], "committed") == 0);
    tgrep_wal_destroy(wal);
}

static void repeated_committed_ids_replay_each_incarnation(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "two-commits");
    void *wal = tgrep_wal_create(path);
    assert(wal);
    assert(tgrep_wal_begin(wal, 1) != 0);
    append_segment(wal, 1, "first");
    assert(tgrep_wal_commit(wal, 1) == 0);
    assert(tgrep_wal_begin(wal, 1) != 0);
    append_segment(wal, 1, "second");
    assert(tgrep_wal_commit(wal, 1) == 0);
    struct replayed_records records = {0};
    assert(tgrep_wal_replay(wal, collect_name, &records) == 2);
    assert(records.count == 2);
    assert(strcmp(records.names[0], "first") == 0 && strcmp(records.names[1], "second") == 0);
    tgrep_wal_destroy(wal);
}

int main(int argc, char **argv)
{
    disable_core_dumps();
    assert(argc == 2);
    const char *directory = argv[1];
    RUN_TEST(reused_transaction_id_does_not_commit_later_records);
    RUN_TEST(abort_and_missing_commit_are_filtered);
    RUN_TEST(repeated_committed_ids_replay_each_incarnation);
    return 0;
}
