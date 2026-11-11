#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
/*
 * QIHSE Unified Write-Ahead Log.
 *
 * The caller must serialize WAL open/repair, append, replay, and checkpoint
 * across processes. tgrep holds its state-directory writer lock throughout.
 * A short final v2 EOF record is repairable; a complete corrupt record is not.
 * Legacy tails lack independently checked lengths and are never truncated.
 */
#include "qihse_wal.h"

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <unistd.h>

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif

struct qihse_wal {
    pthread_mutex_t lock;
    char directory[PATH_MAX];
    size_t segment_size;
    qihse_wal_durability_t durability;
    uint64_t next_lsn;
    uint64_t last_checkpoint_lsn;
    int seg_fd;
    uint64_t seg_index;
    size_t seg_offset;
    char seg_path[PATH_MAX];
    bool failed; /* An uncertain append must not be followed by another record. */
};

struct wal_segment {
    uint64_t index;
    uint64_t max_lsn;
    uint64_t checkpoint_lsn;
    off_t size;
    off_t valid_end;
    bool partial;
    unsigned format;
};

static uint32_t crc32_update(uint32_t crc, const void *data, size_t len)
{
    const uint8_t *p = data;
    for (size_t i = 0; i < len; i++) {
        crc ^= p[i];
        for (int j = 0; j < 8; j++) {
            uint32_t mask = -(crc & 1);
            crc = (crc >> 1) ^ (0xEDB88320u & mask);
        }
    }
    return crc;
}

uint32_t qihse_wal_crc32(const void *data, size_t len)
{
    return ~crc32_update(0xFFFFFFFFu, data, len);
}

static uint32_t record_crc(const qihse_wal_record_t *rec, const void *key, const void *value)
{
    uint32_t crc = qihse_wal_crc32(&rec->lsn, 8);
    crc ^= qihse_wal_crc32(&rec->txn_id, 8);
    crc ^= qihse_wal_crc32(&rec->engine_id, 1);
    crc ^= qihse_wal_crc32(&rec->op_type, 1);
    crc ^= qihse_wal_crc32(&rec->key_length, 4);
    crc ^= qihse_wal_crc32(&rec->value_length, 4);
    if (rec->key_length)
        crc ^= qihse_wal_crc32(key, rec->key_length);
    if (rec->value_length)
        crc ^= qihse_wal_crc32(value, rec->value_length);
    return crc;
}

static uint32_t payload_crc(const qihse_wal_record_t *rec, const void *key, const void *value)
{
    uint32_t crc = crc32_update(0xFFFFFFFFu, key, rec->key_length);
    return ~crc32_update(crc, value, rec->value_length);
}

static void put_u32(uint8_t *bytes, uint32_t value)
{
    for (unsigned i = 0; i < 4; ++i)
        bytes[i] = (uint8_t)(value >> (8 * i));
}

static void put_u64(uint8_t *bytes, uint64_t value)
{
    for (unsigned i = 0; i < 8; ++i)
        bytes[i] = (uint8_t)(value >> (8 * i));
}

static uint32_t get_u32(const uint8_t *bytes)
{
    uint32_t value = 0;
    for (unsigned i = 0; i < 4; ++i)
        value |= (uint32_t)bytes[i] << (8 * i);
    return value;
}

static uint64_t get_u64(const uint8_t *bytes)
{
    uint64_t value = 0;
    for (unsigned i = 0; i < 8; ++i)
        value |= (uint64_t)bytes[i] << (8 * i);
    return value;
}

static void segment_header(uint8_t *header, uint64_t index)
{
    memset(header, 0, QIHSE_WAL_HEADER_SIZE);
    memcpy(header + 8, "QWALv2\0", 8);
    put_u32(header + 16, QIHSE_WAL_FORMAT_VER);
    put_u64(header + 20, index);
    put_u32(header + 28, qihse_wal_crc32(header, 28));
}

static int seg_path(char *buf, size_t capacity, const char *directory, uint64_t index)
{
    int n = snprintf(buf, capacity, "%s/wal_%020" PRIu64 ".log", directory, index);
    if (n < 0 || (size_t)n >= capacity) {
        errno = ENAMETOOLONG;
        return -1;
    }
    return 0;
}

