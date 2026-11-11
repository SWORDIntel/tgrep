/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
#include "support.h"
#include <errno.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/uio.h>

/* Deterministic syscall faults at the linked native WAL boundary. The ordinary
 * path still performs real writes/syncs; production code has no test switches. */
enum write_fault { WRITE_NORMAL, WRITE_INTERRUPTED_SHORT, WRITE_PARTIAL_ERROR, WRITE_ZERO };
static enum write_fault write_fault;
static unsigned write_calls;
static int sync_fail_after = -1;
static int directory_sync_fail_after = -1;

ssize_t writev(int fd, const struct iovec *iov, int count)
{
    unsigned call = write_calls++;
    if (write_fault == WRITE_ZERO)
        return 0;
    if (write_fault == WRITE_INTERRUPTED_SHORT && call == 0) {
        errno = EINTR;
        return -1;
    }
    if ((write_fault == WRITE_INTERRUPTED_SHORT && call == 1) ||
        (write_fault == WRITE_PARTIAL_ERROR && call == 0)) {
        struct iovec short_iov = iov[0];
        if (short_iov.iov_len > 15)
            short_iov.iov_len = 15;
        return syscall(SYS_writev, fd, &short_iov, 1);
    }
    if (write_fault == WRITE_PARTIAL_ERROR) {
        errno = EIO;
        return -1;
    }
    return syscall(SYS_writev, fd, iov, count);
}

int fdatasync(int fd)
{
    if (sync_fail_after == 0) {
        errno = EIO;
        return -1;
    }
    if (sync_fail_after > 0)
        --sync_fail_after;
    return (int)syscall(SYS_fdatasync, fd);
}

int fsync(int fd)
{
    if (directory_sync_fail_after == 0) {
        errno = EIO;
        return -1;
    }
    if (directory_sync_fail_after > 0)
        --directory_sync_fail_after;
    return (int)syscall(SYS_fsync, fd);
}

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

static off_t file_size(const char *path)
{
    struct stat st;
    assert(stat(path, &st) == 0);
    return st.st_size;
}

static void log_path(char *out, size_t capacity, const char *directory, uint64_t index)
{
    char name[64];
    int length = snprintf(name, sizeof(name), "wal_%020llu.log", (unsigned long long)index);
    assert(length > 0 && (size_t)length < sizeof(name));
    test_path(out, capacity, directory, name);
}

static void incomplete_final_record_is_repaired_before_append(const char *directory)
{
    for (int payload_tail = 0; payload_tail < 2; ++payload_tail) {
        char path[PATH_MAX], log[PATH_MAX];
        test_path(path, sizeof(path), directory, payload_tail ? "torn-payload" : "torn-header");
        void *wal = tgrep_wal_create(path);
        assert(wal);
        assert(tgrep_wal_begin(wal, 1));
        append_segment(wal, 1, "before-tail");
        assert(tgrep_wal_commit(wal, 1) == 0);
        log_path(log, sizeof(log), path, 0);
        if (payload_tail)
            assert(tgrep_wal_begin(wal, 2));
        off_t good_end = file_size(log);
        uint64_t expected_next = tgrep_wal_current_lsn(wal);
        if (payload_tail) {
            append_segment(wal, 2, "incomplete-payload-record");
            assert(truncate(log, good_end + QIHSE_WAL_RECORD_HEADER_SIZE + 3) == 0);
        } else {
            int fd = open(log, O_WRONLY | O_APPEND);
            assert(fd >= 0);
            assert(write(fd, "partial-header!", 15) == 15);
            assert(close(fd) == 0);
        }
        tgrep_wal_destroy(wal);
        wal = tgrep_wal_create(path);
        assert(wal);
        assert(file_size(log) == good_end);
        assert(tgrep_wal_current_lsn(wal) == expected_next);
        assert(tgrep_wal_begin(wal, 3) == expected_next);
        append_segment(wal, 3, "after-tail");
        assert(tgrep_wal_commit(wal, 3) == 0);
        tgrep_wal_destroy(wal);
        wal = tgrep_wal_create(path);
        assert(wal && tgrep_wal_current_lsn(wal) == expected_next + 3);
        struct replayed_records records = {0};
        assert(tgrep_wal_replay(wal, collect_name, &records) == 2);
        assert(strcmp(records.names[0], "before-tail") == 0);
        assert(strcmp(records.names[1], "after-tail") == 0);
        tgrep_wal_destroy(wal);
    }
}

