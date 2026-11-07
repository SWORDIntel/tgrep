/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
#include <pthread.h>

#include "support.h"
#include "../../native/qihse_cache_wrapper.c"

struct concurrent_cache {
    tgrep_cache_t *cache;
    const char *snapshot_path;
    pthread_barrier_t start;
};

static void await_start(struct concurrent_cache *state)
{
    int result = pthread_barrier_wait(&state->start);
    assert(result == 0 || result == PTHREAD_BARRIER_SERIAL_THREAD);
}

static void *load_snapshots(void *opaque)
{
    struct concurrent_cache *state = opaque;
    await_start(state);
    for (size_t i = 0; i < 500; ++i) {
        assert(tgrep_cache_load_file(state->cache, state->snapshot_path) == 0);
    }
    return NULL;
}

static void *count_rows(void *opaque)
{
    struct concurrent_cache *state = opaque;
    await_start(state);
    for (size_t i = 0; i < 100000; ++i) {
        assert(tgrep_cache_count(state->cache) == 0);
    }
    return NULL;
}

static void count_and_snapshot_load_share_the_wrapper_lock(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "empty.qsc");
    struct concurrent_cache state = {.cache = tgrep_cache_create(), .snapshot_path = path};
    assert(state.cache);
    assert(tgrep_cache_save(state.cache, path) == 0);
    assert(pthread_barrier_init(&state.start, NULL, 2) == 0);
    pthread_t loader, counter;
    assert(pthread_create(&loader, NULL, load_snapshots, &state) == 0);
    assert(pthread_create(&counter, NULL, count_rows, &state) == 0);
    assert(pthread_join(loader, NULL) == 0);
    assert(pthread_join(counter, NULL) == 0);
    assert(pthread_barrier_destroy(&state.start) == 0);
    tgrep_cache_destroy(state.cache);
}

int main(int argc, char **argv)
{
    disable_core_dumps();
    assert(argc == 2);
    const char *directory = argv[1];
    RUN_TEST(count_and_snapshot_load_share_the_wrapper_lock);
    return 0;
}
