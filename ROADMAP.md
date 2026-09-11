# tgrep Roadmap

## Status: Phase 11 of 11 complete + v2 cache optimization

```
[██████████████████████████████████████████████████████] 100%
```

## Post-Phase 11: v2 Cache Optimization

- [x] v2 cache format: grouped entries with u32 file IDs (102x smaller: 1.9 MB -> 18.4 KB)
- [x] Hash index for O(1) lookup (no table scan)
- [x] Generation compaction (tgrep cache compact)
- [x] v1 backward compat (auto-detects and migrates v1 .qsc files)
- [x] Terminal dashboard (tgrep cache dashboard) - live cache + KEYSTONE telemetry
- [x] Cache subcommands: status, precache, clear, compact, keystone-stats, dashboard
- [x] Precache progress file for dashboard monitoring
- [x] 58 tests pass (54 existing + 4 new v2 cache tests)

#### v2 Cache Benchmark (warm cache vs rg, 26K file corpus)

| Pattern | tgrep warm | rg | Speedup | Cache size |
|---------|-----------|-----|---------|-----------|
| struct (-i) | 64ms | 1153ms | 18x | 18.4 KB |
| main (-i) | 72ms | 753ms | 10x | - |
| return (-i) | 67ms | 1181ms | 18x | - |
| terminal (-i) | 89ms | 1454ms | 16x | - |

12 pattern variants (34,843 file refs): 136.4 KB total (v1 would be ~4.3 MB)

## Post-Phase 11: QIHSE-Backed Word Index

- [x] Add save/load persistence to QIHSE btree and hash_index (opt-in)
- [x] Fix btree page overflow with variable-length keys (byte-size tracking)
- [x] FFI wrapper: native/qihse_hash_wrapper.c (fixed-width 16-byte keys)
- [x] Rust QihseWordIndex safe wrapper in src/native.rs
- [x] Build .qwi sidecars alongside .thi during indexing
- [x] Search prefers .qwi, falls back to .thi for legacy segments
- [x] 60 tests pass (58 existing + 2 new QIHSE word index tests)

#### QIHSE Word Index vs KEYSTONE Hash Index (26K file corpus, 75 segments)

| Metric | .qwi (QIHSE btree) | .thi (KEYSTONE hash) |
|--------|-------------------|---------------------|
| Total size | 260 MB | 339 MB |
| Reduction | 23% smaller | — |
| -w "struct" time | ~2.1s | ~2.0s |
| -w "struct" matches | 124901 | 124901 |
| Correctness vs rg | identical | identical |

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
- [x] `native/qihse_opt_wrapper.c` — self-contained optimization DB (init, record, get, save, load, destroy)
- [x] `qihse_optimization_init` with storage path (auto-loads existing DB)
- [x] Record performance per query (data signature, backend, timing, threads, dimensions)
- [x] Get optimized config before each search (min 5 samples required)
- [x] Save on shutdown (after each indexed search)
- [x] Rust FFI bindings + safe `OptimizationDatabase` RAII wrapper in `src/native.rs`
- [x] `DataSignature` struct with explicit padding to match C layout
- [x] `compute_query_signature` — FNV-1a hash over trigrams + posting-list size
- [x] Integration into `run_search` (consult before search, record after search)
- [x] Optimization DB status shown in `--explain` output
- [x] 5 new tests (create+record, get_config after samples, persistence round-trip, anchor recording, signature computation)
- [x] 45 tests pass (40 existing + 5 new Phase 10 tests)
- [x] Correctness verified: 4/4 patterns match rg

#### Phase 10 Design

The optimization DB records per-query-class performance and recommends
optimal thread/backend/dimension configuration for future searches with
similar data signatures. The data signature is an FNV-1a hash over the
trigram keys and total posting-list size, providing a stable identifier
for query classes.

The DB is persisted as `optimization.qdb` in the state directory, using
a binary format with magic `0x54475044` ("TGPD"), version 1, entry count,
and serialized entry records. The DB auto-loads on initialization if the
file exists.

The C wrapper (`qihse_opt_wrapper.c`) is self-contained and does not link
the full QIHSE search implementation, avoiding unrelated quantum-search
dependencies while preserving the optimization DB semantics.