static bool segment_index(const char *name, uint64_t *index)
{
    if (strlen(name) != 28 || memcmp(name, "wal_", 4) || memcmp(name + 24, ".log", 4))
        return false;
    uint64_t value = 0;
    for (size_t i = 4; i < 24; ++i) {
        if (name[i] < '0' || name[i] > '9')
            return false;
        unsigned digit = (unsigned)(name[i] - '0');
        if (value > (UINT64_MAX - digit) / 10)
            return false;
        value = value * 10 + digit;
    }
    *index = value;
    return true;
}

static int compare_segments(const void *left, const void *right)
{
    uint64_t a = ((const struct wal_segment *)left)->index;
    uint64_t b = ((const struct wal_segment *)right)->index;
    return (a > b) - (a < b);
}

static int list_segments(const char *directory, struct wal_segment **out, size_t *count)
{
    *out = NULL;
    *count = 0;
    DIR *dir = opendir(directory);
    if (!dir)
        return -1;
    size_t capacity = 0;
    int saved_error = 0;
    for (;;) {
        errno = 0;
        struct dirent *entry = readdir(dir);
        if (!entry) {
            saved_error = errno;
            break;
        }
        uint64_t index;
        if (!segment_index(entry->d_name, &index))
            continue;
        if (*count == capacity) {
            if (capacity > SIZE_MAX / 2 / sizeof(**out)) {
                saved_error = EOVERFLOW;
                break;
            }
            size_t next = capacity ? capacity * 2 : 16;
            void *grown = realloc(*out, next * sizeof(**out));
            if (!grown) {
                saved_error = errno;
                break;
            }
            *out = grown;
            capacity = next;
        }
        (*out)[(*count)++] = (struct wal_segment){.index = index};
    }
    if (closedir(dir) != 0 && !saved_error)
        saved_error = errno;
    if (saved_error) {
        free(*out);
        *out = NULL;
        *count = 0;
        errno = saved_error;
        return -1;
    }
    if (*count > 1)
        qsort(*out, *count, sizeof(**out), compare_segments);
    return 0;
}

static int sync_fd(int fd)
{
    int result;
    do {
        result = fdatasync(fd);
    } while (result != 0 && errno == EINTR);
    return result;
}