static void corrupt_byte(const char *path, off_t offset)
{
    int fd = open(path, O_RDWR);
    assert(fd >= 0);
    unsigned char byte;
    assert(pread(fd, &byte, 1, offset) == 1);
    byte ^= 0x40;
    assert(pwrite(fd, &byte, 1, offset) == 1);
    assert(close(fd) == 0);
}

static void rejected_open_preserves_bytes(const char *directory, const char *log)
{
    off_t length = file_size(log);
    assert(length >= 0 && length < 1024);
    uint8_t before[1024], after[1024];
    int fd = open(log, O_RDONLY);
    assert(fd >= 0 && pread(fd, before, (size_t)length, 0) == length);
    assert(tgrep_wal_create(directory) == NULL && errno == EILSEQ);
    assert(file_size(log) == length && pread(fd, after, (size_t)length, 0) == length);
    assert(memcmp(before, after, (size_t)length) == 0);
    assert(close(fd) == 0);
}

static void last_record_header_and_payload_damage_is_not_repaired(const char *directory)
{
    const char payload[] = "tail";
    for (unsigned byte = 0; byte < QIHSE_WAL_RECORD_HEADER_SIZE + sizeof(payload) - 1; ++byte) {
        char path[PATH_MAX], log[PATH_MAX], name[64];
        snprintf(name, sizeof(name), "last-record-damage-%u", byte);
        test_path(path, sizeof(path), directory, name);
        void *wal = tgrep_wal_create(path);
        assert(wal && tgrep_wal_begin(wal, 1));
        append_segment(wal, 1, payload);
        tgrep_wal_destroy(wal);
        log_path(log, sizeof(log), path, 0);
        corrupt_byte(log, QIHSE_WAL_HEADER_SIZE + QIHSE_WAL_RECORD_HEADER_SIZE + byte);
        /* In particular, damaged key/value lengths must not look like an EOF
         * tear even though this is the final record and nothing follows it. */
        rejected_open_preserves_bytes(path, log);
    }
}

static void segment_header_damage_and_wrong_index_fail_closed(const char *directory)
{
    for (unsigned byte = 0; byte < QIHSE_WAL_HEADER_SIZE; ++byte) {
        char path[PATH_MAX], log[PATH_MAX], name[64];
        snprintf(name, sizeof(name), "segment-header-damage-%u", byte);
        test_path(path, sizeof(path), directory, name);
        void *wal = tgrep_wal_create(path);
        assert(wal);
        tgrep_wal_destroy(wal);
        log_path(log, sizeof(log), path, 0);
        corrupt_byte(log, byte);
        rejected_open_preserves_bytes(path, log);
    }
    char path[PATH_MAX], old_log[PATH_MAX], new_log[PATH_MAX];
    test_path(path, sizeof(path), directory, "segment-wrong-index");
    void *wal = tgrep_wal_create(path);
    assert(wal);
    tgrep_wal_destroy(wal);
    log_path(old_log, sizeof(old_log), path, 0);
    log_path(new_log, sizeof(new_log), path, 7);
    assert(rename(old_log, new_log) == 0);
    rejected_open_preserves_bytes(path, new_log);
}

static void every_torn_v2_record_boundary_is_repaired(const char *directory)
{
    const char payload[] = "tail-payload";
    for (unsigned kept = 1; kept < QIHSE_WAL_RECORD_HEADER_SIZE + sizeof(payload) - 1; ++kept) {
        char path[PATH_MAX], log[PATH_MAX], name[64];
        snprintf(name, sizeof(name), "torn-v2-boundary-%u", kept);
        test_path(path, sizeof(path), directory, name);
        void *wal = tgrep_wal_create(path);
        assert(wal && tgrep_wal_begin(wal, 1));
        log_path(log, sizeof(log), path, 0);
        off_t good_end = file_size(log);
        append_segment(wal, 1, payload);
        tgrep_wal_destroy(wal);
        assert(truncate(log, good_end + kept) == 0);
        wal = tgrep_wal_create(path);
        assert(wal && file_size(log) == good_end && tgrep_wal_current_lsn(wal) == 2);
        assert(tgrep_wal_begin(wal, 2) == 2);
        append_segment(wal, 2, "after-tail");
        assert(tgrep_wal_commit(wal, 2) == 0);
        struct replayed_records records = {0};
        assert(tgrep_wal_replay(wal, collect_name, &records) == 1);
        assert(strcmp(records.names[0], "after-tail") == 0);
        tgrep_wal_destroy(wal);
    }
}

