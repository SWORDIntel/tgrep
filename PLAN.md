# tgrep Implementation Plan

## Prerequisites

### Pin Cargo.toml dependencies
All deps use `*` (floating). Pin to versions published >7 days ago before anything compiles reproducibly.

| Dependency | Current | Pin to |
|------------|---------|--------|
| ignore | `*` | `0.4` |
| grep-regex | `*` | `0.1` |
| grep-searcher | `*` | `0.1` |
| grep-printer | `*` | `0.1` |
| regex-syntax | `*` | `0.8` |
| serde | `*` | `1.0` |
| serde_json | `*` | `1.0` |
| clap | `*` | `4.5` |
| cc (build-dep) | `*` | `1.0` |

Add `memmap2 = "0.9"` for memory-mapped segment reads.
Add `crossbeam-channel = "0.5"` for the bounded build queue.

### Fix build.rs
The C wrappers currently compile empty. Add `-D_GNU_SOURCE -std=c11 -Wall -Wextra -O2` flags and link against KEYSTONE/QIHSE object files or their static libs.

---

## Phase 1: C FFI Wrappers
**Files:** `native/keystone_wrapper.c`, `native/qihse_wrapper.c`, `src/native.rs`
**Depends on:** Prerequisites
**Goal:** Working FFI bridge to KEYSTONE trigram API + QIHSE file helpers

### 1a. keystone_wrapper.c
Implement these functions (signatures already declared in `src/native.rs`):

```
tgrep_keystone_index_create(capacity) -> *mut c_void
    Wraps keystone_trigram_index_create, returns opaque handle

tgrep_keystone_begin_document(handle) -> i32
    Calls keystone_trigram_index_add_document_external with NULL name, empty text
    Actually: need a streaming variant. KEYSTONE currently takes full text at once.
    For now: buffer in a per-handle struct, flush on end_document.

tgrep_keystone_feed_bytes(handle, data, len) -> i32
    Append bytes to per-handle buffer. KEYSTONE's API takes whole document at once,
    so we buffer and call add_document_external on end_document.

tgrep_keystone_end_document(handle) -> i32
    Call keystone_trigram_index_add_document_external with buffered bytes.
    Return assigned doc_id via out param or handle state.

tgrep_keystone_finalize(handle) -> i32
    Wraps keystone_trigram_index_finalize

tgrep_keystone_visit_postings(handle, callback) -> i32
    Iterate internal posting lists. KEYSTONE doesn't export this yet (per ARCHITECTURE.md
    "Posting visitor" is a needed change). For now: use keystone_trigram_index_get_candidates
    as the query interface instead.

tgrep_keystone_get_candidates(handle, pattern, len, out_buf, max) -> usize
    Wraps keystone_trigram_index_get_candidates

tgrep_keystone_doc_count(handle) -> usize
    Wraps keystone_trigram_index_document_count

tgrep_keystone_get_stats(handle, out_stats) -> i32
    Wraps keystone_trigram_index_get_stats

tgrep_keystone_destroy(handle)
    Wraps keystone_trigram_index_destroy
```

Per-handle struct in C:
```c
typedef struct {
    keystone_trigram_index_t* idx;
    char* doc_buffer;       // accumulating bytes for current document
    size_t doc_len;
    size_t doc_cap;
    uint32_t current_doc_id;
    int finalized;
} tgrep_keystone_handle_t;
```

### 1b. qihse_wrapper.c
```
tgrep_qihse_file_sync(fd) -> i32
    Wraps qihse_file_fsync or fdatasync

tgrep_qihse_dir_sync(path) -> i32
    Opens dir, calls fsync, closes. Wraps qihse helper or direct POSIX.
```

### 1c. Update src/native.rs
- Add `tgrep_keystone_index_create`, `tgrep_keystone_finalize`, `tgrep_keystone_get_candidates`, `tgrep_keystone_doc_count`, `tgrep_keystone_get_stats`, `tgrep_keystone_destroy`
- Add safe Rust wrappers (RAII handle with Drop)

### Verification
```bash
cargo build  # compiles clean
# Write a test that creates an index, adds 3 docs, finalizes, queries candidates
cargo test -- --nocapture
```

