/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
#include "qihse_btree.h"
#include "support.h"

static int fail_save;
static const char *scratch_directory;

static const char *redirect_legacy_path(const char *path, char *out, size_t capacity)
{
    if (strcmp(path, "/tmp/test_btree_save.qwi") == 0) {
        test_path(out, capacity, scratch_directory, "legacy-save.qwi");
        return out;
    }
    if (strcmp(path, "/tmp/test_btree_cursor.qwi") == 0) {
        test_path(out, capacity, scratch_directory, "legacy-cursor.qwi");
        return out;
    }
    return path;
}

static int fault_save(const qihse_btree_t *tree, const char *path)
{
    if (fail_save)
        return -1;
    char redirected[PATH_MAX];
    return qihse_btree_save(tree, redirect_legacy_path(path, redirected, sizeof(redirected)));
}

static FILE *safe_fopen(const char *path, const char *mode)
{
    char redirected[PATH_MAX];
    return fopen(redirect_legacy_path(path, redirected, sizeof(redirected)), mode);
}

/* Keep the pre-fix program's fixed paths inside the owned scratch directory for red runs. */
#define fopen            safe_fopen
#define qihse_btree_save fault_save
#define main             diagnostic_main
#include "../../native/test_btree_save.c"
#undef main
#undef qihse_btree_save
#undef fopen

static void save_failure_is_a_test_failure(const char *directory)
{
    (void)directory;
    fail_save = 1;
    assert(diagnostic_main() != 0);
    fail_save = 0;
}

static void large_save_and_cursor_scan_pass(const char *directory)
{
    (void)directory;
    assert(diagnostic_main() == 0);
}

int main(int argc, char **argv)
{
    disable_core_dumps();
    assert(argc == 2);
    const char *directory = argv[1];
    scratch_directory = directory;
    RUN_TEST(save_failure_is_a_test_failure);
    RUN_TEST(large_save_and_cursor_scan_pass);
    return 0;
}