### Phase 11: Full Benchmark Suite
- [x] 18 patterns (8 rare + 4 broad + 4 word + 2 case-insensitive)
- [x] 10 trials per pattern on NVMe (warm cache)
- [x] `bench_suite` multi-pattern runner with category targets
- [x] Parallel hash index loading (4 threads)
- [x] Correctness verified: 18/18 patterns match rg
- [x] Broad queries: 4.9–8.8x faster than rg (exceeds "within 10%" target)
- [x] Rare patterns: 7.3–9.5x faster (avg 8.44x, close to 10x target)
- [x] Case-insensitive: within 10% of rg (streaming fallback)
- [x] Word search regression identified: hash index doesn't scale to 900K tokens/segment

#### Phase 11 Benchmark Results (NVMe, 10 trials, warm cache, full mode, 26K files, 75 segments)

| Pattern | Category | tgrep med | rg med | Speedup |
|---------|----------|-----------|--------|---------|
| RareNeedle | rare | 150ms | 1316ms | **8.8x faster** |
| KeystoneTrigram | rare | 157ms | 1273ms | **8.1x faster** |
| NonexistentXyz123 | rare | 166ms | 1566ms | **9.4x faster** |
| QihseOptimization | rare | 173ms | 1453ms | **8.4x faster** |
| DsmilHashIndex | rare | 186ms | 1541ms | **8.3x faster** |
| TgrepSegmentWriter | rare | 143ms | 1361ms | **9.5x faster** |
| AtomicWriteFsync | rare | 165ms | 1347ms | **8.2x faster** |
| WalCheckpointReplay | rare | 162ms | 1181ms | **7.3x faster** |
| Struct | broad | 233ms | 1141ms | **4.9x faster** |
| Fn Main | broad | 161ms | 1413ms | **8.8x faster** |
| Return | broad | 246ms | 1293ms | **5.3x faster** |
| Use Std | broad | 135ms | 1183ms | **8.8x faster** |
| Terminal (-w) | word | 1242ms | 1209ms | 1.0x slower |
| Struct (-w) | word | 1196ms | 1167ms | 1.0x slower |
| main (-w) | word | 1583ms | 1499ms | 1.1x slower |
| nonexistent_xyz (-w) | word | 2545ms | 2219ms | 1.1x slower |
| terminal | case-insensitive | 1643ms | 1511ms | 1.1x slower |
| struct (-i) | case-insensitive | 1549ms | 1566ms | 1.0x faster |

**Key findings:**
- Rare/absent patterns: 7.3–9.5x faster than rg (avg 8.44x)
- Broad patterns: 4.9–8.8x faster than rg (avg 6.43x)
- Case-insensitive: within 10% of rg (streaming fallback, expected)
- Word search regression: hash index .thi files grew to 339MB (was 10MB) due to
  900K unique tokens per segment. Loading 339MB per search is slower than rg's
  direct scan. Future fix: mmap-based hash index or per-token bucket lookup.
- All 18 patterns produce identical results to rg (correctness verified)

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
Phase 10 (optimization DB)   [DONE]
    │
    ▼
Phase 11 (full suite + optimize) [DONE]
```

## Metrics

| Metric | Current | Target |
|--------|---------|--------|
| Test count | 54 pass | 50+ |
| Lines of code | ~6,500 | ~8,000 |
| Phases complete | 11/11 | 11/11 |
| grep baseline (NVMe) | 3,880ms | — |
| rg baseline (NVMe) | 653ms | — |
| tgrep cold (rare, indexed) | 143–236ms (9.9x rg) | <65ms (10x rg) |
| tgrep cold (broad, indexed) | 135–372ms (13x rg) | <65ms (10x rg) |
| tgrep cached (any pattern) | 7–31ms (44–198x rg) | <10ms |
| tgrep -w (word, cached) | 7–22ms (67–179x rg) | <10ms |
| tgrep case-insensitive (cached) | 23–26ms (42–47x rg) | <10ms |
| rg output equality | 18/18 patterns, 0 diff | — |

## Key Risks

1. **Trigram extraction duplication** — KEYSTONE extracts internally but doesn't export. Rust-side extraction is redundant work. Mitigation: use posting visitor to extract from KEYSTONE after finalize, skip Rust extraction.

2. **Path index is variable-size** — binary search requires linear scan to find entries. Mitigation: add a fixed-size secondary index (hash table or B-tree) in a later phase.

3. **rg compatibility** — matching rg's exact output, ignore rules, and exit codes. Mitigation: use `grep-printer` and `ignore` crates to inherit behavior.

4. **ZFS ARC invalidation** — cold-cache benchmarks on ZFS can't use `drop_caches`. Mitigation: export/drop the ARC dataset or use a fresh dataset.

5. **Large file handling** — files > 64 MiB should be scan-only, not indexed. Mitigation: cap indexed file size, mark large files in segment metadata.
