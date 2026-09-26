/* Standalone test: create a large B+ tree, save it, check written == count */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <time.h>

#include "qihse_btree.h"

int main(void) {
    qihse_btree_t* tree = qihse_btree_create(0);
    if (!tree) { fprintf(stderr, "create failed\n"); return 1; }

    /* Insert entries with 16-byte keys (same as tgrep word index) */
    size_t n = 50000;
    srand(42);
    for (size_t i = 0; i < n; i++) {
        unsigned char key[16];
        /* Random hash prefix + sequential doc_id */
        for (int j = 0; j < 8; j++) key[j] = rand() & 0xff;
        for (int j = 0; j < 8; j++) key[8 + j] = (unsigned char)(i >> (56 - 8 * j));
        if (!qihse_btree_insert(tree, key, 16, i)) {
            fprintf(stderr, "insert failed at %zu\n", i);
            return 1;
        }
    }

    size_t count = qihse_btree_size(tree);
    printf("tree count: %zu\n", count);

    /* Save */
    const char* path = "/tmp/test_btree_save.qwi";
    int rc = qihse_btree_save(tree, path);
    printf("save rc: %d\n", rc);

    /* Also test the tgrep save path (cursor-based) */
    FILE* f = fopen("/tmp/test_btree_cursor.qwi", "wb");
    if (!f) { fprintf(stderr, "fopen failed\n"); return 1; }

    /* Write header */
    uint32_t magic = 0x54575149;
    uint32_t version = 2;
    uint64_t hdr_count = count;
    fwrite(&magic, 4, 1, f);
    fwrite(&version, 4, 1, f);
    fwrite(&hdr_count, 8, 1, f);

    /* Cursor scan */
    qihse_btree_cursor_t* cur = qihse_btree_range_open(tree, NULL, 0, NULL, 0);
    if (!cur) {
        fprintf(stderr, "range_open returned NULL (tree has %zu entries)\n", count);
        fclose(f);
        return 1;
    }

    size_t written = 0;
    const void* key;
    size_t key_len;
    uint64_t row_id;
    while (qihse_btree_cursor_get(cur, &key, &key_len, &row_id)) {
        if (key_len == 16) {
            fwrite(key, 16, 1, f);
            written++;
        } else {
            fprintf(stderr, "WARNING: entry %zu has key_len=%zu (expected 16)\n", written, key_len);
        }
        if (!qihse_btree_cursor_next(cur)) break;
    }
    qihse_btree_cursor_close(cur);
    fclose(f);

    printf("cursor written: %zu\n", written);
    printf("match: %s\n", written == count ? "YES" : "NO");
    if (written != count) {
        printf("MISMATCH: count=%zu, written=%zu, diff=%zu\n", count, written, count - written);
    }

    qihse_btree_destroy(tree);
    return 0;
}
