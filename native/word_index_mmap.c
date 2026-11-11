/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
/**
 * QWI v3: generation-bound, checksummed word postings with 12-byte entries.
 * Header (56 bytes, LE): magic, version, count, generation, block_count,
 * metadata_crc32, entry_size, block_entries, reserved, TGS metadata fingerprint.
 * Directory (24 bytes/block): first_hash, last_hash, payload_crc32, entry_count.
 * Entries: hash64 + doc_id32, BE, sorted by (hash, doc_id).
 *
 * Checked directory bounds make negative searches independent of untouched
 * payload. Every accessed block is checked before its entries exclude files.
 * CRC detects corruption/state mixing, not a writer able to forge state.
 */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <fcntl.h>
#include <limits.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include "qihse_btree.h"

#define TGREP_QWI_MAGIC 0x54575149u
#define TGREP_QWI_VERSION 3u
#define TGREP_QWI_HEADER_SIZE 56u
#define TGREP_QWI_DIRECTORY_SIZE 24u
#define TGREP_QWI_ENTRY_SIZE 12u
#define TGREP_QWI_BLOCK_ENTRIES 512u

static uint64_t fnv1a(const void *data, size_t len)
{
    const unsigned char *p = data;
    uint64_t h = 1469598103934665603ULL;
    for (size_t i = 0; i < len; i++) {
        h ^= p[i];
        h *= 1099511628211ULL;
    }
    return h;
}

static uint32_t qwi_get32(const unsigned char *p)
{
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) |
           ((uint32_t)p[3] << 24);
}

static uint64_t qwi_get64(const unsigned char *p)
{
    return (uint64_t)qwi_get32(p) | ((uint64_t)qwi_get32(p + 4) << 32);
}

static void qwi_put32(unsigned char *p, uint32_t value)
{
    for (unsigned i = 0; i < 4; i++)
        p[i] = (unsigned char)(value >> (8 * i));
}

static void qwi_put64(unsigned char *p, uint64_t value)
{
    qwi_put32(p, (uint32_t)value);
    qwi_put32(p + 4, (uint32_t)(value >> 32));
}

static uint64_t entry_hash(const unsigned char *p)
{
    uint64_t value = 0;
    for (unsigned i = 0; i < 8; i++)
        value = (value << 8) | p[i];
    return value;
}

static uint32_t entry_doc(const unsigned char *p)
{
    return ((uint32_t)p[8] << 24) | ((uint32_t)p[9] << 16) |
           ((uint32_t)p[10] << 8) | p[11];
}

