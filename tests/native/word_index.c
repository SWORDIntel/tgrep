/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
#include <dirent.h>
#include <signal.h>
#include <sys/resource.h>
#include <sys/wait.h>

#include "support.h"
#include "../../native/word_index_mmap.c"

static void *index_with_document(uint64_t document)
{
    void *index = tgrep_word_index_create();
    assert(index);
    assert(tgrep_word_index_add(index, "alpha", 5, document) == 0);
    return index;
}

static void expect_document(void *index, uint64_t expected)
{
    uint64_t documents[2] = {0};
    assert(tgrep_word_index_search(index, "alpha", 5, documents, 2) == 1);
    assert(documents[0] == expected);
}

static void failed_save_preserves_file_and_mapping(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "failed-save.qwi");
    void *original = index_with_document(9);
    void *replacement = index_with_document(77);
    assert(tgrep_word_index_save(original, path) == 0);
    void *mapping = tgrep_word_index_load(path);
    assert(mapping);
    struct stat before_stat;
    assert(stat(path, &before_stat) == 0);
    FILE *file = fopen(path, "rb");
    assert(file);
    size_t image_size = (size_t)before_stat.st_size;
    unsigned char *before = malloc(image_size);
    assert(before && fread(before, 1, image_size, file) == image_size);
    assert(fclose(file) == 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(signal(SIGXFSZ, SIG_IGN) != SIG_ERR);
        struct rlimit limit = {.rlim_cur = 16, .rlim_max = 16};
        assert(setrlimit(RLIMIT_FSIZE, &limit) == 0);
        int result = tgrep_word_index_save(replacement, path);
        _exit(result < 0 ? 0 : 1);
    }
    int status;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    file = fopen(path, "rb");
    assert(file);
    unsigned char *after = malloc(image_size + 1);
    assert(after && fread(after, 1, image_size + 1, file) == image_size);
    assert(memcmp(before, after, image_size) == 0 && fclose(file) == 0);
    free(before);
    free(after);
    struct stat after_stat;
    assert(stat(path, &after_stat) == 0 && after_stat.st_ino == before_stat.st_ino);
    expect_document(mapping, 9);
    DIR *entries = opendir(directory);
    assert(entries);
    struct dirent *entry;
    while ((entry = readdir(entries))) {
        assert(strncmp(entry->d_name, "failed-save.qwi.", strlen("failed-save.qwi.")) != 0);
    }
    assert(closedir(entries) == 0);
    tgrep_word_index_unload(mapping);
    tgrep_word_index_destroy(replacement);
    tgrep_word_index_destroy(original);
}

static void successful_replacement_keeps_old_mapping(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "replacement.qwi");
    void *original = index_with_document(9);
    void *replacement = index_with_document(77);
    assert(tgrep_word_index_save(original, path) == 0);
    void *old_mapping = tgrep_word_index_load(path);
    assert(old_mapping);
    struct stat before, after;
    assert(stat(path, &before) == 0);
    assert(tgrep_word_index_save(replacement, path) == 0);
    assert(stat(path, &after) == 0 && before.st_ino != after.st_ino);
    void *new_mapping = tgrep_word_index_load(path);
    assert(new_mapping);
    expect_document(old_mapping, 9);
    expect_document(new_mapping, 77);
    tgrep_word_index_unload(new_mapping);
    tgrep_word_index_unload(old_mapping);
    tgrep_word_index_destroy(replacement);
    tgrep_word_index_destroy(original);
}