---

## Phase 2: Segment Format
**Files:** `src/store.rs`
**Depends on:** Phase 1 (native types)
**Goal:** Write and read tgrep segment files (.tgs)

### 2a. SegmentWriter::flush
Binary format (little-endian), per ARCHITECTURE.md lines 136-154:

```
[Header]      magic(4) version(2) generation(8) profile_hash(8) doc_count(4) + section offsets
[File table]  per doc: local_id(4) root_id(4) path_len(4) path(var) byte_len(8) dev(8) ino(8) mtime(8) ctime(8)
[Path lookup] sorted by (root_id, path) -> file table index
[Dictionary]  sorted gram(4) + doc_freq(4) + posting_offset(8)
[Postings]     per gram: sorted unique doc IDs, delta-varint, blocks of 128
[Block index]  per block: max_doc_id(4) encoded_len(4) checksum(4)
[Footer]       metadata_checksum(8) + complete_marker(4)
```

Implement:
- `add_document(doc, grams)` — dedup grams, append to posting lists
- `flush(segments_dir)` — serialize to tmp/, fsync, rename to segments/, fsync dir
- `publish_manifest(state_dir, segments, generation)` — atomic manifest.json write + fsync

### 2b. SegmentReader (new)
Mmap a .tgs file, parse header, provide:
- `lookup_path(root_id, path) -> Option<DocRecord>` — binary search path lookup table
- `read_postings(gram) -> Vec<u32>` — decode delta-varint blocks for a gram
- `doc_count() -> u32`
- `file_version(doc_id) -> FileVersion` — return metadata for change detection

### Verification
```bash
# Write a test segment with 100 docs, read it back, verify round-trip
cargo test store -- --nocapture
```

---

## Phase 3: Build Pipeline
**Files:** `src/build.rs`, `src/index.rs`
**Depends on:** Phase 1 (FFI), Phase 2 (segment format)
**Goal:** `tgrep index build /path` produces searchable segments

### 3a. start_build (minimal, single-worker)
1. Acquire writer.lock (flock on state_dir/writer.lock)
2. Create KEYSTONE index handle via FFI
3. Walk roots using `ignore` crate (respects .gitignore, hidden files, --one-file-system)
4. For each file:
   a. Read bytes (bounded by 64 MiB cap per file)
   b. Feed to KEYSTONE via tgrep_keystone_feed_bytes / end_document
   c. Record file metadata (path, dev, ino, size, mtime, ctime)
   d. Extract trigrams (KEYSTONE does this internally, but we need them for our segment)
5. At 64 MiB source input or 128 MiB postings or 30s:
   a. Create SegmentWriter, add all docs + postings
   b. Flush segment to disk
   c. Publish manifest
   d. Reset KEYSTONE index for next batch
6. Write build.json with progress

### 3b. Trigram extraction
KEYSTONE's `keystone_trigram_extract` extracts unique 24-bit trigrams from a pattern.
For indexing, we need to extract trigrams from each file's content. Two options:
- **Option A:** Let KEYSTONE do it internally (it does for add_document_external), then export postings via a visitor. Requires the "Posting visitor" API change from ARCHITECTURE.md.
- **Option B:** Extract trigrams in Rust, feed both to KEYSTONE (for candidate queries) and to our SegmentWriter (for persistent storage).

Start with **Option B** — extract in Rust, store in our segment, also feed to KEYSTONE for in-memory candidate queries during the same session. Later, switch to Option A when KEYSTONE exports postings.

### 3c. print_status
Read manifest.json, report:
- Number of segments, total docs, total bytes indexed
- Generation, profile hash
- Build progress (if build.json exists)

### Verification
```bash
# Build index on the strong corpus
cargo run --release -- index build /rpool/scratch/tgrep_corpus
# Check status
cargo run --release -- index status
# Verify segments exist
ls /rpool/data/db/tgrep/segments/
# Verify manifest
cat /rpool/data/db/tgrep/manifest.json
```

---

## Phase 4: Search
**Files:** `src/search.rs`
**Depends on:** Phase 2 (segment reader), Phase 3 (built index)
**Goal:** `tgrep PATTERN PATH` returns rg-compatible results faster than rg