static const uint32_t qwi_crc_table[256] = {
    0x00000000u, 0x77073096u, 0xee0e612cu, 0x990951bau, 0x076dc419u, 0x706af48fu, 0xe963a535u, 0x9e6495a3u,
    0x0edb8832u, 0x79dcb8a4u, 0xe0d5e91eu, 0x97d2d988u, 0x09b64c2bu, 0x7eb17cbdu, 0xe7b82d07u, 0x90bf1d91u,
    0x1db71064u, 0x6ab020f2u, 0xf3b97148u, 0x84be41deu, 0x1adad47du, 0x6ddde4ebu, 0xf4d4b551u, 0x83d385c7u,
    0x136c9856u, 0x646ba8c0u, 0xfd62f97au, 0x8a65c9ecu, 0x14015c4fu, 0x63066cd9u, 0xfa0f3d63u, 0x8d080df5u,
    0x3b6e20c8u, 0x4c69105eu, 0xd56041e4u, 0xa2677172u, 0x3c03e4d1u, 0x4b04d447u, 0xd20d85fdu, 0xa50ab56bu,
    0x35b5a8fau, 0x42b2986cu, 0xdbbbc9d6u, 0xacbcf940u, 0x32d86ce3u, 0x45df5c75u, 0xdcd60dcfu, 0xabd13d59u,
    0x26d930acu, 0x51de003au, 0xc8d75180u, 0xbfd06116u, 0x21b4f4b5u, 0x56b3c423u, 0xcfba9599u, 0xb8bda50fu,
    0x2802b89eu, 0x5f058808u, 0xc60cd9b2u, 0xb10be924u, 0x2f6f7c87u, 0x58684c11u, 0xc1611dabu, 0xb6662d3du,
    0x76dc4190u, 0x01db7106u, 0x98d220bcu, 0xefd5102au, 0x71b18589u, 0x06b6b51fu, 0x9fbfe4a5u, 0xe8b8d433u,
    0x7807c9a2u, 0x0f00f934u, 0x9609a88eu, 0xe10e9818u, 0x7f6a0dbbu, 0x086d3d2du, 0x91646c97u, 0xe6635c01u,
    0x6b6b51f4u, 0x1c6c6162u, 0x856530d8u, 0xf262004eu, 0x6c0695edu, 0x1b01a57bu, 0x8208f4c1u, 0xf50fc457u,
    0x65b0d9c6u, 0x12b7e950u, 0x8bbeb8eau, 0xfcb9887cu, 0x62dd1ddfu, 0x15da2d49u, 0x8cd37cf3u, 0xfbd44c65u,
    0x4db26158u, 0x3ab551ceu, 0xa3bc0074u, 0xd4bb30e2u, 0x4adfa541u, 0x3dd895d7u, 0xa4d1c46du, 0xd3d6f4fbu,
    0x4369e96au, 0x346ed9fcu, 0xad678846u, 0xda60b8d0u, 0x44042d73u, 0x33031de5u, 0xaa0a4c5fu, 0xdd0d7cc9u,
    0x5005713cu, 0x270241aau, 0xbe0b1010u, 0xc90c2086u, 0x5768b525u, 0x206f85b3u, 0xb966d409u, 0xce61e49fu,
    0x5edef90eu, 0x29d9c998u, 0xb0d09822u, 0xc7d7a8b4u, 0x59b33d17u, 0x2eb40d81u, 0xb7bd5c3bu, 0xc0ba6cadu,
    0xedb88320u, 0x9abfb3b6u, 0x03b6e20cu, 0x74b1d29au, 0xead54739u, 0x9dd277afu, 0x04db2615u, 0x73dc1683u,
    0xe3630b12u, 0x94643b84u, 0x0d6d6a3eu, 0x7a6a5aa8u, 0xe40ecf0bu, 0x9309ff9du, 0x0a00ae27u, 0x7d079eb1u,
    0xf00f9344u, 0x8708a3d2u, 0x1e01f268u, 0x6906c2feu, 0xf762575du, 0x806567cbu, 0x196c3671u, 0x6e6b06e7u,
    0xfed41b76u, 0x89d32be0u, 0x10da7a5au, 0x67dd4accu, 0xf9b9df6fu, 0x8ebeeff9u, 0x17b7be43u, 0x60b08ed5u,
    0xd6d6a3e8u, 0xa1d1937eu, 0x38d8c2c4u, 0x4fdff252u, 0xd1bb67f1u, 0xa6bc5767u, 0x3fb506ddu, 0x48b2364bu,
    0xd80d2bdau, 0xaf0a1b4cu, 0x36034af6u, 0x41047a60u, 0xdf60efc3u, 0xa867df55u, 0x316e8eefu, 0x4669be79u,
    0xcb61b38cu, 0xbc66831au, 0x256fd2a0u, 0x5268e236u, 0xcc0c7795u, 0xbb0b4703u, 0x220216b9u, 0x5505262fu,
    0xc5ba3bbeu, 0xb2bd0b28u, 0x2bb45a92u, 0x5cb36a04u, 0xc2d7ffa7u, 0xb5d0cf31u, 0x2cd99e8bu, 0x5bdeae1du,
    0x9b64c2b0u, 0xec63f226u, 0x756aa39cu, 0x026d930au, 0x9c0906a9u, 0xeb0e363fu, 0x72076785u, 0x05005713u,
    0x95bf4a82u, 0xe2b87a14u, 0x7bb12baeu, 0x0cb61b38u, 0x92d28e9bu, 0xe5d5be0du, 0x7cdcefb7u, 0x0bdbdf21u,
    0x86d3d2d4u, 0xf1d4e242u, 0x68ddb3f8u, 0x1fda836eu, 0x81be16cdu, 0xf6b9265bu, 0x6fb077e1u, 0x18b74777u,
    0x88085ae6u, 0xff0f6a70u, 0x66063bcau, 0x11010b5cu, 0x8f659effu, 0xf862ae69u, 0x616bffd3u, 0x166ccf45u,
    0xa00ae278u, 0xd70dd2eeu, 0x4e048354u, 0x3903b3c2u, 0xa7672661u, 0xd06016f7u, 0x4969474du, 0x3e6e77dbu,
    0xaed16a4au, 0xd9d65adcu, 0x40df0b66u, 0x37d83bf0u, 0xa9bcae53u, 0xdebb9ec5u, 0x47b2cf7fu, 0x30b5ffe9u,
    0xbdbdf21cu, 0xcabac28au, 0x53b39330u, 0x24b4a3a6u, 0xbad03605u, 0xcdd70693u, 0x54de5729u, 0x23d967bfu,
    0xb3667a2eu, 0xc4614ab8u, 0x5d681b02u, 0x2a6f2b94u, 0xb40bbe37u, 0xc30c8ea1u, 0x5a05df1bu, 0x2d02ef8du,
};