static int sync_directory(const char *directory)
{
    int fd = open(directory, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (fd < 0)
        return -1;
    int result;
    do {
        result = fsync(fd);
    } while (result != 0 && errno == EINTR);
    int saved_error = errno;
    if (close(fd) != 0 && result == 0)
        return -1;
    errno = saved_error;
    return result;
}

static int read_at(int fd, void *buffer, size_t length, off_t offset)
{
    uint8_t *bytes = buffer;
    while (length) {
        ssize_t n = pread(fd, bytes, length, offset);
        if (n < 0 && errno == EINTR)
            continue;
        if (n <= 0) {
            if (n == 0)
                errno = EIO; /* File changed after its length was checked. */
            return -1;
        }
        bytes += n;
        length -= (size_t)n;
        offset += n;
    }
    return 0;
}

static bool decode_record(const uint8_t *hdr, unsigned format, qihse_wal_record_t *rec)
{
    if (format == QIHSE_WAL_FORMAT_VER) {
        /* Check before interpreting lengths or deciding that EOF is repairable. */
        if (qihse_wal_crc32(hdr, 30) != get_u32(hdr + 30))
            return false;
        rec->lsn = get_u64(hdr);
        rec->txn_id = get_u64(hdr + 8);
        rec->key_length = get_u32(hdr + 18);
        rec->value_length = get_u32(hdr + 22);
        rec->checksum = get_u32(hdr + 26);
    } else {
        /* The original headerless format used host byte order. */
        memcpy(&rec->lsn, hdr, 8);
        memcpy(&rec->txn_id, hdr + 8, 8);
        memcpy(&rec->key_length, hdr + 18, 4);
        memcpy(&rec->value_length, hdr + 22, 4);
        memcpy(&rec->checksum, hdr + 26, 4);
    }
    rec->engine_id = hdr[16];
    rec->op_type = hdr[17];
    return rec->lsn != 0 && rec->op_type >= QIHSE_WAL_OP_INSERT &&
           rec->op_type <= QIHSE_WAL_OP_CHECKPOINT &&
           rec->key_length <= QIHSE_WAL_MAX_KEY && rec->value_length <= QIHSE_WAL_MAX_VALUE &&
           (!(rec->op_type >= QIHSE_WAL_OP_BEGIN && rec->op_type <= QIHSE_WAL_OP_ABORT) ||
            (!rec->key_length && !rec->value_length)) &&
           (rec->op_type != QIHSE_WAL_OP_CHECKPOINT ||
            (rec->key_length == sizeof(uint64_t) && !rec->value_length));
}

/* Returns 0 after validation, 1 when the callback stops, -1 on any error.
 * Only the last v2 segment may end with an incomplete record header/payload,
 * and only open/repair permits that. Legacy incomplete tails and all complete
 * checksum failures fail closed without modifying the log. */
static int scan_segment(const char *directory, struct wal_segment *segment, bool allow_partial,
                        uint64_t start_lsn, qihse_wal_replay_cb callback, void *user_data,
                        int *replayed)
{
    char path[PATH_MAX];
    if (seg_path(path, sizeof(path), directory, segment->index) != 0)
        return -1;
    int fd = open(path, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0)
        return -1;
    int result = -1;
    struct stat st;
    if (fstat(fd, &st) != 0)
        goto done;
    if (!S_ISREG(st.st_mode) || st.st_size < 0 || (uintmax_t)st.st_size > SIZE_MAX) {
        errno = EINVAL;
        goto done;
    }
    segment->size = st.st_size;
    segment->valid_end = 0;
    segment->partial = false;
    segment->max_lsn = 0;
    segment->checkpoint_lsn = 0;
    segment->format = 1;
    if (segment->size >= 8) {
        uint8_t prefix[8];
        if (read_at(fd, prefix, sizeof(prefix), 0) != 0)
            goto done;
        if (get_u64(prefix) == 0) {
            uint8_t header[QIHSE_WAL_HEADER_SIZE], expected[QIHSE_WAL_HEADER_SIZE];
            if (segment->size < QIHSE_WAL_HEADER_SIZE) {
                errno = EILSEQ;
                goto done;
            }
            if (read_at(fd, header, sizeof(header), 0) != 0)
                goto done;
            segment_header(expected, segment->index);
            if (memcmp(header, expected, sizeof(header)) != 0) {
                errno = EILSEQ;
                goto done;
            }
            segment->format = QIHSE_WAL_FORMAT_VER;
            segment->valid_end = QIHSE_WAL_HEADER_SIZE;
        }
    }
    size_t header_size = segment->format == QIHSE_WAL_FORMAT_VER
                             ? QIHSE_WAL_RECORD_HEADER_SIZE
                             : QIHSE_WAL_LEGACY_RECORD_HEADER_SIZE;
    while (segment->valid_end < segment->size) {
        off_t offset = segment->valid_end;
        size_t remaining = (size_t)(segment->size - offset);
        if (remaining < header_size) {
            segment->partial = true;
            break;
        }
        uint8_t hdr[QIHSE_WAL_RECORD_HEADER_SIZE];
        if (read_at(fd, hdr, header_size, offset) != 0)
            goto done;
        qihse_wal_record_t rec;
        if (!decode_record(hdr, segment->format, &rec)) {
            errno = EILSEQ;
            goto done;
        }
        size_t payload_size = (size_t)rec.key_length + rec.value_length;
        if (payload_size > remaining - header_size) {
            segment->partial = true;
            break;
        }
        uint8_t *payload = payload_size ? malloc(payload_size) : NULL;
        if (payload_size && !payload)
            goto done;
        if (payload_size && read_at(fd, payload, payload_size, offset + (off_t)header_size) != 0) {
            free(payload);
            goto done;
        }
        const void *key = rec.key_length ? payload : NULL;
        const void *value = rec.value_length ? payload + rec.key_length : NULL;
        uint32_t checksum = segment->format == QIHSE_WAL_FORMAT_VER
                                ? payload_crc(&rec, key, value) : record_crc(&rec, key, value);
        if (checksum != rec.checksum) {
            free(payload);
            errno = EILSEQ;
            goto done;
        }
        if (rec.lsn > segment->max_lsn)
            segment->max_lsn = rec.lsn;
        if (rec.op_type == QIHSE_WAL_OP_CHECKPOINT) {
            uint64_t checkpoint;
            if (segment->format == QIHSE_WAL_FORMAT_VER)
                checkpoint = get_u64(key);
            else
                memcpy(&checkpoint, key, sizeof(checkpoint));
            if (checkpoint > segment->checkpoint_lsn)
                segment->checkpoint_lsn = checkpoint;
        }
        segment->valid_end += (off_t)(header_size + payload_size);
        if (callback && rec.lsn >= start_lsn) {
            if (*replayed == INT_MAX) {
                free(payload);
                errno = EOVERFLOW;
                goto done;
            }
            ++*replayed;
            bool more = callback(&rec, key, rec.key_length, value, rec.value_length, user_data);
            free(payload);
            if (!more) {
                result = 1;
                goto done;
            }
        } else {
            free(payload);
        }
    }
    if (segment->partial && (!allow_partial || segment->format != QIHSE_WAL_FORMAT_VER)) {
        errno = EILSEQ;
        goto done;
    }
    result = 0;
done: {
        int saved_error = errno;
        if (close(fd) != 0 && result >= 0)
            return -1;
        errno = saved_error;
        return result;
    }
}

static int write_vectors(int fd, struct iovec *iov, int count);

/* Publish only complete v2 segment headers. With durability enabled, a crash
 * before link leaves an ignored temporary file; a crash after link leaves a
 * complete empty segment. link refuses an existing destination instead of
 * replacing unvalidated data. */
static int seg_create(qihse_wal_t *wal)
{
    char temporary[PATH_MAX];
    int length = snprintf(temporary, sizeof(temporary), "%s.tmp.XXXXXX", wal->seg_path);
    if (length < 0 || (size_t)length >= sizeof(temporary)) {
        errno = ENAMETOOLONG;
        return -1;
    }
    int fd = mkostemp(temporary, O_CLOEXEC | O_APPEND);
    if (fd < 0)
        return -1;
    int result = -1;
    uint8_t header[QIHSE_WAL_HEADER_SIZE];
    segment_header(header, wal->seg_index);
    struct iovec iov = {.iov_base = header, .iov_len = sizeof(header)};
    if (write_vectors(fd, &iov, 1) != 0 ||
        (wal->durability != QIHSE_WAL_DURABILITY_NONE && sync_fd(fd) != 0))
        goto done;
    if (link(temporary, wal->seg_path) != 0)
        goto done;
    if (unlink(temporary) != 0)
        goto done;
    if (wal->durability != QIHSE_WAL_DURABILITY_NONE && sync_directory(wal->directory) != 0)
        goto done;
    wal->seg_fd = fd;
    wal->seg_offset = sizeof(header);
    result = 0;
 done: {
        int saved_error = errno;
        if (result != 0) {
            close(fd);
            (void)unlink(temporary);
        }
        errno = saved_error;
        return result;
    }
}

static int seg_open(qihse_wal_t *wal, bool create)
{
    if (seg_path(wal->seg_path, sizeof(wal->seg_path), wal->directory, wal->seg_index) != 0)
        return -1;
    if (create)
        return seg_create(wal);
    int flags = O_WRONLY | O_APPEND | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC;
    wal->seg_fd = open(wal->seg_path, flags);
    if (wal->seg_fd < 0)
        return -1;
    struct stat st;
    if (fstat(wal->seg_fd, &st) != 0)
        return -1;
    if (!S_ISREG(st.st_mode) || st.st_size < 0 || (uintmax_t)st.st_size > SIZE_MAX) {
        errno = EINVAL;
        return -1;
    }
    wal->seg_offset = (size_t)st.st_size;
    return 0;
}

static int seg_rotate(qihse_wal_t *wal)
{
    if (wal->durability != QIHSE_WAL_DURABILITY_NONE && sync_fd(wal->seg_fd) != 0)
        return -1;
    if (wal->seg_index == UINT64_MAX) {
        errno = EOVERFLOW;
        return -1;
    }
    int fd = wal->seg_fd;
    wal->seg_fd = -1;
    if (close(fd) != 0)
        return -1;
    ++wal->seg_index;
    return seg_open(wal, true);
}

qihse_wal_t *qihse_wal_create(const char *directory, size_t segment_size,
                             qihse_wal_durability_t durability)
{
    if (!directory || strlen(directory) >= PATH_MAX ||
        durability < QIHSE_WAL_DURABILITY_NONE || durability > QIHSE_WAL_DURABILITY_GROUP_COMMIT) {
        errno = EINVAL;
        return NULL;
    }
    qihse_wal_t *wal = calloc(1, sizeof(*wal));
    if (!wal)
        return NULL;
    wal->seg_fd = -1;
    int lock_error = pthread_mutex_init(&wal->lock, NULL);
    if (lock_error != 0) {
        free(wal);
        errno = lock_error;
        return NULL;
    }
    strcpy(wal->directory, directory);
    wal->segment_size = segment_size ? segment_size : QIHSE_WAL_DEFAULT_SEGMENT_SIZE;
    wal->durability = durability;
    wal->next_lsn = 1;
    struct wal_segment *segments = NULL;
    size_t count = 0;
    if (mkdir(directory, 0700) != 0 && errno != EEXIST)
        goto fail;
    if (list_segments(directory, &segments, &count) != 0)
        goto fail;
    for (size_t i = 0; i < count; ++i) {
        if (scan_segment(directory, &segments[i], i + 1 == count, 0, NULL, NULL, NULL) != 0)
            goto fail;
        if (segments[i].max_lsn == UINT64_MAX) {
            errno = EOVERFLOW;
            goto fail;
        }
        if (segments[i].max_lsn >= wal->next_lsn)
            wal->next_lsn = segments[i].max_lsn + 1;
        if (segments[i].checkpoint_lsn > wal->last_checkpoint_lsn)
            wal->last_checkpoint_lsn = segments[i].checkpoint_lsn;
    }
    if (count)
        wal->seg_index = segments[count - 1].index;
    bool create = !count || segments[count - 1].format != QIHSE_WAL_FORMAT_VER;
    if (count && create) {
        if (wal->seg_index == UINT64_MAX) {
            errno = EOVERFLOW;
            goto fail;
        }
        ++wal->seg_index;
    }
    if (seg_open(wal, create) != 0)
        goto fail;
    if (count && !create) {
        struct wal_segment *last = &segments[count - 1];
        if (wal->seg_offset != (size_t)last->size) {
            errno = EAGAIN;
            goto fail;
        }
        if (last->partial) {
            /* All preceding records/segments validated before truncation. */
            if (ftruncate(wal->seg_fd, last->valid_end) != 0 || sync_fd(wal->seg_fd) != 0)
                goto fail;
            wal->seg_offset = (size_t)last->valid_end;
        }
    }
    free(segments);
    return wal;
fail: {
        int saved_error = errno;
        free(segments);
        if (wal->seg_fd >= 0)
            close(wal->seg_fd);
        pthread_mutex_destroy(&wal->lock);
        free(wal);
        errno = saved_error;
        return NULL;
    }
}

void qihse_wal_destroy(qihse_wal_t *wal)
{
    if (!wal)
        return;
    pthread_mutex_lock(&wal->lock);
    if (wal->seg_fd >= 0) {
        /* No error channel on destruction; callers must check commit/flush. */
        if (!wal->failed && wal->durability != QIHSE_WAL_DURABILITY_NONE)
            (void)sync_fd(wal->seg_fd);
        close(wal->seg_fd);
    }
    pthread_mutex_unlock(&wal->lock);
    pthread_mutex_destroy(&wal->lock);
    free(wal);
}

static int write_vectors(int fd, struct iovec *iov, int count)
{
    while (count) {
        ssize_t n = writev(fd, iov, count);
        if (n < 0 && errno == EINTR)
            continue;
        if (n <= 0) {
            if (n == 0)
                errno = EIO;
            return -1;
        }
        size_t consumed = (size_t)n;
        while (count && consumed >= iov->iov_len) {
            consumed -= iov->iov_len;
            ++iov;
            --count;
        }
        if (count && consumed) {
            iov->iov_base = (uint8_t *)iov->iov_base + consumed;
            iov->iov_len -= consumed;
        }
    }
    return 0;
}

/* Caller holds wal->lock. Failed writes/syncs poison this handle until reopen. */
static uint64_t append_locked(qihse_wal_t *wal, uint64_t txn_id, uint8_t engine_id,
                              qihse_wal_op_t op_type, const void *key, uint32_t key_len,
                              const void *value, uint32_t value_len)
{
    if (wal->failed || wal->seg_fd < 0) {
        errno = EIO;
        return 0;
    }
    if (key_len > QIHSE_WAL_MAX_KEY || value_len > QIHSE_WAL_MAX_VALUE ||
        (key_len && !key) || (value_len && !value) ||
        op_type < QIHSE_WAL_OP_INSERT || op_type > QIHSE_WAL_OP_CHECKPOINT ||
        (op_type >= QIHSE_WAL_OP_BEGIN && op_type <= QIHSE_WAL_OP_ABORT && (key_len || value_len)) ||
        (op_type == QIHSE_WAL_OP_CHECKPOINT && (key_len != sizeof(uint64_t) || value_len))) {
        errno = EINVAL;
        return 0;
    }
    if (wal->next_lsn == UINT64_MAX) {
        errno = EOVERFLOW;
        return 0;
    }
    size_t record_size = QIHSE_WAL_RECORD_HEADER_SIZE + (size_t)key_len + value_len;
    if (wal->seg_offset > QIHSE_WAL_HEADER_SIZE &&
        (record_size > wal->segment_size || wal->seg_offset > wal->segment_size - record_size)) {
        if (seg_rotate(wal) != 0)
            goto fail;
    }
    qihse_wal_record_t rec = {
        .lsn = wal->next_lsn, .txn_id = txn_id, .engine_id = engine_id,
        .op_type = (uint8_t)op_type, .key_length = key_len, .value_length = value_len,
    };
    rec.checksum = payload_crc(&rec, key, value);
    uint8_t hdr[QIHSE_WAL_RECORD_HEADER_SIZE];
    put_u64(hdr, rec.lsn);
    put_u64(hdr + 8, rec.txn_id);
    hdr[16] = rec.engine_id;
    hdr[17] = rec.op_type;
    put_u32(hdr + 18, rec.key_length);
    put_u32(hdr + 22, rec.value_length);
    put_u32(hdr + 26, rec.checksum);
    put_u32(hdr + 30, qihse_wal_crc32(hdr, 30));
    struct iovec iov[3] = {{.iov_base = hdr, .iov_len = sizeof(hdr)}};
    int count = 1;
    if (key_len)
        iov[count++] = (struct iovec){.iov_base = (void *)key, .iov_len = key_len};
    if (value_len)
        iov[count++] = (struct iovec){.iov_base = (void *)value, .iov_len = value_len};
    if (write_vectors(wal->seg_fd, iov, count) != 0)
        goto fail;
    wal->seg_offset += record_size;
    ++wal->next_lsn;
    if (wal->durability == QIHSE_WAL_DURABILITY_FDATASYNC && sync_fd(wal->seg_fd) != 0)
        goto fail;
    return rec.lsn;
fail:
    wal->failed = true;
    return 0;
}

static uint64_t wal_append_raw(qihse_wal_t *wal, uint64_t txn_id, uint8_t engine_id,
                               qihse_wal_op_t op_type, const void *key, uint32_t key_len,
                               const void *value, uint32_t value_len)
{
    if (!wal) {
        errno = EINVAL;
        return 0;
    }
    pthread_mutex_lock(&wal->lock);
    uint64_t result = append_locked(wal, txn_id, engine_id, op_type, key, key_len, value, value_len);
    pthread_mutex_unlock(&wal->lock);
    return result;
}

uint64_t qihse_wal_append(qihse_wal_t *wal, uint64_t txn_id, uint8_t engine_id,
                          qihse_wal_op_t op_type, const void *key, uint32_t key_len,
                          const void *value, uint32_t value_len)
{
    return wal_append_raw(wal, txn_id, engine_id, op_type, key, key_len, value, value_len);
}

uint64_t qihse_wal_append_begin(qihse_wal_t *wal, uint64_t txn_id)
{
    return wal_append_raw(wal, txn_id, 0, QIHSE_WAL_OP_BEGIN, NULL, 0, NULL, 0);
}

uint64_t qihse_wal_append_commit(qihse_wal_t *wal, uint64_t txn_id)
{
    return wal_append_raw(wal, txn_id, 0, QIHSE_WAL_OP_COMMIT, NULL, 0, NULL, 0);
}

uint64_t qihse_wal_append_abort(qihse_wal_t *wal, uint64_t txn_id)
{
    return wal_append_raw(wal, txn_id, 0, QIHSE_WAL_OP_ABORT, NULL, 0, NULL, 0);
}

uint64_t qihse_wal_append_checkpoint(qihse_wal_t *wal, uint64_t lsn)
{
    uint8_t payload[8];
    put_u64(payload, lsn);
    return wal_append_raw(wal, 0, 0, QIHSE_WAL_OP_CHECKPOINT, payload, sizeof(payload), NULL, 0);
}

static int flush_locked(qihse_wal_t *wal)
{
    if (wal->failed || wal->seg_fd < 0) {
        errno = EIO;
        return -1;
    }
    if (sync_fd(wal->seg_fd) != 0) {
        wal->failed = true;
        return -1;
    }
    return 0;
}

int qihse_wal_flush(qihse_wal_t *wal)
{
    if (!wal) {
        errno = EINVAL;
        return -1;
    }
    pthread_mutex_lock(&wal->lock);
    int result = flush_locked(wal);
    pthread_mutex_unlock(&wal->lock);
    return result;
}

int qihse_wal_replay(qihse_wal_t *wal, uint64_t start_lsn,
                     qihse_wal_replay_cb callback, void *user_data)
{
    if (!wal || !callback) {
        errno = EINVAL;
        return -1;
    }
    pthread_mutex_lock(&wal->lock);
    struct wal_segment *segments = NULL;
    size_t count = 0;
    int replayed = 0, result = -1;
    if (flush_locked(wal) != 0 || list_segments(wal->directory, &segments, &count) != 0)
        goto done;
    for (size_t i = 0; i < count; ++i) {
        int scanned = scan_segment(wal->directory, &segments[i], false, start_lsn,
                                   callback, user_data, &replayed);
        if (scanned < 0)
            goto done;
        if (scanned > 0)
            break;
    }
    result = replayed;
done:
    free(segments);
    pthread_mutex_unlock(&wal->lock);
    return result;
}

int qihse_wal_checkpoint(qihse_wal_t *wal, uint64_t checkpoint_lsn)
{
    if (!wal) {
        errno = EINVAL;
        return -1;
    }
    pthread_mutex_lock(&wal->lock);
    struct wal_segment *segments = NULL;
    size_t count = 0;
    int result = -1;
    if (checkpoint_lsn > wal->next_lsn) {
        errno = EINVAL;
        goto done;
    }
    if (flush_locked(wal) != 0 || list_segments(wal->directory, &segments, &count) != 0)
        goto done;
    /* Validate the complete log before any destructive cleanup. */
    for (size_t i = 0; i < count; ++i) {
        if (scan_segment(wal->directory, &segments[i], false, 0, NULL, NULL, NULL) != 0)
            goto done;
    }
    /* Keep a durable high-water LSN even if rotation/checkpoint removes every
     * older record. v2 payload integers use explicit little-endian byte order. */
    uint8_t checkpoint_payload[8];
    put_u64(checkpoint_payload, checkpoint_lsn);
    if (!append_locked(wal, 0, 0, QIHSE_WAL_OP_CHECKPOINT,
                       checkpoint_payload, sizeof(checkpoint_payload), NULL, 0) ||
        flush_locked(wal) != 0)
        goto done;
    for (size_t i = 0; i < count; ++i) {
        if (segments[i].index == wal->seg_index || segments[i].max_lsn >= checkpoint_lsn)
            continue;
        char path[PATH_MAX];
        if (seg_path(path, sizeof(path), wal->directory, segments[i].index) != 0 || unlink(path) != 0)
            goto done;
    }
    if (sync_directory(wal->directory) != 0)
        goto done;
    wal->last_checkpoint_lsn = checkpoint_lsn;
    result = 0;
done:
    free(segments);
    pthread_mutex_unlock(&wal->lock);
    return result;
}

uint64_t qihse_wal_current_lsn(qihse_wal_t *wal)
{
    if (!wal)
        return 0;
    pthread_mutex_lock(&wal->lock);
    uint64_t lsn = wal->next_lsn;
    pthread_mutex_unlock(&wal->lock);
    return lsn;
}

uint64_t qihse_wal_last_checkpoint(qihse_wal_t *wal)
{
    if (!wal)
        return 0;
    pthread_mutex_lock(&wal->lock);
    uint64_t lsn = wal->last_checkpoint_lsn;
    pthread_mutex_unlock(&wal->lock);
    return lsn;
}