### 4a. Literal search (no regex planning)
1. Read RIPGREP_CONFIG_PATH, apply CLI overrides for ignore/glob/case
2. Walk requested paths using `ignore` crate
3. For each file:
   a. Check if indexed (lookup_path in segments)
   b. If indexed and unchanged (metadata match): it's a candidate
   c. If new/changed/unindexed: send directly to streaming matcher
4. For indexed candidates:
   a. Extract trigrams from the literal pattern
   b. Intersect posting lists (smallest first)
   c. If candidate set is small enough: verify each with ripgrep matcher
   d. If candidate set too large (>25% of corpus): fall back to streaming scan
5. For streaming files: use `grep-searcher` directly
6. Merge results, emit via `grep-printer` in rg-compatible format

### 4b. Query planner (minimal)
- Literal >= 3 bytes: extract trigrams, intersect postings
- Literal < 3 bytes: stream all files
- Regex: extract necessary literals from `regex-syntax` AST, use those trigrams
- If no extractable literals: stream all files

### 4c. Result equality
- Run the same pattern through rg
- Compare line counts and content
- Must match exactly (modulo rg's ignore rules)

### Verification
```bash
# Search for known patterns (must find them)
cargo run --release -- 'rareneedle_tgrep_test_42' /rpool/scratch/tgrep_corpus
# Compare with rg
rg 'rareneedle_tgrep_test_42' /rpool/scratch/tgrep_corpus
# Result counts must match

# Search for common patterns
cargo run --release -- 'struct' /rpool/scratch/tgrep_corpus -c
rg 'struct' /rpool/scratch/tgrep_corpus -c
# Compare

# Search for absent pattern (must be fast — trigram rejection)
time cargo run -- release -- 'ZZZZZ_NO_MATCH_ZZZZZ' /rpool/scratch/tgrep_corpus
time rg 'ZZZZZ_NO_MATCH_ZZZZZ' /rpool/scratch/tgrep_corpus
```

---

## Phase 5: Benchmark Runner
**Files:** `benches/compare_rg.rs`
**Depends on:** Phase 4 (working search)
**Goal:** Automated paired comparison against the strong corpus

### 5a. Implementation
1. Take pattern + path + trial count from CLI
2. Verify result equality (tgrep vs rg) — fail fast if mismatch
3. Run N alternating trials (tgrep, rg, tgrep, rg, ...)
4. Capture: wall time, peak RSS (via getrusage), bytes read
5. Report: median, p95, mean, min, max for each tool
6. Report: speedup ratio

### 5b. Suite runner
Run the 10 history-derived patterns + 6 known patterns against the strong corpus:
```bash
cargo run --release --bin compare_rg -- \
    --patterns rareneedle_tgrep_test_42,ZEBRA_ALPHANUMERIC_X9,struct,keystone,error \
    --path /rpool/scratch/tgrep_corpus \
    --trials 10
```

### Success criteria
- Result equality: 100% match with rg for all patterns
- Speedup: >10x vs rg for rare/absent patterns on warm cache
- Speedup: >2x vs rg for common patterns
- Broad-query scanning: within 10% of rg

---

## Phase 6: QIHSE WAL Integration
**Files:** `src/preindex.rs` (new), `native/qihse_wrapper.c`
**Depends on:** Phase 3 (build pipeline works)
**Goal:** Crash-safe segment publication

- Wire `qihse_vfs_wal_create` / `qihse_vfs_wal_append` for each segment publication
- Replace `build.json` checkpoint with WAL replay via `qihse_recovery_replay`
- `qihse_recovery_checkpoint` at segment publication boundaries
- Test: kill build mid-segment, restart, verify no partial segments

---

## Phase 7: QIHSE Index Manager
**Files:** `src/preindex.rs`
**Depends on:** Phase 6 (WAL)
**Goal:** Fast metadata lookups for refresh

- `qihse_index_manager_create` per root
- `qihse_index_manager_add_btree` on (mtime, inode) for range scans
- `qihse_index_manager_add_hash` on path for exact lookup
- `qihse_index_bulk_load` during initial build
- `qihse_index_scan` with RANGE predicate for "files modified since T"
- Replace the linear metadata scan in refresh with BTREE range scan

---

## Phase 8: KEYSTONE Hash Index Fast Path
**Files:** `native/keystone_wrapper.c`, `src/search.rs`
**Depends on:** Phase 4 (search works)
**Goal:** O(1) exact-match for whole-token queries

- Wire `dsmil_hash_index_add` / `dsmil_hash_index_search` through FFI
- During build: hash each identifier token, add to hash index
- During search: if pattern is a whole-token literal (no regex metachars), check hash index first
- If hash hit: skip trigram intersection, go straight to verification
- If hash miss: fall back to trigram path

---

## Phase 9: Anchor Seeding + Batch Search
**Files:** `native/keystone_wrapper.c`, `src/search.rs`
**Depends on:** Phase 4 (search works)
**Goal:** Warm-start interpolation, parallel multi-trigram intersection

- After each segment flush: call `keystone_anchor_seed_batch` on sorted posting arrays
- For multi-trigram queries: use `keystone_search_batch_auto` for parallel intersection
- Auto-backend router selects scalar/SSE4.2/OpenMP based on array size

---

## Phase 10: QIHSE Optimization DB
**Files:** `src/preindex.rs`
**Depends on:** Phase 5 (benchmarks running)
**Goal:** Persistent learned query optimization

- `qihse_optimization_init` with storage_path under tgrep data dir
- After each search: `qihse_record_performance` with data signature, backend, timing
- Before each search: `qihse_get_optimized_config` for backend/prefetch hints
- `qihse_save_optimization_db` on shutdown
- Benefit: repeated queries get faster as the system learns which backends win

---

## Phase 11: Full Benchmark Suite + Optimization
**Depends on:** Phases 1-10
**Goal:** Prove 10x speedup, optimize bottlenecks

- Run the full pattern suite (10 history + 6 known + random) on NVMe and ZFS
- 10 trials per pattern on NVMe, 30 on ZFS (per the sample-size analysis)
- Report: median, p95, source bytes read, peak RSS, index size, build time
- Separate warm-cache and cold-cache results
- Identify the largest remaining costs and optimize them

---

## Dependency Graph

```
Prerequisites (pin deps)
    |
    v
Phase 1: C FFI Wrappers
    |
    +--> Phase 2: Segment Format
    |        |
    |        v
    |    Phase 3: Build Pipeline
    |        |
    |        +--> Phase 6: QIHSE WAL
    |        |        |
    |        |        v
    |        |    Phase 7: QIHSE Index Manager
    |        |
    |        v
    |    Phase 4: Search
    |        |
    |        +--> Phase 8: Hash Index Fast Path
    |        +--> Phase 9: Anchor Seeding + Batch Search
    |        |
    |        v
    |    Phase 5: Benchmark Runner
    |        |
    |        v
    |    Phase 10: QIHSE Optimization DB
    |        |
    |        v
    |    Phase 11: Full Suite + Optimization
    |
    v
  (all phases feed into Phase 11)
```

## Key Risks

1. **KEYSTONE posting export** — the API doesn't expose postings yet. Option B (extract trigrams in Rust) works around this but means KEYSTONE is only used for in-memory candidate queries, not persistent storage. The "Posting visitor" API change in ARCHITECTURE.md is needed for Option A.

2. **Segment format complexity** — the full format (path lookup, block index, checksums) is a lot of code. Start with a minimal format (header + file table + dictionary + raw postings) and add block index + checksums later.

3. **rg compatibility** — matching rg's exact output format, ignore rules, and exit codes is fiddly. Use `grep-printer` for output and `ignore` for traversal to inherit rg's behavior.

4. **ZFS ARC vs page cache** — cold-cache benchmarks on ZFS need to account for the ARC. `drop_caches` doesn't clear ARC. Need to export/drop the ARC dataset or use a fresh dataset for cold tests.

5. **Build speed** — KEYSTONE reports 111s for 1 GiB. The strong corpus is 1.75 GB, so expect ~200s build time. This is acceptable for a one-time cost but needs to be measured.