static uint32_t qwi_crc_update(uint32_t crc, const unsigned char *data, size_t size)
{
    for (size_t i = 0; i < size; i++)
        crc = (crc >> 8) ^ qwi_crc_table[(crc ^ data[i]) & 0xffu];
    return crc;
}

static uint32_t qwi_crc32(const unsigned char *data, size_t size)
{
    return ~qwi_crc_update(UINT32_MAX, data, size);
}

static uint32_t qwi_metadata_crc(const unsigned char *header,
                                 const unsigned char *directory, size_t size)
{
    unsigned char copy[TGREP_QWI_HEADER_SIZE];
    memcpy(copy, header, sizeof(copy));
    memset(copy + 32, 0, 4);
    uint32_t crc = qwi_crc_update(UINT32_MAX, copy, sizeof(copy));
    return ~qwi_crc_update(crc, directory, size);
}

typedef struct {
    void *mmap_base;
    size_t mmap_size;
    const unsigned char *directory;
    const unsigned char *entries;
    size_t count;
    size_t blocks;
} qwi_mmap_t;

void *tgrep_word_index_create(void)
{
    return qihse_btree_create(0);
}

void tgrep_word_index_destroy(void *handle)
{
    if (handle)
        qihse_btree_destroy(handle);
}

static int qwi_add_hash(void *handle, uint64_t hash, uint32_t doc_id)
{
    unsigned char key[16];
    uint64_t document = doc_id;
    for (unsigned i = 0; i < 8; i++) {
        key[i] = (unsigned char)(hash >> (56 - 8 * i));
        key[8 + i] = (unsigned char)(document >> (56 - 8 * i));
    }
    return qihse_btree_insert(handle, key, sizeof(key), doc_id) ? 0 : -1;
}

int tgrep_word_index_add(void *handle, const char *token, size_t len, uint64_t doc_id)
{
    if (!handle || !token || !len || doc_id > UINT32_MAX)
        return -1;
    return qwi_add_hash(handle, fnv1a(token, len), (uint32_t)doc_id);
}

int tgrep_qihse_dir_sync(const char *path);

static int qwi_write_block(FILE *file, const unsigned char *entries, size_t count,
                           unsigned char *directory)
{
    size_t size = count * TGREP_QWI_ENTRY_SIZE;
    qwi_put64(directory, entry_hash(entries));
    qwi_put64(directory + 8, entry_hash(entries + (count - 1) * TGREP_QWI_ENTRY_SIZE));
    qwi_put32(directory + 16, qwi_crc32(entries, size));
    qwi_put32(directory + 20, (uint32_t)count);
    return fwrite(entries, 1, size, file) == size ? 0 : -1;
}

