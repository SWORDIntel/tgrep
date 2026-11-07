/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
#ifndef TGREP_NATIVE_TEST_SUPPORT_H
#define TGREP_NATIVE_TEST_SUPPORT_H

#ifdef NDEBUG
#error "Native regression tests require active assertions"
#endif

#include <assert.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/resource.h>

static inline void disable_core_dumps(void)
{
    struct rlimit limit = {.rlim_cur = 0, .rlim_max = 0};
    assert(setrlimit(RLIMIT_CORE, &limit) == 0);
}

static inline void test_path(char *out, size_t capacity, const char *directory, const char *name)
{
    int length = snprintf(out, capacity, "%s/%s", directory, name);
    assert(length >= 0 && (size_t)length < capacity);
}

#define RUN_TEST(test)                                                                             \
    do {                                                                                           \
        fprintf(stderr, "%s\n", #test);                                                            \
        test(directory);                                                                           \
    } while (0)

#endif