/* Independent writer for the old host-endian, 30-byte headerless format. */
static void write_legacy_record(int fd, uint64_t lsn, uint64_t transaction,
                                uint8_t operation, const char *key)
{
    uint8_t header[QIHSE_WAL_LEGACY_RECORD_HEADER_SIZE] = {0};
    uint32_t key_length = key ? (uint32_t)strlen(key) : 0;
    memcpy(header, &lsn, 8);
    memcpy(header + 8, &transaction, 8);
    header[17] = operation;
    memcpy(header + 18, &key_length, 4);
    uint32_t crc = qihse_wal_crc32(header, 8) ^ qihse_wal_crc32(header + 8, 8) ^
                   qihse_wal_crc32(header + 16, 1) ^ qihse_wal_crc32(header + 17, 1) ^
                   qihse_wal_crc32(header + 18, 4) ^ qihse_wal_crc32(header + 22, 4);
    if (key_length)
        crc ^= qihse_wal_crc32(key, key_length);
    memcpy(header + 26, &crc, 4);
    assert(write(fd, header, sizeof(header)) == (ssize_t)sizeof(header));
    if (key_length)
        assert(write(fd, key, key_length) == (ssize_t)key_length);
}

static int create_legacy_log(const char *directory, char *log, size_t capacity)
{
    assert(mkdir(directory, 0700) == 0);
    log_path(log, capacity, directory, 0);
    int fd = open(log, O_RDWR | O_CREAT | O_EXCL, 0600);
    assert(fd >= 0);
    write_legacy_record(fd, 1, 1, QIHSE_WAL_OP_BEGIN, NULL);
    write_legacy_record(fd, 2, 1, QIHSE_WAL_OP_INSERT, "legacy-first");
    write_legacy_record(fd, 3, 1, QIHSE_WAL_OP_COMMIT, NULL);
    return fd;
}

static void legacy_logs_transition_without_rewriting_and_checkpoint_normally(const char *directory)
{
    char path[PATH_MAX], legacy[PATH_MAX], current[PATH_MAX];
    test_path(path, sizeof(path), directory, "legacy-transition");
    int fd = create_legacy_log(path, legacy, sizeof(legacy));
    /* Old binaries reused LSNs and transaction IDs on reopen. */
    write_legacy_record(fd, 1, 1, QIHSE_WAL_OP_BEGIN, NULL);
    write_legacy_record(fd, 2, 1, QIHSE_WAL_OP_INSERT, "legacy-second");
    write_legacy_record(fd, 3, 1, QIHSE_WAL_OP_COMMIT, NULL);
    off_t length = file_size(legacy);
    uint8_t before[256], after[256];
    assert(length < (off_t)sizeof(before));
    assert(pread(fd, before, (size_t)length, 0) == length);
    void *wal = tgrep_wal_create(path);
    assert(wal && tgrep_wal_current_lsn(wal) == 4);
    log_path(current, sizeof(current), path, 1);
    assert(file_size(current) == QIHSE_WAL_HEADER_SIZE);
    assert(tgrep_wal_begin(wal, 1) == 4);
    append_segment(wal, 1, "v2-third");
    assert(tgrep_wal_commit(wal, 1) == 0);
    assert(file_size(legacy) == length && pread(fd, after, (size_t)length, 0) == length);
    assert(memcmp(before, after, (size_t)length) == 0 && close(fd) == 0);
    tgrep_wal_destroy(wal);
    wal = tgrep_wal_create(path);
    assert(wal && tgrep_wal_current_lsn(wal) == 7);
    struct replayed_records records = {0};
    assert(tgrep_wal_replay(wal, collect_name, &records) == 3);
    assert(strcmp(records.names[0], "legacy-first") == 0);
    assert(strcmp(records.names[1], "legacy-second") == 0);
    assert(strcmp(records.names[2], "v2-third") == 0);
    assert(tgrep_wal_checkpoint(wal, 7) == 0);
    assert(access(legacy, F_OK) != 0 && errno == ENOENT);
    tgrep_wal_destroy(wal);
    wal = tgrep_wal_create(path);
    assert(wal && tgrep_wal_current_lsn(wal) == 8);
    assert(qihse_wal_last_checkpoint(wal) == 7);
    tgrep_wal_destroy(wal);
}

