# KEYSTONE Changes for tgrep

## Current State

KEYSTONE's trigram index (`keystone_trigram.c`) has:
- In-memory immutable index with hash-table-backed posting lists
- SSE4.2 trigram extraction during indexing
- Galloping posting-list intersection with SIMD lower_bound
- `bounded_memmem` SIMD substring verification (owned docs only)
- External-document mode (no content retention)

KEYSTONE's trigram index lacks:
- Streaming ingestion (takes whole text at once)
- Posting export (no way to iterate all gram → doc_id pairs)
- Paginated candidate iterator
- Frequency lookup (can't query a gram's df without intersection)
- Memory budget hooks
- The redundant sort in finalize (doc IDs are already sorted by construction)

## Changes Required

All changes are additive — existing APIs stay intact. New functions get new
declarations in `keystone_trigram.h` and implementations in `keystone_trigram.c`.

### Change 1: Streaming ingestion (begin/feed/end)

**Header:** `keystone_trigram.h`
**File:** `keystone_trigram.c`

tgrep reads files in chunks (bounded by `--max-memory`). KEYSTONE's
`add_document_external` takes the full text in one call. Add streaming
ingestion that buffers chunks and flushes on `end_document`.

```c
/* Streaming ingestion context. Opaque to callers. */
typedef struct keystone_trigram_stream keystone_trigram_stream_t;

/* Begin a streaming document. Returns a context for feed/end calls.
 * The index must not be finalized. */
keystone_trigram_stream_t* keystone_trigram_begin_document(
    keystone_trigram_index_t* idx,
    const char* name  /* optional, may be NULL */
);

/* Feed bytes to the current document. Accumulates into an internal
 * buffer. Trigrams are extracted on end_document to handle boundary
 * trigrams across chunk boundaries (two trailing bytes carried over). */
int keystone_trigram_feed_bytes(
    keystone_trigram_stream_t* stream,
    const char* data,
    size_t len
);

/* Finish the document: extract trigrams from accumulated bytes, add
 * to the index, assign a doc_id. Frees the stream context.
 * Sets *out_doc_id if non-NULL. */
int keystone_trigram_end_document(
    keystone_trigram_stream_t* stream,
    uint32_t* out_doc_id
);

/* Abort a streaming document without indexing. Frees the stream. */
void keystone_trigram_cancel_document(
    keystone_trigram_stream_t* stream
);
```

**Implementation:**

```c
struct keystone_trigram_stream {
    keystone_trigram_index_t* idx;
    char* name;              /* copied, freed on end/cancel */
    char* buffer;            /* accumulated bytes */
    size_t buf_len;
    size_t buf_cap;
    int failed;
};
```

`begin_document`: allocate stream, copy name, init buffer (initial 64KB).
`feed_bytes`: append to buffer, grow with realloc (doubling, checked).
`end_document`: call `add_document_external(idx, name, buffer, buf_len, out_doc_id)`, then free stream.
`cancel_document`: free buffer + name + stream without indexing.

This is a thin wrapper — the actual trigram extraction still happens in
`add_document_internal`. The two-trailing-bytes boundary handling is
already correct because we accumulate the full document before extraction.

**Why not extract incrementally?** Trigram extraction across chunk
boundaries requires carrying two trailing bytes between chunks. The
accumulate-then-extract approach is simpler and correct. If memory is a
concern (files > 64 MiB), tgrep can cap indexed file size and mark larger
files as scan-only, per ARCHITECTURE.md line 194.

### Change 2: Posting export visitor

**Header:** `keystone_trigram.h`
**File:** `keystone_trigram.c`

tgrep needs to iterate all (gram, sorted_doc_ids) pairs to write its
persistent segment format. Currently the posting lists are in a private
hash table with no export API.

```c
/* Visitor callback. Called once per unique trigram.
 * gram: the 24-bit trigram key
 * doc_ids: sorted unique document IDs (read-only, valid for callback duration)
 * count: number of doc IDs
 * ctx: user-provided context pointer
 * Return non-zero to stop iteration early. */
typedef int (*keystone_trigram_posting_visitor_t)(
    uint32_t gram,
    const uint32_t* doc_ids,
    size_t count,
    void* ctx
);

/* Visit all posting lists in the finalized index.
 * Trigrams are visited in hash-table order (not sorted by gram).
 * The index must be finalized. */
int keystone_trigram_visit_postings(
    const keystone_trigram_index_t* idx,
    keystone_trigram_posting_visitor_t visitor,
    void* ctx
);
```

**Implementation:**