int tgrep_word_index_save_for_segment(void *handle, const char *path, uint64_t generation,
                                      uint64_t segment_fingerprint)
{
    if (!handle || !path)
        return -1;
    size_t count = qihse_btree_size(handle);
    size_t blocks = count / TGREP_QWI_BLOCK_ENTRIES + (count % TGREP_QWI_BLOCK_ENTRIES != 0);
    if (blocks > (SIZE_MAX - TGREP_QWI_HEADER_SIZE) / TGREP_QWI_DIRECTORY_SIZE)
        return -1;
    size_t directory_size = blocks * TGREP_QWI_DIRECTORY_SIZE;
    size_t data_offset = TGREP_QWI_HEADER_SIZE + directory_size;
    if (count > (SIZE_MAX - data_offset) / TGREP_QWI_ENTRY_SIZE ||
        data_offset > INT64_MAX || count > (INT64_MAX - data_offset) / TGREP_QWI_ENTRY_SIZE ||
        strlen(path) > SIZE_MAX - 12)
        return -1;
    unsigned char *directory = calloc(directory_size ? directory_size : 1, 1);
    char *staged = malloc(strlen(path) + 12);
    char *parent = strdup(path);
    if (!directory || !staged || !parent) {
        free(directory);
        free(staged);
        free(parent);
        return -1;
    }
    snprintf(staged, strlen(path) + 12, "%s.tmp.XXXXXX", path);
    int fd = mkstemp(staged);
    if (fd < 0) {
        free(directory);
        free(staged);
        free(parent);
        return -1;
    }
    FILE *file = fdopen(fd, "wb");
    if (!file) {
        close(fd);
        unlink(staged);
        free(directory);
        free(staged);
        free(parent);
        return -1;
    }
    qihse_btree_cursor_t *cursor = NULL;
    int result = -1;
    if (fseeko(file, (off_t)data_offset, SEEK_SET) != 0)
        goto done;
    if (count) {
        cursor = qihse_btree_range_open(handle, NULL, 0, NULL, 0);
        if (!cursor)
            goto done;
    }
    unsigned char payload[TGREP_QWI_BLOCK_ENTRIES * TGREP_QWI_ENTRY_SIZE];
    size_t written = 0, in_block = 0, block = 0;
    const void *key;
    size_t key_len;
    uint64_t row_id;
    while (cursor && qihse_btree_cursor_get(cursor, &key, &key_len, &row_id)) {
        if (key_len != 16 || row_id > UINT32_MAX || entry_hash((const unsigned char *)key + 8) != row_id)
            goto done;
        if (written == count)
            goto done;
        unsigned char *entry = payload + in_block * TGREP_QWI_ENTRY_SIZE;
        memcpy(entry, key, 8);
        memcpy(entry + 8, (const unsigned char *)key + 12, 4);
        ++written;
        ++in_block;
        if (in_block == TGREP_QWI_BLOCK_ENTRIES) {
            if (qwi_write_block(file, payload, in_block,
                                directory + block * TGREP_QWI_DIRECTORY_SIZE) != 0)
                goto done;
            ++block;
            in_block = 0;
        }
        if (!qihse_btree_cursor_next(cursor))
            break;
    }
    if (written != count)
        goto done;
    if (in_block) {
        if (qwi_write_block(file, payload, in_block,
                            directory + block * TGREP_QWI_DIRECTORY_SIZE) != 0)
            goto done;
        ++block;
    }
    if (block != blocks)
        goto done;
    unsigned char header[TGREP_QWI_HEADER_SIZE] = {0};
    qwi_put32(header, TGREP_QWI_MAGIC);
    qwi_put32(header + 4, TGREP_QWI_VERSION);
    qwi_put64(header + 8, count);
    qwi_put64(header + 16, generation);
    qwi_put64(header + 24, blocks);
    qwi_put32(header + 36, TGREP_QWI_ENTRY_SIZE);
    qwi_put32(header + 40, TGREP_QWI_BLOCK_ENTRIES);
    qwi_put64(header + 48, segment_fingerprint);
    qwi_put32(header + 32, qwi_metadata_crc(header, directory, directory_size));
    if (fseeko(file, 0, SEEK_SET) != 0 || fwrite(header, 1, sizeof(header), file) != sizeof(header) ||
        fwrite(directory, 1, directory_size, file) != directory_size ||
        fflush(file) != 0 || fsync(fileno(file)) != 0)
        goto done;
    if (fclose(file) != 0) {
        file = NULL;
        goto done;
    }
    file = NULL;
    if (rename(staged, path) != 0)
        goto done;
    char *slash = strrchr(parent, '/');
    if (!slash) {
        result = tgrep_qihse_dir_sync(".");
    } else {
        if (slash == parent)
            slash[1] = '\0';
        else
            *slash = '\0';
        result = tgrep_qihse_dir_sync(parent);
    }
done:
    if (cursor)
        qihse_btree_cursor_close(cursor);
    if (file)
        fclose(file);
    if (result != 0)
        unlink(staged);
    free(directory);
    free(staged);
    free(parent);
    return result;
}

int tgrep_word_index_save(void *handle, const char *path)
{
    return tgrep_word_index_save_for_segment(handle, path, 0, 0);
}

