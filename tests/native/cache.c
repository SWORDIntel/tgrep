/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
#include <errno.h>

#include "qihse_table_store.h"
#include "support.h"

static int strdup_budget = -1;
static int insert_budget = -1;
static int fwrite_budget = -1;

static char *fault_strdup(const char *text)
{
    if (strdup_budget == 0)
        return NULL;
    if (strdup_budget > 0)
        --strdup_budget;
    return strdup(text);
}

static int64_t fault_insert(qihse_table_t *table, const qihse_col_value_t *values, size_t count)
{
    if (insert_budget == 0)
        return -1;
    if (insert_budget > 0)
        --insert_budget;
    return qihse_table_insert(table, values, count);
}

static size_t fault_fwrite(const void *data, size_t size, size_t count, FILE *file)
{
    if (fwrite_budget == 0) {
        errno = EIO;
        return 0;
    }
    if (fwrite_budget > 0)
        --fwrite_budget;
    return fwrite(data, size, count, file);
}

/* Inject only this wrapper's boundary calls; the table implementation is linked normally. */
#define strdup             fault_strdup
#define qihse_table_insert fault_insert
#define fwrite             fault_fwrite
#include "../../native/qihse_cache_wrapper.c"
#undef fwrite
#undef qihse_table_insert
#undef strdup

static const char *paths[] = {"/synthetic/first", "/synthetic/second"};

static tgrep_cache_t *new_cache(void)
{
    tgrep_cache_t *cache = tgrep_cache_create();
    assert(cache);
    return cache;
}

static void expect_hit_count(tgrep_cache_t *cache, const char *pattern, int flags,
                             int64_t generation, size_t expected)
{
    char **results = NULL;
    size_t count = 0;
    assert(tgrep_cache_lookup(cache, pattern, flags, generation, &results, &count) ==
           (int)expected);
    assert(count == expected);
    tgrep_cache_free_paths(results, count);
}

static void partial_insert_is_a_miss(const char *directory)
{
    (void)directory;
    tgrep_cache_t *cache = new_cache();
    insert_budget = 1;
    assert(tgrep_cache_store_results(cache, "key", 0, 1, paths, 2) == -1);
    insert_budget = -1;
    expect_hit_count(cache, "key", 0, 1, 0);
    tgrep_cache_destroy(cache);
}

static void replacement_does_not_append(const char *directory)
{
    (void)directory;
    tgrep_cache_t *cache = new_cache();
    assert(tgrep_cache_store_results(cache, "key", 0, 1, paths, 2) == 2);
    assert(tgrep_cache_store_results(cache, "key", 0, 1, paths, 1) == 1);
    expect_hit_count(cache, "key", 0, 1, 1);
    tgrep_cache_destroy(cache);
}

static void invalidation_matches_pattern_and_flags(const char *directory)
{
    (void)directory;
    tgrep_cache_t *cache = new_cache();
    assert(tgrep_cache_store_results(cache, "key", 0, 1, paths, 1) == 1);
    assert(tgrep_cache_store_results(cache, "key", 0, 2, paths, 1) == 1);
    assert(tgrep_cache_store_results(cache, "key", 1, 1, paths, 1) == 1);
    assert(tgrep_cache_invalidate(cache, "key", 0) == 0);
    expect_hit_count(cache, "key", 0, 1, 0);
    expect_hit_count(cache, "key", 0, 2, 0);
    expect_hit_count(cache, "key", 1, 1, 1);
    tgrep_cache_destroy(cache);
}

static void lookup_oom_does_not_publish_a_prefix(const char *directory)
{
    (void)directory;
    tgrep_cache_t *cache = new_cache();
    assert(tgrep_cache_store_results(cache, "key", 0, 1, paths, 2) == 2);
    char **results = NULL;
    size_t count = 0;
    strdup_budget = 1;
    assert(tgrep_cache_lookup(cache, "key", 0, 1, &results, &count) == -1);
    strdup_budget = -1;
    assert(!results && count == 0);
    expect_hit_count(cache, "key", 0, 1, 2);
    tgrep_cache_destroy(cache);
}

