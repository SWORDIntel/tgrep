/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
#include <math.h>
#include <signal.h>
#include <sys/wait.h>

#include "support.h"
#include "../../native/qihse_opt_wrapper.c"

static tgrep_data_signature_t signature(void)
{
    tgrep_data_signature_t value = {
        .data_hash = 42,
        .array_size = 100,
        .data_type = 0,
        .entropy = 0.5,
        .gap_variance = 0.1,
    };
    return value;
}

static void truncated_load_keeps_previous_entries(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "truncated.opt");
    tgrep_optimization_db_t db;
    assert(tgrep_optimization_init(&db, 4, path) == 0);
    tgrep_data_signature_t sig = signature();
    tgrep_optimization_record(&db, &sig, 0, 4, 10, 1, 2, 0);
    FILE *file = fopen(path, "wb");
    assert(file);
    uint32_t magic = TGREP_OPT_MAGIC, version = TGREP_OPT_VERSION;
    size_t count = 3;
    assert(fwrite(&magic, 4, 1, file) == 1);
    assert(fwrite(&version, 4, 1, file) == 1);
    assert(fwrite(&count, sizeof(count), 1, file) == 1);
    assert(fclose(file) == 0);
    assert(tgrep_optimization_load(&db) != 0);
    assert(tgrep_optimization_count(&db) == 1);
    tgrep_optimized_config_t config;
    assert(tgrep_optimization_get_config(&db, &sig, 1, &config) == 1);
    assert(config.optimal_threads == 2);
    tgrep_optimization_destroy(&db);
}

static void first_sample_seeds_averages(const char *directory)
{
    (void)directory;
    tgrep_optimization_db_t db;
    assert(tgrep_optimization_init(&db, 4, NULL) == 0);
    tgrep_data_signature_t sig = signature();
    tgrep_optimization_record(&db, &sig, 0, 4, 10, 1, 2, 0);
    tgrep_optimized_config_t config;
    assert(tgrep_optimization_get_config(&db, &sig, 1, &config) == 1);
    assert(config.avg_speedup == 10 && config.avg_confidence == 1);
    tgrep_optimization_record(&db, &sig, 1, 4, 5, 1, 4, 0);
    assert(tgrep_optimization_get_config(&db, &sig, 1, &config) == 1);
    assert(config.best_pipeline == 0 && config.avg_speedup == 9.5);
    tgrep_optimization_destroy(&db);
}

static void heap_handle_keeps_mutex_aligned(const char *directory)
{
    (void)directory;
    tgrep_optimization_db_t *db = tgrep_optimization_create(4, NULL);
    assert(db);
    assert((uintptr_t)db % _Alignof(tgrep_optimization_db_t) == 0);
    tgrep_data_signature_t sig = signature();
    tgrep_optimization_record(db, &sig, 0, 4, 10, 1, 2, 0);
    assert(tgrep_optimization_count(db) == 1);
    tgrep_optimization_free(db);
    tgrep_optimization_free(NULL);
}

static void *record_on_heap_handle(void *arg)
{
    tgrep_optimization_db_t *db = arg;
    tgrep_data_signature_t sig = signature();
    for (int i = 0; i < 1000; i++)
        tgrep_optimization_record(db, &sig, 0, 4, 10, 1, 2, 0);
    return NULL;
}

static void heap_handle_supports_concurrent_records(const char *directory)
{
    (void)directory;
    tgrep_optimization_db_t *db = tgrep_optimization_create(4, NULL);
    assert(db);
    pthread_t first, second;
    assert(pthread_create(&first, NULL, record_on_heap_handle, db) == 0);
    assert(pthread_create(&second, NULL, record_on_heap_handle, db) == 0);
    assert(pthread_join(first, NULL) == 0);
    assert(pthread_join(second, NULL) == 0);
    tgrep_data_signature_t sig = signature();
    tgrep_optimized_config_t config;
    assert(tgrep_optimization_get_config(db, &sig, 1, &config) == 1);
    assert(config.samples == 2000);
    tgrep_optimization_free(db);
}

