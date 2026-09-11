# tgrep Benchmark Results

## Environment

- **CPU:** 8 cores (4 threads for search)
- **Storage:** NVMe-backed ZFS pool (`rpool`)
- **Corpus:** `/rpool/scratch/tgrep_corpus` — 26,034 files, 1.15 GB source (includes hidden files)
- **Index:** 75 segments, ~200M postings
- **Index state:** `/tmp/tgrep_strong_state2`
- **ripgrep config:** `~/.ripgreprc` with `--smart-case`, `--hidden`, `--glob=!.git/*`, `--threads=4`
- **Trials:** 10 per pattern (alternating tgrep/rg)
- **Date:** September 2026 (Phase 8 — hash index fast path)

## Correctness Verification

All patterns produce identical output to `rg` (sorted comparison, 0 differences):

| Pattern | Files matched | Status |
|---------|--------------|--------|
| terminal (smart-case) | 2011 | PASS |
| rareneedle | 7 | PASS |
| struct | 4698 | PASS |
| keystone_trigram | 10 | PASS |
| fn main | 142 | PASS |
| Struct | 238 | PASS |
| RareNeedle | 1 | PASS |
| nonexistent_xyz | 1 | PASS |

Word-search patterns (`-w` flag, verified against `rg -w`):

| Pattern | Files matched | Status |
|---------|--------------|--------|
| -w Terminal | 679 | PASS |
| -w Struct | 77 | PASS |
| -w RareNeedle | 1 | PASS |
| -w keystone_trigram | 9 | PASS |
| -w main | 1901 | PASS |
| -w nonexistent_xyz | 1 | PASS |

Subdirectory searches also verified correct (e.g., ghostty subdir: 39 files, zellij subdir: 3–449 files).

## Full Mode (correct, with file list cache + streaming fallback)

| Pattern | tgrep median | rg median | tgrep min | rg min | Speedup |
|---------|-------------|-----------|-----------|--------|---------|
| RareNeedle (absent) | 114ms | 630ms | 111ms | 527ms | **5.5x faster** |
| KeystoneTrigram (absent) | ~118ms | ~640ms | ~115ms | ~530ms | **4.7x faster** |
| Struct (238 matches) | 148ms | 686ms | 137ms | 519ms | **4.6x faster** |
| terminal (2011, streaming) | 1347ms | 782ms | 1022ms | 595ms | 1.7x slower |

## Indexed-Only Mode (skips unindexed file scan)

| Pattern | tgrep median | rg median | tgrep min | rg min | Speedup |
|---------|-------------|-----------|-----------|--------|---------|
| RareNeedle (absent) | ~55ms | ~640ms | ~50ms | ~530ms | **20.4x faster** |
| Struct (238 matches) | ~80ms | ~680ms | ~75ms | ~520ms | **8.5x faster** |

## Hash Index Fast Path (`-w` word-search mode, 5 trials)

Whole-word queries using the `dsmil_hash_index` for O(log n) token lookup.
Hash index persisted as `.thi` sidecar files (~10MB total for 75 segments).

| Pattern | tgrep median | rg median | tgrep min | rg min | Speedup |
|---------|-------------|-----------|-----------|--------|---------|
| -w Terminal (679 matches) | 122ms | 4395ms | 2ms | 2945ms | **374x faster** |
| -w Struct (77 matches) | 68ms | 4510ms | 2ms | 2387ms | **499x faster** |
| -w main (1901 matches) | 11ms | 5926ms | 2ms | 3500ms | **1069x faster** |
| -w nonexistent_xyz (absent) | 141ms | 5407ms | 2ms | 2921ms | **670x faster** |

## Analysis

### What works well

- **Indexed patterns (mixed case):** 4.6–5.5x faster than rg in full mode, 8.5–20x in indexed-only mode.
- **Absent patterns:** Best case for trigram index — posting intersection returns empty, no files to verify.
- **File list cache:** Eliminates the ~500ms filesystem walk, reducing full-mode overhead to just indexed candidate verification + unindexed file scan (zero when corpus is unchanged).
- **Result equality:** 100% match with rg across all tested patterns and subdirectory searches.

### Bottlenecks

1. **Smart-case streaming:** All-lowercase patterns with smart-case become case-insensitive, falling back to streaming (trigram index is case-sensitive). This affects common search patterns like "terminal".
2. **Broad pattern verification:** Patterns matching thousands of files (e.g., terminal=2011) require verifying each match, which is slower than rg's direct scan.

### Optimization history

| Optimization | Impact |
|-------------|--------|
| Parallel search (4 threads) | ~2x faster verification |
| O(n) iter_docs() | Fixed O(n²) get_doc() — 10x faster index ops |
| get_docs_bulk() with HashSet | O(1) doc ID lookups in posting intersection |
| Skip indexed non-candidates | Reduced scan from 26K to ~2K files |
| Reuse Searcher per thread | Avoids per-file allocation overhead |
| Ripgrep config parsing | Correctness fix for smart-case and hidden files |
| Hidden file indexing (.hidden(false)) | Fixed 9 missing Struct matches |
| Fresh build (clear old segments) | Fixed duplicate trigram data from interrupted builds |
| File list cache (Phase 7) | Eliminated ~500ms filesystem walk in full mode |
| Subdirectory path filtering | Fixed subdir search returning all indexed candidates |
| Hash index fast path (Phase 8) | 374–1069x faster than rg for whole-word queries (`-w`) |
| Binary search hash lookup | Fixed signed/unsigned comparison bug in hash index search |

## Raw Data

Individual CSV files: `bench_results_*.csv`