static void sorted_roundtrip_and_bounds(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "bounds.qwi");
    void *index = index_with_document(9);
    assert(tgrep_word_index_add(index, "alpha", 5, 2) == 0);
    assert(tgrep_word_index_add(index, "beta", 4, 3) == 0);
    assert(tgrep_word_index_save(index, path) == 0);
    void *mapping = tgrep_word_index_load(path);
    assert(mapping && tgrep_word_index_size(mapping) == 3);
    uint64_t documents[4];
    assert(tgrep_word_index_search(mapping, "alpha", 5, documents, 4) == 2);
    assert(documents[0] == 2 && documents[1] == 9);
    assert(tgrep_word_index_search(mapping, "alpha", 5, documents, 1) == 1);
    assert(tgrep_word_index_search(mapping, "absent", 6, documents, 4) == 0);
    tgrep_word_index_unload(mapping);
    FILE *file = fopen(path, "r+b");
    assert(file);
    uint64_t count = UINT64_MAX;
    assert(fseek(file, 8, SEEK_SET) == 0);
    assert(fwrite(&count, 8, 1, file) == 1 && fclose(file) == 0);
    assert(tgrep_word_index_load(path) == NULL);
    assert(tgrep_word_index_save(index, path) == 0);
    assert(truncate(path, 17) == 0);
    assert(tgrep_word_index_load(path) == NULL);
    tgrep_word_index_destroy(index);
}

static void compact_entries_and_segment_binding(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "bound.qwi");
    void *index = index_with_document(9);
    assert(tgrep_word_index_add(index, "beta", 4, UINT32_MAX) == 0);
    assert(tgrep_word_index_add(index, "overflow", 8, (uint64_t)UINT32_MAX + 1) < 0);
    assert(tgrep_word_index_save_for_segment(index, path, 41, 123) == 0);
    struct stat st;
    assert(stat(path, &st) == 0);
    assert(st.st_size == TGREP_QWI_HEADER_SIZE + TGREP_QWI_DIRECTORY_SIZE + 2 * TGREP_QWI_ENTRY_SIZE);
    assert(tgrep_word_index_load_for_segment(path, 42, 123) == NULL);
    assert(tgrep_word_index_load_for_segment(path, 41, 456) == NULL);
    assert(tgrep_word_index_load(path) == NULL);
    void *mapping = tgrep_word_index_load_for_segment(path, 41, 123);
    assert(mapping);
    expect_document(mapping, 9);
    tgrep_word_index_unload(mapping);
    tgrep_word_index_destroy(index);
}

static void flip_byte(const char *path, off_t offset)
{
    int fd = open(path, O_RDWR);
    assert(fd >= 0);
    unsigned char byte;
    assert(pread(fd, &byte, 1, offset) == 1);
    byte ^= 1;
    assert(pwrite(fd, &byte, 1, offset) == 1);
    assert(close(fd) == 0);
}

static void corrupted_directory_is_rejected(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "metadata.qwi");
    void *index = index_with_document(9);
    assert(tgrep_word_index_save(index, path) == 0);
    flip_byte(path, TGREP_QWI_HEADER_SIZE);
    assert(tgrep_word_index_load(path) == NULL);
    assert(tgrep_word_index_save(index, path) == 0);
    flip_byte(path, 16);
    assert(tgrep_word_index_load_for_segment(path, 1, 0) == NULL);
    tgrep_word_index_destroy(index);
}

static void payload_corruption_is_error_not_miss(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "payload.qwi");
    void *index = index_with_document(9);
    assert(tgrep_word_index_save(index, path) == 0);
    flip_byte(path, TGREP_QWI_HEADER_SIZE + TGREP_QWI_DIRECTORY_SIZE + 7);
    void *mapping = tgrep_word_index_load(path);
    assert(mapping);
    uint64_t documents[2];
    assert(tgrep_word_index_search(mapping, "alpha", 5, documents, 2) == SIZE_MAX);
    assert(tgrep_word_index_search(mapping, "absent", 6, documents, 2) == 0);
    tgrep_word_index_unload(mapping);
    tgrep_word_index_destroy(index);
}

