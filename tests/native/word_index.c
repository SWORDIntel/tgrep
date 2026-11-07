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
    unsigned char before[40];
    assert(fread(before, 1, sizeof(before), file) == sizeof(before));
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
    unsigned char after[41];
    assert(fread(after, 1, sizeof(after), file) == sizeof(before));
    assert(memcmp(before, after, sizeof(before)) == 0 && fclose(file) == 0);
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

int main(int argc, char **argv)
{
    disable_core_dumps();
    assert(argc == 2);
    const char *directory = argv[1];
    RUN_TEST(failed_save_preserves_file_and_mapping);
    RUN_TEST(successful_replacement_keeps_old_mapping);
    RUN_TEST(sorted_roundtrip_and_bounds);
    return 0;
}