static void padding_does_not_change_signature_identity(const char *directory)
{
    (void)directory;
    tgrep_optimization_db_t db;
    assert(tgrep_optimization_init(&db, 4, NULL) == 0);
    tgrep_data_signature_t first, second;
    memset(&first, 0, sizeof(first));
    memset(&second, 0xff, sizeof(second));
    first.data_hash = second.data_hash = 42;
    first.array_size = second.array_size = 100;
    first.data_type = second.data_type = 0;
    first.entropy = second.entropy = 0.5;
    first.gap_variance = second.gap_variance = 0.1;
    tgrep_optimization_record(&db, &first, 0, 4, 10, 1, 2, 0);
    tgrep_optimized_config_t config;
    assert(tgrep_optimization_get_config(&db, &second, 1, &config) == 1);
    tgrep_optimization_destroy(&db);
}

static void nonfinite_signatures_are_rejected(const char *directory)
{
    (void)directory;
    tgrep_optimization_db_t db;
    assert(tgrep_optimization_init(&db, 4, NULL) == 0);
    tgrep_data_signature_t sig = signature();
    sig.entropy = NAN;
    tgrep_optimization_record(&db, &sig, 0, 4, 10, 1, 2, 0);
    assert(tgrep_optimization_count(&db) == 0);
    sig.entropy = 0.5;
    sig.gap_variance = INFINITY;
    tgrep_optimization_record(&db, &sig, 0, 4, 10, 1, 2, 0);
    assert(tgrep_optimization_count(&db) == 0);
    tgrep_optimization_destroy(&db);
}

static void valid_persistence_roundtrip(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "roundtrip.opt");
    tgrep_optimization_db_t db;
    assert(tgrep_optimization_init(&db, 4, path) == 0);
    tgrep_data_signature_t sig = signature();
    tgrep_optimization_record(&db, &sig, 0, 4, 10, 1, 2, 0);
    assert(tgrep_optimization_save(&db) == 0);
    tgrep_optimization_destroy(&db);
    assert(tgrep_optimization_init(&db, 4, path) == 0);
    tgrep_optimized_config_t config;
    assert(tgrep_optimization_get_config(&db, &sig, 1, &config) == 1);
    assert(config.optimal_threads == 2 && config.samples == 1);
    tgrep_optimization_destroy(&db);
}

static void failed_save_reports_the_io_error(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "write-error.opt");
    tgrep_optimization_db_t db;
    assert(tgrep_optimization_init(&db, 4, path) == 0);
    tgrep_data_signature_t sig = signature();
    tgrep_optimization_record(&db, &sig, 0, 4, 10, 1, 2, 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(signal(SIGXFSZ, SIG_IGN) != SIG_ERR);
        struct rlimit limit = {.rlim_cur = 16, .rlim_max = 16};
        assert(setrlimit(RLIMIT_FSIZE, &limit) == 0);
        int result = tgrep_optimization_save(&db);
        _exit(result < 0 ? 0 : 1);
    }
    int status;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(tgrep_optimization_load(&db) != 0);
    assert(tgrep_optimization_count(&db) == 1);
    tgrep_optimization_destroy(&db);
}

int main(int argc, char **argv)
{
    disable_core_dumps();
    assert(argc == 2);
    const char *directory = argv[1];
    RUN_TEST(truncated_load_keeps_previous_entries);
    RUN_TEST(first_sample_seeds_averages);
    RUN_TEST(heap_handle_keeps_mutex_aligned);
    RUN_TEST(heap_handle_supports_concurrent_records);
    RUN_TEST(padding_does_not_change_signature_identity);
    RUN_TEST(nonfinite_signatures_are_rejected);
    RUN_TEST(valid_persistence_roundtrip);
    RUN_TEST(failed_save_reports_the_io_error);
    return 0;
}