static void enumeration_oom_is_reported(const char *directory)
{
    (void)directory;
    tgrep_cache_t *cache = new_cache();
    assert(tgrep_cache_store_results(cache, "first", 0, 1, paths, 1) == 1);
    assert(tgrep_cache_store_results(cache, "second", 0, 1, paths, 1) == 1);
    tgrep_cache_entry_info_t entries[4] = {0};
    size_t count = 0;
    strdup_budget = 1;
    assert(tgrep_cache_get_entries(cache, entries, 4, &count) == -1);
    strdup_budget = -1;
    assert(count == 0);
    tgrep_cache_destroy(cache);
}

static void failed_record_write_is_reported(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "write-error.qsc");
    tgrep_cache_t *cache = new_cache();
    assert(tgrep_cache_store_results(cache, "key", 0, 1, paths, 2) == 2);
    fwrite_budget = 3; /* Permit the three header fields, then fail the first row. */
    assert(tgrep_cache_save(cache, path) == -1);
    fwrite_budget = -1;
    tgrep_cache_destroy(cache);
}

static void truncated_load_preserves_existing_cache(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "truncated.qsc");
    tgrep_cache_t *source = new_cache();
    assert(tgrep_cache_store_results(source, "key", 0, 1, paths, 2) == 2);
    assert(tgrep_cache_save(source, path) == 2);
    off_t first_row_end = 8 + sizeof(size_t) + 4 + strlen("key") + 4 + 8 + 4 + strlen(paths[0]) + 8;
    assert(truncate(path, first_row_end) == 0);
    tgrep_cache_t *target = new_cache();
    assert(tgrep_cache_store_results(target, "existing", 0, 1, paths, 1) == 1);
    assert(tgrep_cache_load_file(target, path) == -1);
    assert(tgrep_cache_count(target) == 1);
    expect_hit_count(target, "key", 0, 1, 0);
    expect_hit_count(target, "existing", 0, 1, 1);
    tgrep_cache_destroy(target);
    tgrep_cache_destroy(source);
}

static void valid_load_merges_complete_rows(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "roundtrip.qsc");
    tgrep_cache_t *source = new_cache();
    assert(tgrep_cache_store_results(source, "key", 0, 1, paths, 2) == 2);
    assert(tgrep_cache_save(source, path) == 2);
    tgrep_cache_t *target = new_cache();
    assert(tgrep_cache_store_results(target, "existing", 0, 1, paths, 1) == 1);
    assert(tgrep_cache_load_file(target, path) == 2);
    assert(tgrep_cache_count(target) == 3);
    expect_hit_count(target, "key", 0, 1, 2);
    expect_hit_count(target, "existing", 0, 1, 1);
    tgrep_cache_destroy(target);
    tgrep_cache_destroy(source);
}

static void failed_merge_keeps_existing_cache(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "failed-merge.qsc");
    tgrep_cache_t *source = new_cache();
    assert(tgrep_cache_store_results(source, "loaded", 0, 1, paths, 2) == 2);
    assert(tgrep_cache_save(source, path) == 2);
    tgrep_cache_t *target = new_cache();
    assert(tgrep_cache_store_results(target, "existing", 0, 1, paths, 1) == 1);
    insert_budget = 2; /* Stage both disk rows, then fail copying the live row. */
    assert(tgrep_cache_load_file(target, path) == -1);
    insert_budget = -1;
    assert(tgrep_cache_count(target) == 1);
    expect_hit_count(target, "loaded", 0, 1, 0);
    expect_hit_count(target, "existing", 0, 1, 1);
    tgrep_cache_destroy(target);
    tgrep_cache_destroy(source);
}

int main(int argc, char **argv)
{
    disable_core_dumps();
    assert(argc == 2);
    const char *directory = argv[1];
    RUN_TEST(partial_insert_is_a_miss);
    RUN_TEST(replacement_does_not_append);
    RUN_TEST(invalidation_matches_pattern_and_flags);
    RUN_TEST(lookup_oom_does_not_publish_a_prefix);
    RUN_TEST(enumeration_oom_is_reported);
    RUN_TEST(failed_record_write_is_reported);
    RUN_TEST(truncated_load_preserves_existing_cache);
    RUN_TEST(valid_load_merges_complete_rows);
    RUN_TEST(failed_merge_keeps_existing_cache);
    return 0;
}
