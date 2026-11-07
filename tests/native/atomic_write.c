/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
#include <errno.h>

#include "support.h"

static enum { NORMAL_WRITE, INTERRUPT_ONCE, ZERO_PROGRESS } write_fault;
static size_t zero_write_calls;

static ssize_t fault_write(int descriptor, const void *data, size_t length)
{
    if (write_fault == INTERRUPT_ONCE) {
        write_fault = NORMAL_WRITE;
        errno = EINTR;
        return -1;
    }
    if (write_fault == ZERO_PROGRESS) {
        ++zero_write_calls;
        if (zero_write_calls == 1)
            return 0;
        /* Bound the defective implementation without relying on a wall-clock timeout. */
        errno = EIO;
        return -1;
    }
    return write(descriptor, data, length);
}

#define write fault_write
#include "../../native/qihse_wrapper.c"
#undef write

static void expect_content(const char *path, const char *expected)
{
    FILE *file = fopen(path, "rb");
    assert(file);
    char bytes[64] = {0};
    size_t length = strlen(expected);
    assert(fread(bytes, 1, sizeof(bytes), file) == length);
    assert(memcmp(bytes, expected, length) == 0);
    assert(fclose(file) == 0);
}

static void interrupted_write_retries(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "interrupt.txt");
    write_fault = INTERRUPT_ONCE;
    assert(tgrep_qihse_atomic_write(path, (const uint8_t *)"replacement", 11) == 0);
    expect_content(path, "replacement");
}

static void zero_write_stops_and_preserves_target(const char *directory)
{
    char path[PATH_MAX];
    test_path(path, sizeof(path), directory, "zero-write.txt");
    FILE *file = fopen(path, "wb");
    assert(file);
    assert(fwrite("original", 1, 8, file) == 8 && fclose(file) == 0);
    write_fault = ZERO_PROGRESS;
    zero_write_calls = 0;
    assert(tgrep_qihse_atomic_write(path, (const uint8_t *)"replacement", 11) == -1);
    assert(errno == EIO && zero_write_calls == 1);
    write_fault = NORMAL_WRITE;
    expect_content(path, "original");
}

int main(int argc, char **argv)
{
    disable_core_dumps();
    assert(argc == 2);
    const char *directory = argv[1];
    RUN_TEST(interrupted_write_retries);
    RUN_TEST(zero_write_stops_and_preserves_target);
    return 0;
}
