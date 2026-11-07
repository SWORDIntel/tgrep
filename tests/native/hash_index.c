/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
#include <signal.h>
#include <sys/wait.h>

#include "dsmil_hash_indexer.h"
#include "support.h"

static dsmil_hash_index_t *sample_index(void)
{
    dsmil_hash_index_t *index = dsmil_hash_index_create(1);
    assert(index);
    char original[] = "sample";
    assert(dsmil_hash_index_add(index, original, 6, 17) == 0);
    original[0] = 'z';
    assert(dsmil_hash_index_add(index, "sample", 6, 23) == 0);
    assert(dsmil_hash_index_finalize(index) == 0);
    return index;
}

static void retained_tokens_and_bounded_results(const char *directory)
{
    (void)directory;
    dsmil_hash_index_t *index = sample_index();
    uint64_t offsets[2] = {0, 99};
    size_t count = 0;
    assert(dsmil_hash_index_search_all(index, "sample", offsets, 1, &count) == 0);
    assert(count == 2 && offsets[0] == 17 && offsets[1] == 99);
    assert(dsmil_hash_index_search_all(index, "sample", NULL, 0, &count) == 0);
    assert(count == 2);
    assert(dsmil_hash_index_search_all(index, "absent", offsets, 2, &count) == KEYSTONE_NOT_FOUND);
    assert(count == 0);
    dsmil_hash_index_destroy(index);
}

static void persisted_duplicate_offsets_roundtrip(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "roundtrip.thi");
    dsmil_hash_index_t *index = sample_index();
    assert(dsmil_hash_index_save(index, path) == 0);
    dsmil_hash_index_t *loaded = dsmil_hash_index_load(path);
    assert(loaded);
    uint64_t offsets[2];
    size_t count = 0;
    assert(dsmil_hash_index_search_all(loaded, "sample", offsets, 2, &count) == 0);
    assert(count == 2 && offsets[0] == 17 && offsets[1] == 23);
    dsmil_hash_index_destroy(loaded);
    dsmil_hash_index_destroy(index);
}

static void empty_index_roundtrip(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "empty.thi");
    dsmil_hash_index_t *index = dsmil_hash_index_create(1);
    assert(index);
    assert(dsmil_hash_index_finalize(index) == 0);
    assert(dsmil_hash_index_save(index, path) == 0);
    dsmil_hash_index_t *loaded = dsmil_hash_index_load(path);
    assert(loaded && loaded->count == 0);
    dsmil_hash_index_destroy(loaded);
    dsmil_hash_index_destroy(index);
}

static void zero_length_token_roundtrip(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "empty-token.thi");
    dsmil_hash_index_t *index = dsmil_hash_index_create(1);
    assert(index);
    assert(dsmil_hash_index_add(index, "", 0, 31) == 0);
    assert(dsmil_hash_index_finalize(index) == 0);
    assert(dsmil_hash_index_save(index, path) == 0);
    dsmil_hash_index_t *loaded = dsmil_hash_index_load(path);
    assert(loaded);
    uint64_t offset = 0;
    size_t count = 0;
    assert(dsmil_hash_index_search_all(loaded, "", &offset, 1, &count) == 0);
    assert(count == 1 && offset == 31);
    dsmil_hash_index_destroy(loaded);
    dsmil_hash_index_destroy(index);
}

static void write_failure_is_not_reported_as_success(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "write-error.thi");
    dsmil_hash_index_t *index = sample_index();
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(signal(SIGXFSZ, SIG_IGN) != SIG_ERR);
        struct rlimit limit = {.rlim_cur = 16, .rlim_max = 16};
        assert(setrlimit(RLIMIT_FSIZE, &limit) == 0);
        int result = dsmil_hash_index_save(index, path);
        _exit(result < 0 ? 0 : 1);
    }
    int status;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    dsmil_hash_index_destroy(index);
}

int main(int argc, char **argv)
{
    disable_core_dumps();
    assert(argc == 2);
    const char *directory = argv[1];
    RUN_TEST(retained_tokens_and_bounded_results);
    RUN_TEST(persisted_duplicate_offsets_roundtrip);
    RUN_TEST(empty_index_roundtrip);
    RUN_TEST(zero_length_token_roundtrip);
    RUN_TEST(write_failure_is_not_reported_as_success);
    return 0;
}