```c
int keystone_trigram_visit_postings(
    const keystone_trigram_index_t* idx,
    keystone_trigram_posting_visitor_t visitor,
    void* ctx
) {
    if (!idx || !idx->is_finalized || !visitor) return KEYSTONE_TRIGRAM_EINVAL;

    for (size_t i = 0; i < idx->num_buckets; i++) {
        if (idx->bucket_keys[i] == TRIGRAM_KEY_EMPTY) continue;
        const keystone_trigram_posting_list_t* plist = &idx->bucket_lists[i];
        if (plist->count == 0) continue;
        if (visitor(idx->bucket_keys[i], plist->doc_ids, plist->count, ctx))
            break;
    }
    return KEYSTONE_TRIGRAM_OK;
}
```

tgrep's segment writer uses this to extract all postings during a flush.
The visitor receives sorted doc_ids (already sorted by `finalize`).

### Change 3: Frequency lookup

**Header:** `keystone_trigram.h`
**File:** `keystone_trigram.c`

tgrep's query planner needs to know a trigram's document frequency *before*
intersection to pick the rarest grams and estimate candidate set size.

```c
/* Return the document frequency of a trigram (number of docs containing it).
 * Returns 0 if the trigram is not in the index or the index is not finalized. */
size_t keystone_trigram_index_frequency(
    const keystone_trigram_index_t* idx,
    uint32_t gram
);
```

**Implementation:**

```c
size_t keystone_trigram_index_frequency(
    const keystone_trigram_index_t* idx,
    uint32_t gram
) {
    if (!idx || !idx->is_finalized) return 0;
    const keystone_trigram_posting_list_t* plist = get_posting_list(idx, gram);
    return plist ? plist->count : 0;
}
```

`get_posting_list` is already a static function in the file. This just
exposes the count through a public API.

### Change 4: Paginated candidate iterator

**Header:** `keystone_trigram.h`
**File:** `keystone_trigram.c`

`get_candidates` allocates a full candidate buffer. For large candidate
sets, tgrep needs pagination to bound output memory.

```c
/* Opaque candidate iterator. */
typedef struct keystone_trigram_candidate_iter
    keystone_trigram_candidate_iter_t;

/* Create a candidate iterator for a pattern.
 * The index must be finalized. Returns NULL on error. */
keystone_trigram_candidate_iter_t* keystone_trigram_candidates_begin(
    const keystone_trigram_index_t* idx,
    const char* pattern,
    size_t pattern_len
);

/* Fetch the next batch of candidates.
 * Writes up to capacity doc IDs into out_buf.
 * Sets *out_count to the number written.
 * Sets *out_exhausted to 1 when no more candidates remain.
 * Returns KEYSTONE_TRIGRAM_OK on success. */
int keystone_trigram_candidates_next(
    keystone_trigram_candidate_iter_t* iter,
    uint32_t* out_buf,
    size_t capacity,
    size_t* out_count,
    int* out_exhausted
);

/* Free the iterator. */
void keystone_trigram_candidates_free(
    keystone_trigram_candidate_iter_t* iter
);
```

**Implementation:**

The iterator internally runs the same galloping intersection but yields
results in batches. It stores:
- The sorted posting lists (pointers into the index)
- Current position in the base (smallest) list
- Cursor positions for each other list

```c
struct keystone_trigram_candidate_iter {
    const keystone_trigram_index_t* idx;
    const keystone_trigram_posting_list_t* lists[TRIGRAM_QUERY_MAX_UNIQUE];
    size_t num_lists;
    size_t base_pos;       /* current position in base list */
    size_t cursor[TRIGRAM_QUERY_MAX_UNIQUE]; /* positions in other lists */
};
```

`begin`: extract trigrams, look up posting lists, sort by size, init cursors.
`next`: run the galloping intersection loop from `base_pos`, yielding up to
`capacity` candidates, then save state and return.
`free`: free the iterator struct.

### Change 5: Remove redundant sort in finalize

**File:** `keystone_trigram.c`

Doc IDs are assigned in insertion order (0, 1, 2, ...) and appended to
posting lists via `add_doc_to_posting_list`. The per-document dedup bitmap
prevents duplicates. Therefore posting lists are already sorted ascending.

`finalize` currently runs counting sort or qsort on every posting list.
This is O(total_postings) wasted work.

**Change:** Remove the sort loop in `keystone_trigram_index_finalize`.
Keep the total_postings count and unique_trigrams stats.

