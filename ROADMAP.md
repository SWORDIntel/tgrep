# tgrep Roadmap

## Status: Phase 9 of 11 complete

```
[██████████████████████████████████████░░░░░░░░░░░░] 82%
```

## Completed

### KEYSTONE Changes (prerequisite)
- [x] **Change 5**: Remove redundant sort in `finalize` (posting lists already sorted)
- [x] **Change 3**: Frequency lookup (`keystone_trigram_index_frequency`)
- [x] **Change 6**: Memory reporting (`keystone_trigram_index_memory_usage`)
- [x] **Change 1**: Streaming ingestion (`begin_document` / `feed_bytes` / `end_document` / `cancel`)
- [x] **Change 2**: Posting export visitor (`keystone_trigram_visit_postings`)
- [x] **Change 4**: Paginated candidate iterator (`candidates_begin` / `next` / `free`)
- [x] Internal header `keystone_trigram_internal.h` for extension modules
- [x] Test suite `test_trigram_tgrep_apis.c` — 6 tests pass
- [x] Existing `test_trigram_index` still passes
- [x] Benchmark: 1 GiB corpus, 114s build, 1.6ms search, 624x speedup

### Phase 0: Prerequisites
- [x] Pin all Cargo.toml dependencies (no floating `*` ranges)
- [x] Add `memmap2` and `crossbeam-channel` deps
- [x] Fix `build.rs` to compile KEYSTONE sources with wrappers

### Phase 1: C FFI Wrappers
- [x] `native/keystone_wrapper.c` — wraps all KEYSTONE APIs with `void*` handles
- [x] `native/qihse_wrapper.c` — fsync, dir_sync, atomic_write
- [x] `src/native.rs` — raw FFI + safe RAII wrappers (KeystoneIndex, DocumentStream, CandidateIter)
- [x] 8 FFI tests pass

### Phase 2: Segment Format
- [x] `SegmentWriter` — serialize .tgs files (header, file table, path index, dictionary, postings, block index, footer)
- [x] `SegmentReader` — mmap + read .tgs files (get_doc, lookup_path, read_postings, iter_postings)
- [x] Delta-varint encoded posting blocks (128 per block)
- [x] Crash-safe publication via atomic_write
- [x] `publish_manifest` / `load_manifest` (JSON)
- [x] 4 store tests pass (round-trip, 1000-doc large segment, manifest, varint)

### Phase 3: Build Pipeline
- [x] `start_build` — walks roots with `ignore` crate, feeds files to KEYSTONE streaming ingestion
- [x] C trampoline `tgrep_keystone_collect_postings` for posting visitor → Rust BTreeMap
- [x] `KeystoneIndex::collect_postings()` safe Rust wrapper
- [x] `SegmentWriter::add_doc_record` + `set_postings` for KEYSTONE visitor output
- [x] Writer lock (flock-based) for exclusive build access
- [x] `build.json` state tracking (running/paused/completed, progress)
- [x] Flush thresholds: 64 MiB source, 30s, 10K files
- [x] `print_status` — reads manifest, reports segments/docs/postings/disk usage
- [x] Tested on strong corpus: 22,235 files, 1 GB source, 57 segments, 170M postings

### Phase 4: Search
- [x] Query planner — extract trigrams from literal/regex patterns, smart-case detection
- [x] Ripgrep config parsing (`~/.ripgreprc` / `RIPGREP_CONFIG_PATH`)
- [x] Open segments from manifest, intersect posting lists across segments
- [x] Map doc IDs to file paths via indexed roots
- [x] Verify candidates with `grep-searcher` + `grep-regex` matcher
- [x] Streaming fallback for unindexed/changed files
- [x] rg-compatible output via `grep-printer` (Standard + Summary printers)
- [x] Support -l, -c, -n, -q, -i, -S, -F flags
- [x] Binary detection (quit on NUL byte, matching rg default)
- [x] Exit codes matching rg (0=match, 1=no match, 2=error)
- [x] Case-insensitive fallback to streaming (trigram index is case-sensitive)
- [x] 15 search tests pass (trigram extraction, planning, intersection, regex literal)
- [x] Verified output equality with rg on strong corpus:
  - "terminal" (smart-case): 2011 files, 0 diff
  - "rareneedle": 7 files, 0 diff
  - "fn main": 142 files, 0 diff
  - "keystone_trigram": 10 files, 0 diff
  - "struct": 4698 files, 0 diff
  - "nonexistent_pattern_xyz123": 0 files, 0 diff
  - -c count mode: identical
  - -n line number mode: identical
  - -q quiet mode: identical exit codes

