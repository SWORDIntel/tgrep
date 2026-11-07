/* SPDX-License-Identifier: LicenseRef-SRSL-1.1 */
/* Standalone synthetic B+ tree persistence and full-cursor regression. */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

#include "qihse_btree.h"

int main(void)
{
    int result = 1;
    qihse_btree_t *tree = qihse_btree_create(0);
    qihse_btree_t *loaded = NULL;
    qihse_btree_cursor_t *cur = NULL;
    FILE *f = NULL;
    char dir[] = "/tmp/tgrep-btree-test-XXXXXX";
    char saved[128] = {0}, scanned[128] = {0};
    if (!tree || !mkdtemp(dir))
        goto cleanup;
    snprintf(saved, sizeof(saved), "%s/tree.qwi", dir);
    snprintf(scanned, sizeof(scanned), "%s/cursor.qwi", dir);
    const size_t n = 50000;
    srand(42);
    for (size_t i = 0; i < n; i++) {
        unsigned char key[16];
        for (int j = 0; j < 8; j++)
            key[j] = rand() & 0xff;
        for (int j = 0; j < 8; j++)
            key[8 + j] = (unsigned char)((uint64_t)i >> (56 - 8 * j));
        if (!qihse_btree_insert(tree, key, 16, i))
            goto cleanup;
    }
    size_t count = qihse_btree_size(tree);
    if (count != n || qihse_btree_save(tree, saved) != 0)
        goto cleanup;
    loaded = qihse_btree_load(saved);
    if (!loaded || qihse_btree_size(loaded) != count)
        goto cleanup;
    f = fopen(scanned, "wb");
    if (!f)
        goto cleanup;
    uint32_t magic = 0x54575149, version = 2;
    uint64_t hdr_count = count;
    if (fwrite(&magic, 4, 1, f) != 1 || fwrite(&version, 4, 1, f) != 1 ||
        fwrite(&hdr_count, 8, 1, f) != 1)
        goto cleanup;
    cur = qihse_btree_range_open(tree, NULL, 0, NULL, 0);
    if (!cur)
        goto cleanup;
    size_t written = 0, key_len;
    const void *key;
    uint64_t row_id;
    while (qihse_btree_cursor_get(cur, &key, &key_len, &row_id)) {
        unsigned char rid[8];
        for (int j = 0; j < 8; j++)
            rid[j] = (unsigned char)(row_id >> (56 - 8 * j));
        if (key_len != 16 || fwrite(key, 16, 1, f) != 1 || fwrite(rid, 8, 1, f) != 1)
            goto cleanup;
        written++;
        if (!qihse_btree_cursor_next(cur))
            break;
    }
    if (fclose(f) != 0) {
        f = NULL;
        goto cleanup;
    }
    f = NULL;
    if (written != count)
        goto cleanup;
    printf("PASS: persisted and scanned %zu entries\n", count);
    result = 0;
cleanup:
    if (f)
        fclose(f);
    if (cur)
        qihse_btree_cursor_close(cur);
    if (loaded)
        qihse_btree_destroy(loaded);
    if (tree)
        qihse_btree_destroy(tree);
    if (saved[0])
        unlink(saved);
    if (scanned[0])
        unlink(scanned);
    if (saved[0])
        rmdir(dir);
    if (result)
        fprintf(stderr, "B+ tree persistence/cursor regression failed\n");
    return result;
}