static void common_hash_crosses_block_boundary(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "many.qwi");
    void *index = tgrep_word_index_create();
    assert(index);
    for (uint32_t id = 0; id < TGREP_QWI_BLOCK_ENTRIES + 1; id++)
        assert(tgrep_word_index_add(index, "alpha", 5, id) == 0);
    assert(tgrep_word_index_save(index, path) == 0);
    void *mapping = tgrep_word_index_load(path);
    assert(mapping);
    uint64_t documents[TGREP_QWI_BLOCK_ENTRIES + 2];
    assert(tgrep_word_index_search(mapping, "alpha", 5, documents,
                                   TGREP_QWI_BLOCK_ENTRIES + 2) == TGREP_QWI_BLOCK_ENTRIES + 1);
    for (uint32_t id = 0; id < TGREP_QWI_BLOCK_ENTRIES + 1; id++)
        assert(documents[id] == id);
    tgrep_word_index_unload(mapping);
    flip_byte(path, TGREP_QWI_HEADER_SIZE + 2 * TGREP_QWI_DIRECTORY_SIZE +
                   TGREP_QWI_BLOCK_ENTRIES * TGREP_QWI_ENTRY_SIZE + 7);
    mapping = tgrep_word_index_load(path);
    assert(mapping);
    assert(tgrep_word_index_search(mapping, "alpha", 5, documents,
                                   TGREP_QWI_BLOCK_ENTRIES + 2) == SIZE_MAX);
    tgrep_word_index_unload(mapping);
    tgrep_word_index_destroy(index);
}

static void legacy_and_nonregular_are_rejected(const char *directory)
{
    char path[PATH_MAX], fifo[PATH_MAX];
    test_path(path, sizeof(path), directory, "legacy.qwi");
    unsigned char header[TGREP_QWI_HEADER_SIZE] = {0};
    qwi_put32(header, TGREP_QWI_MAGIC);
    qwi_put32(header + 4, 2);
    FILE *file = fopen(path, "wb");
    assert(file && fwrite(header, 1, sizeof(header), file) == sizeof(header));
    assert(fclose(file) == 0 && tgrep_word_index_load(path) == NULL);
    test_path(fifo, sizeof(fifo), directory, "index.fifo");
    assert(mkfifo(fifo, 0600) == 0);
    assert(tgrep_word_index_load(fifo) == NULL);
}

static void empty_index_is_valid(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "empty.qwi");
    void *index = tgrep_word_index_create();
    assert(index && tgrep_word_index_save(index, path) == 0);
    void *mapping = tgrep_word_index_load(path);
    assert(mapping && tgrep_word_index_size(mapping) == 0);
    uint64_t document;
    assert(tgrep_word_index_search(mapping, "alpha", 5, &document, 1) == 0);
    tgrep_word_index_unload(mapping);
    tgrep_word_index_destroy(index);
}

static void import_remaps_sparse_ids_and_preserves_binding(const char *directory)
{
    char path[PATH_MAX], merged_path[PATH_MAX];
    test_path(path, sizeof(path), directory, "import-source.qwi");
    test_path(merged_path, sizeof(merged_path), directory, "import-merged.qwi");
    void *source = index_with_document(9);
    assert(tgrep_word_index_add(source, "alpha", 5, 2) == 0);
    assert(tgrep_word_index_add(source, "beta", 4, 9) == 0);
    assert(tgrep_word_index_save_for_segment(source, path, 11, 22) == 0);
    void *mapped = tgrep_word_index_load_for_segment(path, 11, 22);
    void *merged = tgrep_word_index_create();
    assert(mapped && merged);
    const uint32_t old_ids[] = {2, 9};
    assert(tgrep_word_index_import_block(merged, mapped, 0, old_ids, 2, 40) == 0);
    assert(tgrep_word_index_save_for_segment(merged, merged_path, 12, 33) == 0);
    assert(tgrep_word_index_load_for_segment(merged_path, 11, 22) == NULL);
    void *result = tgrep_word_index_load_for_segment(merged_path, 12, 33);
    uint64_t documents[3];
    assert(result && tgrep_word_index_search(result, "alpha", 5, documents, 3) == 2);
    assert(documents[0] == 40 && documents[1] == 41);
    assert(tgrep_word_index_search(result, "beta", 4, documents, 3) == 1 && documents[0] == 41);
    tgrep_word_index_unload(result);
    tgrep_word_index_unload(mapped);
    tgrep_word_index_destroy(merged);
    tgrep_word_index_destroy(source);
}