### Phase 5: Benchmark Runner
- [x] `src/bin/bench.rs` — paired tgrep vs rg benchmark runner
- [x] Result equality verification on first trial (sorted comparison)
- [x] N alternating trials per pattern (default 10)
- [x] Statistics: min, median, p95, max, mean
- [x] CSV output with raw per-trial data
- [x] `--indexed-only` mode for pure indexed path benchmarking
- [x] Benchmarks run against strong corpus (22,235 indexed files, 57 segments)

#### Search Optimizations
- [x] Parallel search via `std::thread::scope` (4 threads)
- [x] Reuse `Searcher` per thread (avoid per-file allocation)
- [x] O(n) `iter_docs()` replaces O(n²) `get_doc()` loop
- [x] `get_docs_bulk()` with HashSet for O(1) doc ID lookups
- [x] Skip indexed non-candidates (only scan truly unindexed files)
- [x] Conditional `indexed_paths` building (only when needed)

#### Benchmark Results (NVMe, 10 trials, strong corpus, full mode = correct)

| Pattern | tgrep median | rg median | Speedup |
|---------|-------------|-----------|---------|
| RareNeedle (absent, indexed) | 629ms | 1069ms | **1.7x faster** |
| KeystoneTrigram (absent, indexed) | 611ms | 909ms | **1.5x faster** |
| Struct (238 matches, indexed) | 681ms | 1039ms | **1.5x faster** |
| terminal (2011 matches, streaming) | 1290ms | 977ms | 1.3x slower |

#### Indexed-Only Mode (skips unindexed file scan)

| Pattern | tgrep median | rg median | Speedup |
|---------|-------------|-----------|---------|
| RareNeedle (absent) | 24ms | 1185ms | **49.4x faster** |
| Struct (229 matches) | 141ms | 1035ms | **7.3x faster** |

**Key findings:**
- Full mode (correct): 1.5-1.7x faster than rg for indexed patterns
- Indexed-only mode: 7-49x faster than rg
- Streaming patterns (smart-case, case-insensitive): 1.3x slower (no index benefit)
- Remaining bottleneck: filesystem walk (~500ms) to find unindexed files
- Phase 7 (metadata index) will eliminate the walk for unchanged corpora

## Remaining Phases

### Phase 6: QIHSE WAL Integration
- [x] Wire `qihse_vfs_wal` for crash-safe segment publication
- [x] Log each segment publication as a WAL transaction (BEGIN/INSERT/COMMIT)
- [x] Replay committed transactions on startup to recover segments
- [x] Clean up orphaned segment files not in WAL or manifest
- [x] Checkpoint WAL after manifest publication to bound log size
- [x] `tgrep index recover` command for manual WAL recovery
- [x] WAL status in `tgrep index status`
- [x] Test: committed transactions replayed correctly
- [x] Test: uncommitted transactions skipped (crash simulation)
- [x] Test: orphaned segment files cleaned up
- [x] Test: multiple transactions with mixed commit states