```c
int keystone_trigram_index_finalize(keystone_trigram_index_t* idx) {
    if (!idx) return KEYSTONE_TRIGRAM_EINVAL;
    if (idx->failed) return idx->failure_code ? idx->failure_code : KEYSTONE_TRIGRAM_ESTATE;
    if (idx->is_finalized) return KEYSTONE_TRIGRAM_OK;

    /* Doc IDs are assigned in insertion order and deduplicated per-document.
     * Posting lists are already sorted ascending. No sort needed. */
    size_t total_postings = 0u;
    for (size_t i = 0u; i < idx->num_buckets; i++) {
        if (idx->bucket_keys[i] == TRIGRAM_KEY_EMPTY) continue;
        const keystone_trigram_posting_list_t* plist = &idx->bucket_lists[i];
        if (plist->count == 0u) continue;
        if (!plist->doc_ids || plist->count > plist->capacity) {
            return poison_index(idx, KEYSTONE_TRIGRAM_ESTATE);
        }
        if (plist->count > SIZE_MAX - total_postings) {
            return poison_index(idx, KEYSTONE_TRIGRAM_EOVERFLOW);
        }
        total_postings += plist->count;
    }

    idx->stats.unique_trigrams = idx->unique_trigrams;
    idx->stats.total_postings = total_postings;
    idx->is_finalized = true;
    return KEYSTONE_TRIGRAM_OK;
}
```

Remove `counting_sort_doc_ids`, `compare_uint32`, and the `counts`
allocation. Save ~111 seconds on the 1 GiB benchmark corpus.

### Change 6: Memory budget reporting

**Header:** `keystone_trigram.h`
**File:** `keystone_trigram.c`

tgrep needs to bound total builder memory. KEYSTONE should report its
current allocation so tgrep can decide when to flush a segment.

```c
/* Return the approximate bytes allocated by the index internally.
 * Includes posting list arrays, doc table, dedup bitmap, and hash table.
 * Does not include the index struct itself (small, fixed). */
size_t keystone_trigram_index_memory_usage(const keystone_trigram_index_t* idx);
```

**Implementation:**

```c
size_t keystone_trigram_index_memory_usage(const keystone_trigram_index_t* idx) {
    if (!idx) return 0;
    size_t total = 0;
    /* Hash table */
    total += idx->num_buckets * (sizeof(uint32_t) + sizeof(keystone_trigram_posting_list_t));
    /* Posting list arrays */
    for (size_t i = 0; i < idx->num_buckets; i++) {
        if (idx->bucket_keys[i] != TRIGRAM_KEY_EMPTY) {
            total += idx->bucket_lists[i].capacity * sizeof(uint32_t);
        }
    }
    /* Doc table */
    total += idx->doc_capacity * sizeof(keystone_trigram_doc_t);
    /* Doc content (owned only) */
    for (size_t i = 0; i < idx->doc_count; i++) {
        if (idx->docs[i].owns_content && idx->docs[i].content)
            total += idx->docs[i].content_len + 1;
        if (idx->docs[i].name)
            total += strlen(idx->docs[i].name) + 1;
    }
    /* Dedup bitmap + touched array */
    total += TRIGRAM_BITMAP_BYTES;
    total += idx->doc_seen_touched_cap * sizeof(size_t);
    return total;
}
```

## Change 7: Allocation budget hook (optional, deferred)

An optional `max_memory` field on the index that causes `add_document`
to return `KEYSTONE_TRIGRAM_ENOMEM` when `memory_usage()` exceeds the
budget. tgrep uses this to trigger segment flushes.

Deferred — tgrep can poll `memory_usage()` itself and decide when to flush.
No KEYSTONE change needed for the initial implementation.

## Implementation Order

1. **Change 5** (remove redundant sort) — simplest, immediate benchmark win
2. **Change 2** (posting export visitor) — needed for segment persistence
3. **Change 3** (frequency lookup) — needed for query planning
4. **Change 1** (streaming ingestion) — needed for chunked file reads
5. **Change 4** (candidate iterator) — needed for large candidate sets
6. **Change 6** (memory reporting) — needed for bounded builds

Changes 1-3 are required for tgrep Phase 1-3. Changes 4-6 are needed by
Phase 3-4. All are additive — no existing API breaks.

## Verification

After each change, verify with the existing KEYSTONE benchmark:

```bash
cd /home/john/Documents/KEYSTONE
cc -D_GNU_SOURCE -std=c11 -Wall -Wextra -O2 -msse4.2 \
    -I include \
    -o benchmarks/trigram_benchmark \
    benchmarks/trigram_benchmark.c \
    src/keystone_trigram.c \
    -lm
./benchmarks/trigram_benchmark
```

Expected: build time drops significantly after Change 5 (no sort).
Candidate queries still return correct results after all changes.

Add a new test that exercises the new APIs:

```c
/* test_tgrep_apis.c */
/* 1. Create index, add 3 docs via streaming API, finalize */
/* 2. Visit all postings, verify counts */
/* 3. Query frequency of known trigrams */
/* 4. Iterate candidates in batches of 10, verify all found */
/* 5. Check memory_usage() is reasonable */
```