static void import_rejects_unknown_ids_and_overflow(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "import-invalid.qwi");
    void *source = index_with_document(9);
    assert(tgrep_word_index_save(source, path) == 0);
    void *mapped = tgrep_word_index_load(path);
    void *merged = tgrep_word_index_create();
    assert(mapped && merged);
    const uint32_t ids[] = {2, 9};
    assert(tgrep_word_index_import_block(merged, mapped, 0, ids, 1, 0) != 0);
    assert(tgrep_word_index_import_block(merged, mapped, 0, ids, 2, UINT32_MAX) != 0);
    assert(tgrep_word_index_import_block(merged, mapped, 1, ids, 2, 0) != 0);
    assert(tgrep_word_index_import_block(merged, mapped, 0, NULL, 0, 0) != 0);
    tgrep_word_index_destroy(merged);
    tgrep_word_index_unload(mapped);
    tgrep_word_index_destroy(source);
}

static void import_checks_every_block_and_document_boundary(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "import-blocks.qwi");
    void *source = tgrep_word_index_create();
    assert(source);
    uint32_t old_ids[TGREP_QWI_BLOCK_ENTRIES + 1];
    for (uint32_t id = 0; id < TGREP_QWI_BLOCK_ENTRIES + 1; id++) {
        old_ids[id] = id * 2;
        assert(tgrep_word_index_add(source, "alpha", 5, id * 2) == 0);
    }
    assert(tgrep_word_index_save(source, path) == 0);
    void *mapped = tgrep_word_index_load(path);
    void *merged = tgrep_word_index_create();
    assert(mapped && merged);
    for (size_t block = 0; block < 2; block++)
        assert(tgrep_word_index_import_block(merged, mapped, block, old_ids,
                                            TGREP_QWI_BLOCK_ENTRIES + 1, 100) == 0);
    assert(qihse_btree_size(merged) == TGREP_QWI_BLOCK_ENTRIES + 1);
    tgrep_word_index_destroy(merged);
    tgrep_word_index_unload(mapped);
    flip_byte(path, TGREP_QWI_HEADER_SIZE + 2 * TGREP_QWI_DIRECTORY_SIZE +
                   TGREP_QWI_BLOCK_ENTRIES * TGREP_QWI_ENTRY_SIZE + 11);
    mapped = tgrep_word_index_load(path);
    merged = tgrep_word_index_create();
    assert(mapped && merged);
    assert(tgrep_word_index_import_block(merged, mapped, 0, old_ids,
                                        TGREP_QWI_BLOCK_ENTRIES + 1, 100) == 0);
    assert(tgrep_word_index_import_block(merged, mapped, 1, old_ids,
                                        TGREP_QWI_BLOCK_ENTRIES + 1, 100) != 0);
    tgrep_word_index_destroy(merged);
    tgrep_word_index_unload(mapped);
    tgrep_word_index_destroy(source);
}

int main(int argc, char **argv)
{
    disable_core_dumps();
    assert(argc == 2);
    const char *directory = argv[1];
    RUN_TEST(failed_save_preserves_file_and_mapping);
    RUN_TEST(successful_replacement_keeps_old_mapping);
    RUN_TEST(sorted_roundtrip_and_bounds);
    RUN_TEST(compact_entries_and_segment_binding);
    RUN_TEST(corrupted_directory_is_rejected);
    RUN_TEST(payload_corruption_is_error_not_miss);
    RUN_TEST(common_hash_crosses_block_boundary);
    RUN_TEST(legacy_and_nonregular_are_rejected);
    RUN_TEST(empty_index_is_valid);
    RUN_TEST(import_remaps_sparse_ids_and_preserves_binding);
    RUN_TEST(import_rejects_unknown_ids_and_overflow);
    RUN_TEST(import_checks_every_block_and_document_boundary);
    return 0;
}