### Phase 7: File List Cache + Metadata Optimization
- [x] Persistent file list cache (`file_list_cache.json`) saved during build
- [x] Cache stores path, size, mtime_ns, inode, device for each indexed file
- [x] Cache loaded during search to avoid filesystem walk
- [x] Sample-based validity check (20 evenly-spaced files)
- [x] Stale cache detection (size/mtime/inode mismatch → fallback to walk)
- [x] Root overlap verification (cache only used if roots match search paths)
- [x] Subdirectory path filtering for indexed candidates
- [x] Hidden file indexing fix (`.hidden(false)` in build walker)
- [x] Fresh build fix (clear old segments + WAL at build start)
- [x] Correctness verified: 8/8 patterns match rg, subdirectory searches correct
- [x] Stale cache fallback tested (modified file → detected → walk fallback)
- [x] Benchmarks: 4.6–5.5x faster than rg in full mode (was 1.5–1.7x)

#### Phase 7 Benchmark Results (NVMe, 10 trials, 26K files, 75 segments)

| Pattern | tgrep median | rg median | Speedup |
|---------|-------------|-----------|---------|
| RareNeedle (absent, full mode) | 114ms | 630ms | **5.5x faster** |
| KeystoneTrigram (absent, full mode) | ~118ms | ~640ms | **4.7x faster** |
| Struct (238 matches, full mode) | 148ms | 686ms | **4.6x faster** |
| terminal (2011, streaming) | 1347ms | 782ms | 1.7x slower |
| RareNeedle (absent, indexed-only) | ~55ms | ~640ms | **20.4x faster** |
| Struct (238 matches, indexed-only) | ~80ms | ~680ms | **8.5x faster** |

### Phase 8: KEYSTONE Hash Index Fast Path
- [x] `dsmil_hash_index_search_all` — return all matching doc IDs (not just first)
- [x] Hash index serialization/deserialization (`dsmil_hash_index_save` / `load`)
- [x] `native/hash_wrapper.c` — FFI wrapper around `dsmil_hash_index_t`
- [x] Rust FFI bindings + safe `HashIndex` RAII wrapper in `src/native.rs`
- [x] Tokenize files during build (ASCII word characters, unique tokens per file)
- [x] Per-segment `.thi` sidecar files saved alongside `.tgs` segments
- [x] `QueryPlan::HashIndex` variant for whole-token queries (`-w` flag)
- [x] `hash_index_candidates()` — load `.thi` files, lookup token, map to paths
- [x] Binary search fix (unsigned comparison matching radix sort order)
- [x] `-w` / `--word-regexp` CLI flag
- [x] Word-boundary regex matching via `grep_regex::RegexMatcherBuilder::word()`
- [x] Correctness verified: 6/6 word-search patterns match `rg -w`, 8/8 regular patterns match `rg`
- [x] 36 tests pass (30 existing + 6 new hash index tests)

#### Phase 8 Benchmark Results (NVMe, 5 trials, 26K files, 75 segments, `-w -l` mode)

| Pattern | tgrep median | rg median | Speedup |
|---------|-------------|-----------|---------|
| `-w Terminal` (679 matches) | 122ms | 4395ms | **374x faster** |
| `-w Struct` (77 matches) | 68ms | 4510ms | **499x faster** |
| `-w main` (1901 matches) | 11ms | 5926ms | **1069x faster** |
| `-w nonexistent_xyz` (absent) | 141ms | 5407ms | **670x faster** |

**Key findings:**
- Hash index fast path is 374x–1069x faster than `rg -w` for whole-word queries
- Tokenization uses ASCII word boundaries (alphanumeric + underscore)
- Hash index is persisted as `.thi` sidecar files (~10MB total for 75 segments)
- Falls back to trigram path for multi-word patterns or case-insensitive queries
- Binary files are correctly filtered by regex verification (matching rg behavior)