static void ambiguous_legacy_tails_are_never_truncated(const char *directory)
{
    for (unsigned mode = 0; mode < 3; ++mode) {
        char path[PATH_MAX], log[PATH_MAX], name[64];
        snprintf(name, sizeof(name), "ambiguous-legacy-tail-%u", mode);
        test_path(path, sizeof(path), directory, name);
        int fd = create_legacy_log(path, log, sizeof(log));
        off_t good_end = file_size(log);
        write_legacy_record(fd, 4, 1, QIHSE_WAL_OP_INSERT, "incomplete-tail");
        if (mode == 0) {
            assert(ftruncate(fd, good_end + 15) == 0);
        } else if (mode == 1) {
            assert(ftruncate(fd, good_end + QIHSE_WAL_LEGACY_RECORD_HEADER_SIZE + 3) == 0);
        } else {
            /* Complete last legacy record with a damaged length, no surviving
             * later record: the ambiguity that previously allowed data loss. */
            uint8_t damaged = 1;
            assert(pwrite(fd, &damaged, 1, good_end + 19) == 1);
        }
        assert(close(fd) == 0);
        rejected_open_preserves_bytes(path, log);
    }
}

static void segment_creation_publishes_only_complete_headers(const char *directory)
{
    for (unsigned mode = 0; mode < 4; ++mode) {
        char path[PATH_MAX], log[PATH_MAX], name[64];
        snprintf(name, sizeof(name), "creation-fault-%u", mode);
        test_path(path, sizeof(path), directory, name);
        write_calls = 0;
        if (mode == 0)
            write_fault = WRITE_PARTIAL_ERROR;
        else if (mode == 1)
            sync_fail_after = 0;
        else if (mode == 2)
            directory_sync_fail_after = 0;
        else
            write_fault = WRITE_INTERRUPTED_SHORT;
        void *wal = tgrep_wal_create(path);
        assert((mode == 3) == (wal != NULL));
        write_fault = WRITE_NORMAL;
        sync_fail_after = -1;
        directory_sync_fail_after = -1;
        tgrep_wal_destroy(wal);
        log_path(log, sizeof(log), path, 0);
        if (mode < 2)
            assert(access(log, F_OK) != 0 && errno == ENOENT);
        else
            assert(file_size(log) == QIHSE_WAL_HEADER_SIZE);
        wal = tgrep_wal_create(path);
        assert(wal && tgrep_wal_begin(wal, 1) == 1);
        append_segment(wal, 1, "after-creation-fault");
        assert(tgrep_wal_commit(wal, 1) == 0);
        struct replayed_records records = {0};
        assert(tgrep_wal_replay(wal, collect_name, &records) == 1);
        tgrep_wal_destroy(wal);
    }
}

static void complete_corruption_rejects_replay_open_and_checkpoint(const char *directory)
{
    char path[PATH_MAX], first[PATH_MAX], last[PATH_MAX];
    test_path(path, sizeof(path), directory, "corrupt-checksum");
    void *wal = qihse_wal_create(path, 40, QIHSE_WAL_DURABILITY_FDATASYNC);
    assert(wal);
    assert(tgrep_wal_begin(wal, 1));
    append_segment(wal, 1, "committed");
    assert(tgrep_wal_commit(wal, 1) == 0);
    log_path(first, sizeof(first), path, 0);
    log_path(last, sizeof(last), path, 2);
    off_t first_size = file_size(first), last_size = file_size(last);
    corrupt_byte(last, last_size - 1); /* Complete record, including all checksum bytes. */
    struct replayed_records records = {0};
    assert(tgrep_wal_replay(wal, collect_name, &records) < 0);
    assert(records.count == 0); /* First pass fails before any committed callback. */
    assert(tgrep_wal_checkpoint(wal, tgrep_wal_current_lsn(wal)) < 0);
    assert(file_size(first) == first_size && file_size(last) == last_size);
    tgrep_wal_destroy(wal);
    assert(tgrep_wal_create(path) == NULL);
    assert(file_size(first) == first_size && file_size(last) == last_size);
}

static void truncated_older_segment_is_not_repaired(const char *directory)
{
    char path[PATH_MAX], first[PATH_MAX], last[PATH_MAX];
    test_path(path, sizeof(path), directory, "older-partial");
    void *wal = qihse_wal_create(path, 40, QIHSE_WAL_DURABILITY_FDATASYNC);
    assert(wal);
    assert(tgrep_wal_begin(wal, 1));
    append_segment(wal, 1, "committed");
    assert(tgrep_wal_commit(wal, 1) == 0);
    tgrep_wal_destroy(wal);
    log_path(first, sizeof(first), path, 0);
    log_path(last, sizeof(last), path, 2);
    assert(truncate(first, 15) == 0);
    assert(tgrep_wal_create(path) == NULL);
    assert(file_size(first) == 15 &&
           file_size(last) == QIHSE_WAL_HEADER_SIZE + QIHSE_WAL_RECORD_HEADER_SIZE);
}