void *tgrep_word_index_load_for_segment(const char *path, uint64_t generation,
                                        uint64_t segment_fingerprint)
{
    if (!path)
        return NULL;
    int fd = open(path, O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0)
        return NULL;
    struct stat st, after;
    if (fstat(fd, &st) != 0 || !S_ISREG(st.st_mode) || st.st_size < TGREP_QWI_HEADER_SIZE ||
        (uintmax_t)st.st_size > SIZE_MAX) {
        close(fd);
        return NULL;
    }
    size_t size = (size_t)st.st_size;
    unsigned char *base = mmap(NULL, size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (base == MAP_FAILED) {
        close(fd);
        return NULL;
    }
    uint64_t count = qwi_get64(base + 8), blocks = qwi_get64(base + 24);
    if (qwi_get32(base) != TGREP_QWI_MAGIC || qwi_get32(base + 4) != TGREP_QWI_VERSION ||
        qwi_get64(base + 16) != generation || qwi_get64(base + 48) != segment_fingerprint ||
        qwi_get32(base + 36) != TGREP_QWI_ENTRY_SIZE ||
        qwi_get32(base + 40) != TGREP_QWI_BLOCK_ENTRIES || qwi_get32(base + 44) != 0 ||
        blocks != count / TGREP_QWI_BLOCK_ENTRIES + (count % TGREP_QWI_BLOCK_ENTRIES != 0) ||
        blocks > (size - TGREP_QWI_HEADER_SIZE) / TGREP_QWI_DIRECTORY_SIZE)
        goto invalid;
    size_t directory_size = (size_t)blocks * TGREP_QWI_DIRECTORY_SIZE;
    size_t data_offset = TGREP_QWI_HEADER_SIZE + directory_size;
    if ((size - data_offset) % TGREP_QWI_ENTRY_SIZE ||
        count != (size - data_offset) / TGREP_QWI_ENTRY_SIZE ||
        qwi_get32(base + 32) != qwi_metadata_crc(base, base + TGREP_QWI_HEADER_SIZE, directory_size))
        goto invalid;
    uint64_t previous_high = 0;
    for (size_t i = 0; i < (size_t)blocks; i++) {
        const unsigned char *d = base + TGREP_QWI_HEADER_SIZE + i * TGREP_QWI_DIRECTORY_SIZE;
        uint64_t low = qwi_get64(d), high = qwi_get64(d + 8);
        uint64_t remaining = count - i * TGREP_QWI_BLOCK_ENTRIES;
        uint32_t expected = remaining < TGREP_QWI_BLOCK_ENTRIES ? (uint32_t)remaining : TGREP_QWI_BLOCK_ENTRIES;
        if (low > high || (i && previous_high > low) || qwi_get32(d + 20) != expected)
            goto invalid;
        previous_high = high;
    }
    if (fstat(fd, &after) != 0 || st.st_dev != after.st_dev || st.st_ino != after.st_ino ||
        st.st_size != after.st_size || st.st_mtim.tv_sec != after.st_mtim.tv_sec ||
        st.st_mtim.tv_nsec != after.st_mtim.tv_nsec || st.st_ctim.tv_sec != after.st_ctim.tv_sec ||
        st.st_ctim.tv_nsec != after.st_ctim.tv_nsec)
        goto invalid;
    qwi_mmap_t *index = calloc(1, sizeof(*index));
    if (!index)
        goto invalid;
    index->mmap_base = base;
    index->mmap_size = size;
    index->directory = base + TGREP_QWI_HEADER_SIZE;
    index->entries = base + data_offset;
    index->count = (size_t)count;
    index->blocks = (size_t)blocks;
    close(fd);
    (void)posix_madvise(base, size, POSIX_MADV_RANDOM);
    return index;
invalid:
    munmap(base, size);
    close(fd);
    return NULL;
}

void *tgrep_word_index_load(const char *path)
{
    return tgrep_word_index_load_for_segment(path, 0, 0);
}

void tgrep_word_index_unload(void *handle)
{
    qwi_mmap_t *index = handle;
    if (index) {
        munmap(index->mmap_base, index->mmap_size);
        free(index);
    }
}

static int qwi_validate_block(const qwi_mmap_t *index, size_t block)
{
    const unsigned char *d = index->directory + block * TGREP_QWI_DIRECTORY_SIZE;
    const unsigned char *entries = index->entries + block * TGREP_QWI_BLOCK_ENTRIES * TGREP_QWI_ENTRY_SIZE;
    size_t count = qwi_get32(d + 20);
    if (!count || count > TGREP_QWI_BLOCK_ENTRIES ||
        qwi_crc32(entries, count * TGREP_QWI_ENTRY_SIZE) != qwi_get32(d + 16) ||
        entry_hash(entries) != qwi_get64(d) ||
        entry_hash(entries + (count - 1) * TGREP_QWI_ENTRY_SIZE) != qwi_get64(d + 8))
        return -1;
    for (size_t i = 1; i < count; i++) {
        const unsigned char *previous = entries + (i - 1) * TGREP_QWI_ENTRY_SIZE;
        const unsigned char *current = entries + i * TGREP_QWI_ENTRY_SIZE;
        uint64_t a = entry_hash(previous), b = entry_hash(current);
        if (a > b || (a == b && entry_doc(previous) >= entry_doc(current)))
            return -1;
    }
    return 0;
}

/* Import one checked block while assigning dense IDs in document-table order.
 * old_ids must be strictly increasing; the Rust owner checks that once before
 * starting the import. An unknown ID is corruption, never a dropped posting.
 * A failed import may have partially filled the unpublished builder; callers
 * must discard it rather than save it. A NULL builder validates without
 * retaining entries. No source files or tokenization needed.
 */
int tgrep_word_index_import_block(void *builder, const void *mapped, size_t block,
                                 const uint32_t *old_ids, size_t count,
                                 uint32_t first_document)
{
    const qwi_mmap_t *index = mapped;
    if (!index || (!old_ids && count) || block >= index->blocks ||
        count > (uint64_t)UINT32_MAX + 1 - first_document ||
        qwi_validate_block(index, block) != 0)
        return -1;
    const unsigned char *entries = index->entries + block * TGREP_QWI_BLOCK_ENTRIES * TGREP_QWI_ENTRY_SIZE;
    size_t entries_count = qwi_get32(index->directory + block * TGREP_QWI_DIRECTORY_SIZE + 20);
    if (block) {
        const unsigned char *previous = entries - TGREP_QWI_ENTRY_SIZE;
        uint64_t previous_hash = entry_hash(previous), first_hash = entry_hash(entries);
        if (previous_hash > first_hash ||
            (previous_hash == first_hash && entry_doc(previous) >= entry_doc(entries)))
            return -1;
    }
    for (size_t entry = 0; entry < entries_count; entry++) {
        const unsigned char *value = entries + entry * TGREP_QWI_ENTRY_SIZE;
        uint32_t document = entry_doc(value);
        size_t lo = 0, hi = count;
        while (lo < hi) {
            size_t mid = lo + (hi - lo) / 2;
            if (old_ids[mid] < document)
                lo = mid + 1;
            else
                hi = mid;
        }
        if (lo == count || old_ids[lo] != document ||
            (builder && qwi_add_hash(builder, entry_hash(value), first_document + (uint32_t)lo) != 0))
            return -1;
    }
    return 0;
}

/* SIZE_MAX means an integrity error, never an absent token. */
size_t tgrep_word_index_search(void *handle, const char *token, size_t len,
                                uint64_t *out, size_t max_results)
{
    if (!handle || !token || (max_results && !out))
        return SIZE_MAX;
    if (!len || !max_results)
        return 0;
    const qwi_mmap_t *index = handle;
    uint64_t hash = fnv1a(token, len);
    size_t low = 0, high = index->blocks;
    while (low < high) {
        size_t mid = low + (high - low) / 2;
        if (qwi_get64(index->directory + mid * TGREP_QWI_DIRECTORY_SIZE + 8) < hash)
            low = mid + 1;
        else
            high = mid;
    }
    size_t found = 0;
    for (size_t block = low; block < index->blocks; block++) {
        const unsigned char *d = index->directory + block * TGREP_QWI_DIRECTORY_SIZE;
        if (qwi_get64(d) > hash)
            break;
        if (qwi_validate_block(index, block) != 0)
            return SIZE_MAX;
        const unsigned char *entries = index->entries + block * TGREP_QWI_BLOCK_ENTRIES * TGREP_QWI_ENTRY_SIZE;
        size_t count = qwi_get32(d + 20);
        for (size_t i = 0; i < count; i++) {
            const unsigned char *entry = entries + i * TGREP_QWI_ENTRY_SIZE;
            uint64_t value = entry_hash(entry);
            if (value > hash)
                break;
            if (value == hash) {
                out[found++] = entry_doc(entry);
                if (found == max_results)
                    return found;
            }
        }
    }
    return found;
}

size_t tgrep_word_index_size(const void *handle)
{
    return handle ? ((const qwi_mmap_t *)handle)->count : 0;
}