### Phase 9: Anchor Seeding + Batch Search
- [x] `keystone_anchor_seed_batch` FFI wrapper (`tgrep_keystone_anchor_seed_batch`)
- [x] `keystone_search_batch_auto` FFI wrapper with auto-backend router
- [x] `AnchorTable` RAII wrapper (create, seed_batch, destroy)
- [x] `batch_search_auto` — batch lookup with auto-backend (scalar/SSE4.2/OpenMP)
- [x] `detect_cpu_features` — CPU SIMD detection for backend selection
- [x] CPU features + backend selection in `--explain` output
- [x] 4 new tests (anchor table, batch search, batch with anchors, CPU features)
- [x] 40 tests pass (36 existing + 4 new Phase 9 tests)
- [x] Correctness verified: 10/10 patterns match rg

### Phase 10: QIHSE Optimization DB
- [ ] `qihse_optimization_init` with storage path
- [ ] Record performance per query (data signature, backend, timing)
- [ ] Get optimized config before each search
- [ ] Save on shutdown

### Phase 11: Full Benchmark Suite
- [ ] 10 history patterns + 6 known patterns + random
- [ ] 10 trials per pattern on NVMe, 30 on ZFS
- [ ] Separate warm-cache and cold-cache results
- [ ] Prove 10x speedup for rare/absent patterns
- [ ] Prove within 10% of rg for broad queries
- [ ] Optimize largest remaining costs

## Dependency Graph

```
Phase 0 (prerequisites)     [DONE]
Phase 1 (C FFI)             [DONE]
Phase 2 (segment format)    [DONE]
    │
    ▼
Phase 3 (build pipeline)    [DONE]
    │
    ├──▶ Phase 6 (QIHSE WAL)
    │        │
    │        ▼
    │    Phase 7 (QIHSE index manager)
    │
    ▼
Phase 4 (search)            [DONE]
    │
    ├──▶ Phase 8 (hash index fast path)
    ├──▶ Phase 9 (anchor seeding + batch)
    │
    ▼
Phase 5 (benchmark runner)  [DONE]
    │
    ▼
Phase 6 (QIHSE WAL)         [DONE]
    │
    ▼
Phase 7 (file list cache)   [DONE]
    │
    ▼
Phase 8 (hash index)        [DONE]
    │
    ▼
Phase 9 (anchor + batch)    [DONE]
    │
    ▼
Phase 10 (optimization DB)
    │
    ▼
Phase 11 (full suite + optimize)
```

## Metrics

| Metric | Current | Target |
|--------|---------|--------|
| Test count | 40 pass | 50+ |
| Lines of code | ~5,000 | ~8,000 |
| Phases complete | 9/11 | 11/11 |
| grep baseline (NVMe) | 3,880ms | — |
| rg baseline (NVMe) | 653ms | — |
| tgrep full (absent, indexed) | 114ms (5.5x rg) | <65ms (10x rg) |
| tgrep full (common, indexed) | 148ms (4.6x rg) | <65ms (10x rg) |
| tgrep indexed-only (absent) | ~55ms (20.4x rg) | <65ms (10x rg) |
| tgrep indexed-only (common) | ~80ms (8.5x rg) | <65ms (10x rg) |
| tgrep -w (word search) | 11–141ms (374–1069x rg) | — |
| rg output equality | 14/14 patterns, 0 diff | — |

## Key Risks

1. **Trigram extraction duplication** — KEYSTONE extracts internally but doesn't export. Rust-side extraction is redundant work. Mitigation: use posting visitor to extract from KEYSTONE after finalize, skip Rust extraction.

2. **Path index is variable-size** — binary search requires linear scan to find entries. Mitigation: add a fixed-size secondary index (hash table or B-tree) in a later phase.

3. **rg compatibility** — matching rg's exact output, ignore rules, and exit codes. Mitigation: use `grep-printer` and `ignore` crates to inherit behavior.

4. **ZFS ARC invalidation** — cold-cache benchmarks on ZFS can't use `drop_caches`. Mitigation: export/drop the ARC dataset or use a fresh dataset.

5. **Large file handling** — files > 64 MiB should be scan-only, not indexed. Mitigation: cap indexed file size, mark large files in segment metadata.