static void corrupt_length_before_commits_is_not_a_torn_tail(const char *directory)
{
    char path[PATH_MAX], log[PATH_MAX];
    test_path(path, sizeof(path), directory, "corrupt-length");
    void *wal = tgrep_wal_create(path);
    assert(wal);
    assert(tgrep_wal_begin(wal, 1));
    append_segment(wal, 1, "first");
    assert(tgrep_wal_commit(wal, 1) == 0);
    assert(tgrep_wal_begin(wal, 2));
    append_segment(wal, 2, "second");
    assert(tgrep_wal_commit(wal, 2) == 0);
    log_path(log, sizeof(log), path, 0);
    off_t original_size = file_size(log);
    enum { LOG_SIZE = QIHSE_WAL_HEADER_SIZE + 6 * QIHSE_WAL_RECORD_HEADER_SIZE + 11 };
    assert(original_size == LOG_SIZE);
    int fd = open(log, O_RDWR);
    assert(fd >= 0);
    uint8_t damaged = 1; /* First INSERT key length becomes 261, exceeding EOF. */
    assert(pwrite(fd, &damaged, 1,
                  QIHSE_WAL_HEADER_SIZE + QIHSE_WAL_RECORD_HEADER_SIZE + 19) == 1);
    uint8_t before[LOG_SIZE], after[LOG_SIZE];
    assert(pread(fd, before, sizeof(before), 0) == (ssize_t)sizeof(before));
    struct replayed_records records = {0};
    assert(tgrep_wal_replay(wal, collect_name, &records) < 0 && records.count == 0);
    assert(tgrep_wal_checkpoint(wal, tgrep_wal_current_lsn(wal)) < 0);
    tgrep_wal_destroy(wal);
    assert(tgrep_wal_create(path) == NULL);
    assert(file_size(log) == original_size);
    assert(pread(fd, after, sizeof(after), 0) == (ssize_t)sizeof(after));
    assert(memcmp(before, after, sizeof(before)) == 0);
    assert(close(fd) == 0);
}

static void checkpoint_preserves_lsn_after_rotation_and_reopen(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "checkpoint-lsn");
    qihse_wal_t *wal = qihse_wal_create(path, 40, QIHSE_WAL_DURABILITY_FDATASYNC);
    assert(wal);
    assert(tgrep_wal_begin(wal, 1) == 1);
    append_segment(wal, 1, "before-checkpoint");
    assert(tgrep_wal_commit(wal, 1) == 0);
    uint64_t checkpoint = tgrep_wal_current_lsn(wal);
    assert(tgrep_wal_checkpoint(wal, checkpoint) == 0);
    uint64_t next = tgrep_wal_current_lsn(wal);
    assert(next > checkpoint);
    tgrep_wal_destroy(wal);
    wal = qihse_wal_create(path, 40, QIHSE_WAL_DURABILITY_FDATASYNC);
    assert(wal && tgrep_wal_current_lsn(wal) == next);
    assert(qihse_wal_last_checkpoint(wal) == checkpoint);
    assert(tgrep_wal_begin(wal, 2) == next);
    append_segment(wal, 2, "after-checkpoint");
    assert(tgrep_wal_commit(wal, 2) == 0);
    struct replayed_records records = {0};
    assert(tgrep_wal_replay(wal, collect_name, &records) == 1);
    assert(strcmp(records.names[0], "after-checkpoint") == 0);
    tgrep_wal_destroy(wal);
}

static void short_writes_and_interruptions_are_completed(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "short-completed");
    void *wal = tgrep_wal_create(path);
    assert(wal);
    write_fault = WRITE_INTERRUPTED_SHORT;
    write_calls = 0;
    assert(tgrep_wal_begin(wal, 1) == 1);
    assert(write_calls >= 3);
    write_fault = WRITE_NORMAL;
    append_segment(wal, 1, "complete");
    assert(tgrep_wal_commit(wal, 1) == 0);
    tgrep_wal_destroy(wal);
    wal = tgrep_wal_create(path);
    assert(wal);
    struct replayed_records records = {0};
    assert(tgrep_wal_replay(wal, collect_name, &records) == 1);
    tgrep_wal_destroy(wal);
}

static void write_failure_blocks_commit_until_reopen(const char *directory)
{
    for (int zero = 0; zero < 2; ++zero) {
        char path[PATH_MAX];
        test_path(path, sizeof(path), directory, zero ? "zero-write" : "partial-write-error");
        void *wal = tgrep_wal_create(path);
        assert(wal && tgrep_wal_begin(wal, 1));
        uint64_t expected_next = tgrep_wal_current_lsn(wal);
        write_fault = zero ? WRITE_ZERO : WRITE_PARTIAL_ERROR;
        write_calls = 0;
        assert(tgrep_wal_log_segment(wal, 1, "unfinished", NULL, 0) == 0);
        write_fault = WRITE_NORMAL;
        assert(tgrep_wal_commit(wal, 1) < 0);
        assert(tgrep_wal_flush(wal) < 0);
        tgrep_wal_destroy(wal);
        wal = tgrep_wal_create(path);
        assert(wal && tgrep_wal_current_lsn(wal) == expected_next);
        assert(tgrep_wal_begin(wal, 2) == expected_next);
        append_segment(wal, 2, "after-error");
        assert(tgrep_wal_commit(wal, 2) == 0);
        struct replayed_records records = {0};
        assert(tgrep_wal_replay(wal, collect_name, &records) == 1);
        assert(strcmp(records.names[0], "after-error") == 0);
        tgrep_wal_destroy(wal);
    }
}

static void sync_failures_are_reported_and_prevent_cleanup(const char *directory)
{
    for (unsigned mode = 0; mode < 5; ++mode) {
        char path[PATH_MAX], first[PATH_MAX], name[32];
        snprintf(name, sizeof(name), "sync-failure-%u", mode);
        test_path(path, sizeof(path), directory, name);
        void *wal = qihse_wal_create(path, 40, QIHSE_WAL_DURABILITY_GROUP_COMMIT);
        assert(wal);
        assert(tgrep_wal_begin(wal, 1));
        append_segment(wal, 1, "committed");
        assert(tgrep_wal_commit(wal, 1) == 0);
        log_path(first, sizeof(first), path, 0);
        off_t original_size = file_size(first);
        sync_fail_after = mode == 4 ? 1 : 0;
        if (mode == 0) {
            assert(tgrep_wal_flush(wal) < 0);
        } else if (mode == 1) {
            /* A commit can fail in rotation or the final durability flush. */
            assert(tgrep_wal_commit(wal, 1) < 0);
        } else if (mode == 2) {
            struct replayed_records records = {0};
            assert(tgrep_wal_replay(wal, collect_name, &records) < 0);
            assert(records.count == 0);
        } else {
            /* Before validation, or while making the checkpoint durable. */
            assert(tgrep_wal_checkpoint(wal, tgrep_wal_current_lsn(wal)) < 0);
        }
        sync_fail_after = -1;
        assert(file_size(first) == original_size);
        assert(tgrep_wal_begin(wal, 99) == 0); /* Poisoned even after fault disappears. */
        tgrep_wal_destroy(wal);
    }
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "append-sync-error");
    void *wal = tgrep_wal_create(path);
    assert(wal);
    sync_fail_after = 0;
    assert(tgrep_wal_begin(wal, 1) == 0);
    sync_fail_after = -1;
    assert(tgrep_wal_commit(wal, 1) < 0);
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
    RUN_TEST(incomplete_final_record_is_repaired_before_append);
    RUN_TEST(last_record_header_and_payload_damage_is_not_repaired);
    RUN_TEST(segment_header_damage_and_wrong_index_fail_closed);
    RUN_TEST(every_torn_v2_record_boundary_is_repaired);
    RUN_TEST(legacy_logs_transition_without_rewriting_and_checkpoint_normally);
    RUN_TEST(ambiguous_legacy_tails_are_never_truncated);
    RUN_TEST(segment_creation_publishes_only_complete_headers);
    RUN_TEST(complete_corruption_rejects_replay_open_and_checkpoint);
    RUN_TEST(corrupt_length_before_commits_is_not_a_torn_tail);
    RUN_TEST(truncated_older_segment_is_not_repaired);
    RUN_TEST(checkpoint_preserves_lsn_after_rotation_and_reopen);
    RUN_TEST(short_writes_and_interruptions_are_completed);
    RUN_TEST(write_failure_blocks_commit_until_reopen);
    RUN_TEST(sync_failures_are_reported_and_prevent_cleanup);
    return 0;
}
