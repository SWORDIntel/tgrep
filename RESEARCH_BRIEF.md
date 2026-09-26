# tgrep Deep Research Brief

## Objective

Identify and evaluate every viable optimization to make tgrep the fastest
persistent search CLI on the planet — beating ripgrep on repeated selective
searches by 10x (baseline) to 100x (stretch) while matching its output,
ignore behavior, and exit codes.

---

## Current Performance Profile

### Measured A/B benchmark (26K files, 1.15 GB, ZFS, SSE4.2)

| Pattern     | With prefetch | Without prefetch | Speedup | rg baseline |
|-------------|-------------|-----------------|---------|-------------|
| Struct      | 13.4s       | 26.3s           | 2.0x    | 8.2s        |
| Return      | 10.7s       | 35.2s           | 3.3x    | 5.5s        |
| terminal    | 14.3s       | 32.5s           | 2.3x    | 28.0s       |
| RareNeedle  | 22.5s       | 23.2s           | 1.0x    | 12.0s       |

### Previous benchmarks (before .qwi sidecar regression)

| Category          | Cold avg speedup | Warm avg speedup |
|-------------------|---------------:|----------------:|
| Rare             | 8.9x           | 31.7x           |
| Broad            | 5.8x           | 35.3x           |
| Word             | 5.7x           | 33.7x           |
| Case-insensitive | 0.9x           | 32.5x           |

### Key observation

Current absolute times (10–22s) are 10–25x slower than previous benchmarks
(0.17–0.87s). Root cause: `.qwi` whole-word mmap sidecars are failing to save
during index build (`word_index_save failed (rc=-1)`), forcing fallback to the
slower trigram-only path. Fixing this alone should restore 10x+ performance.

---

## Research Areas

### 1. Fix QIHSE word-index save failures (BLOCKING)

**Problem:** `word_index_save failed (rc=-1)` appears during every index build.
The `.qwi` mmap sidecar files are not being written. All word-mode searches
(`-w`) and the mmap fast path are degraded.

**Investigate:**
- QIHSE `word_index_save` return path — what condition returns -1?
- Is the QIHSE B+ tree handle being closed/invalidated before save?
- Is the state directory path being passed correctly to the C wrapper?
- Are file descriptor limits or permissions involved?
- Compare the working build (commit `fc1ff86`) against current — what
  changed in the QIHSE wrapper or build path?

**Success criteria:** `.qwi` files appear in the state directory after build,
and `-w` searches use the mmap fast path instead of falling back to trigram
verification.

**Expected impact:** Restore 10–30x speedup on word searches.

---

### 2. Index compaction and segment strategy

**Problem:** Current build produces 96 segments for 26K files. Previous
working build had 75. More segments = more open file descriptors, more
posting-list merges, slower candidate intersection.

**Investigate:**
- Optimal segment count vs segment size tradeoff
- Merge segments after build (like LSM compaction)
- Target: 8–16 segments for 26K files (1.15 GB)
- Memory-mapped segment access vs file-per-segment
- Single flat file with offset index vs many `.tgs` files
- Impact of segment count on cold search (more files to open = more syscalls)

**Success criteria:** Post-build compaction reduces 96 segments to <16 with
no loss of search correctness. Cold search improves 20–40%.

---

### 3. SIMD acceleration (AVX2 / AVX-512)

**Problem:** Current machine only has SSE4.2. `compile.sh` detects this
correctly, but AVX2/AVX-512 paths are untested and unbenchmarked.

**Investigate:**
- Test on AVX2-capable hardware (Haswell+, most modern x86)
- Test on AVX-512 hardware (Skylake-X, ICX, Zen4)
- Benchmark KEYSTONE trigram extraction with AVX2 shuffle vs SSE4.2
- Benchmark KEYSTONE linear search with AVX-512 VBMI
- Function multiversioning: compile multiple versions, dispatch at runtime
  via `__builtin_cpu_supports()` — avoids requiring AVX at load time
- Measure: index build speed, candidate intersection speed, search speed

**Success criteria:** On AVX2 hardware, 1.5–2x faster index build and 1.3x
faster search vs SSE4.2. On AVX-512, 2–3x faster build and 1.5x faster search.

---

### 4. Advanced prefetch strategies

**Current:** `posix_fadvise(WILLNEED)` for all candidates, fire-and-forget.

**Investigate:**

#### 4a. Ranked prefetch
- Sort candidates by trigram frequency score (rarest trigram match first)
- Prefetch highest-ranked files first — they're most likely to match
- Low-ranked files (common trigrams, likely false positives) prefetch last
- If verification finds matches early, can cancel remaining prefetches

#### 4b. Incremental prefetch
- Start prefetching as soon as the FIRST trigram posting list is read,
  not after full intersection
- Pipeline: read posting list 1 → prefetch its files → read posting list 2
  → prefetch intersection delta → ...
- Overlaps posting-list I/O with file I/O

#### 4c. io_uring async reads
- Use `io_uring` instead of `posix_fadvise` for true async file reads
- Submit read requests for all candidates immediately
- Consume results as verification proceeds
- Linux 5.1+ only — fallback to posix_fadvise on older kernels

#### 4d. Readahead instead of fadvise
- `readahead()` (Linux-specific) may be more aggressive than
  `posix_fadvise(WILLNEED)` for sequential access patterns
- Benchmark both on ZFS vs ext4 vs XFS

**Success criteria:** Ranked prefetch reduces cold broad-pattern search by
additional 20–30% over current uniform prefetch. io_uring adds another 10–15%.

---

### 5. Index-trust expansion

**Current:** `-w -l` with unchanged files (mtime+size match) returns directly
from index without reading file content.

**Investigate:**
- Extend index-trust to `-l` (non-word) mode: if file unchanged since index
  build, and the trigram index says it contains the pattern, trust it
- Risk: false positives if trigram index has stale data — but mtime+size
  check should catch modifications
- Extend to `-i -l` (case-insensitive): normalize trigrams at index time
- Extend to `-c` (count): store per-file match counts in index for unchanged
  files — return count without reading
- Extend to `-n` (line numbers): store line offsets in index for unchanged
  files — return matches with line numbers without reading

**Success criteria:** `-l` on unchanged corpus returns in <50ms (index lookup
only, zero file reads). `-c` on unchanged corpus returns in <100ms.

**Expected impact:** 10–100x on warm indexes for `-l`, `-c`, `-w -l`.

---

### 6. Multi-level indexing (bigram + trigram)

**Current:** Trigram-only index. Short patterns (1–2 chars) can't use the
trigram index and fall back to full scan.

**Investigate:**
- Add bigram index for 2-character patterns
- Add unigram (single char) frequency table for 1-character patterns
- Bigram index is ~2x larger than trigram but enables 2-char search
- Combined bigram+trigram index: use bigram for short patterns, trigram for
  3+ char patterns
- Evaluate: is the extra index size worth it? How common are 1–2 char
  searches in real usage?

**Success criteria:** 2-char patterns use index instead of full scan.
Bigram index adds <30% to index build time and <50% to index size.

---

### 7. Bloom filter pre-filtering

**Concept:** For each file, store a Bloom filter of its trigrams. During
candidate selection, check the Bloom filter before opening the posting list.

**Investigate:**
- Per-file Bloom filter (1–2 KB per file, 26K files = 26–52 MB)
- Global Bloom filter for the whole corpus (check if trigram exists anywhere)
- False positive rate vs memory tradeoff
- Can skip posting-list traversal entirely for absent trigrams
- Combine with index-trust: Bloom filter check is O(1), no file I/O

**Success criteria:** Rare/absent patterns skip 90%+ of posting-list work.
Bloom filter adds <10% to index size and <5% to build time.

---

### 8. File clustering by trigram similarity

**Concept:** Group files with similar trigram profiles into the same segment.
When a pattern matches one file in a cluster, other files in the cluster are
likely matches too — prefetch the whole cluster.

**Investigate:**
- K-means or LSH clustering on trigram histograms
- Cluster size: 50–200 files per cluster
- Store cluster ID in index, prefetch by cluster
- Evaluate: does clustering improve prefetch hit rate?
- Risk: clustering adds build-time complexity

**Success criteria:** Clustered prefetch improves cold search 10–20% over
uniform prefetch on broad patterns.

---

### 9. ZFS-specific tuning

**Problem:** Corpus is on ZFS (`/rpool/scratch/tgrep_corpus`). ZFS ARC
behavior, recordsize, and compression affect search performance.

**Investigate:**
- ZFS recordsize: default 128K. For search workloads, smaller recordsize
  (16K–64K) may reduce read amplification for small files
- ARC size: ensure ARC is large enough to cache the working set
- ZFS compression: lz4 vs zstd-1 for the corpus — compressed reads may be
  faster (less I/O) or slower (decompression overhead)
- `primarycache=all` vs `primarycache=metadata` on the dataset
- `logbias=throughput` for the index state directory
- Benchmark: same corpus on ZFS vs ext4 vs tmpfs — isolate ZFS overhead

**Success criteria:** ZFS tuning improves cold search 10–30% without
changing the index format.

---

### 10. Memory-mapped index access

**Current:** Index segments are read via `read()` syscalls.

**Investigate:**
- `mmap()` the entire index into virtual memory
- Kernel handles paging — only touched pages are loaded
- Enables `madvise(MADV_WILLNEED)` for posting lists before traversal
- Enables `madvise(MADV_SEQUENTIAL)` for sequential posting-list reads
- Avoids `read()` syscall overhead (one syscall per read vs zero for mmap)
- Risk: address space limits on 32-bit (not a concern on 64-bit)
- Evaluate: mmap vs read for random-access posting lists

**Success criteria:** mmap-based index access reduces cold index load by
30–50% and eliminates per-segment open overhead.

---

### 11. Parallel index loading

**Current:** Index segments loaded sequentially at search start.

**Investigate:**
- Load all segments in parallel using a thread pool
- Parse segment headers concurrently
- Merge posting lists after all segments loaded
- Benchmark: 96 segments loaded in parallel vs sequential
- Risk: I/O contention if all threads hit disk simultaneously
- Mitigate: limit parallelism to 4–8 threads, use io_uring for async I/O

**Success criteria:** Index load time drops from X to X/N where N is thread
count (up to I/O bandwidth limit).

---

### 12. Candidate ranking and early termination

**Current:** All candidates are verified in parallel, no early termination.

**Investigate:**
- Rank candidates by trigram frequency score (rarest trigram match = highest
  rank)
- Verify highest-ranked first
- For `-l` mode: once enough matches found, skip remaining candidates
  (if user only needs to know "does it exist")
- For `-c` mode: verify in rank order, stop when count is determined
- Evaluate: how often do users need ALL matches vs just "any match"?
- Risk: changes output ordering — may break ripgrep compatibility

**Success criteria:** Early termination reduces verification time 30–50%
for patterns with many matches.

---

### 13. Incremental index updates

**Current:** Full rebuild required when corpus changes.

**Investigate:**
- Detect changed files (mtime + size) since last build
- Re-index only changed files
- Append new segments for changed files
- Mark old segments for changed files as stale
- Periodic compaction merges and removes stale entries
- WAL for crash safety during incremental updates
- Evaluate: how common is incremental update in practice? Most searches are
  on stable codebases.

**Success criteria:** Incremental update of 100 changed files completes in
<5s (vs 10min full rebuild). No search correctness regression.

---

### 14. Hot/cold segment separation

**Concept:** Frequently-searched patterns have "hot" posting lists. Separate
hot posting lists into a dedicated segment for faster access.

**Investigate:**
- Track search frequency per trigram (in the QIHSE optimization data)
- Move hot trigrams to a dedicated "hot" segment
- Hot segment is smaller, fits in page cache, loads faster
- Cold segments are only loaded when needed
- Evaluate: does access pattern justify the complexity?

**Success criteria:** Hot segment reduces cold search for common patterns
by 20–40%.

---

### 15. Compression of posting lists

**Current:** Posting lists stored as arrays of file IDs.

**Investigate:**
- Delta-encode file IDs (consecutive IDs are close in value)
- Varint encoding for delta-encoded IDs (1–5 bytes per ID vs 4 bytes fixed)
- PFor-Delta encoding (patched frame-of-reference) for bulk decode
- SIMD-accelerated delta decoding (AVX2 has `vpshufb` for fast varint)
- Evaluate: posting list size reduction (expect 3–5x) vs decode overhead
- Benchmark: smaller posting lists = faster I/O, but decode adds CPU

**Success criteria:** Posting list size reduced 3–5x. Decode overhead <10%
of I/O savings. Net cold search improvement 15–25%.

---

### 16. grep-printer output optimization

**Current:** Output via grep-printer, which is correct but not optimized
for tgrep's use case.

**Investigate:**
- Buffer output in large chunks (64KB+) instead of line-by-line
- Pre-sort matched files by path for consistent output ordering
- Parallel output for `-l` mode (collect all matches, sort, print)
- Evaluate: is output a bottleneck? For 238 matches, probably not. For
  10K+ matches, possibly.

**Success criteria:** Output of 10K matches completes in <100ms.

---

### 17. Cross-filesystem index portability

**Problem:** Index state is in `/tmp` which is ephemeral. Reboot loses it.

**Investigate:**
- Default index location: `~/.cache/tgrep/` or `~/.local/share/tgrep/`
- Portable index format (no absolute paths — store relative paths + corpus
  root hash)
- Index relocation: move index to different corpus path without rebuild
- Multiple corpus support: one index per corpus, keyed by path hash
- Auto-detect: if corpus path matches a stored index, use it

**Success criteria:** Index survives reboot. Index is portable across
machines with the same corpus. Multiple corpora supported.

---

## Prioritization

| Priority | Research area                        | Expected impact | Effort |
|---------|--------------------------------------|----------------|--------|
| P0      | 1. Fix .qwi word-index save failures | 10–30x          | Low    |
| P0      | 17. Cross-filesystem index location  | UX critical     | Low    |
| P1      | 2. Index compaction                  | 20–40%          | Med    |
| P1      | 5. Index-trust expansion             | 10–100x (warm)  | Med    |
| P1      | 10. Memory-mapped index access       | 30–50% (cold)   | Med    |
| P1      | 4a. Ranked prefetch                  | 20–30% (cold)   | Low    |
| P2      | 15. Posting list compression         | 15–25%          | Med    |
| P2      | 3. SIMD acceleration                 | 1.5–3x (build)  | Med    |
| P2      | 11. Parallel index loading           | N-fold (load)   | Low    |
| P2      | 4c. io_uring async reads             | 10–15% (cold)   | Med    |
| P3      | 7. Bloom filter pre-filtering        | 10–20% (rare)   | Med    |
| P3      | 12. Candidate ranking + early term   | 30–50% (broad)  | Med    |
| P3      | 9. ZFS tuning                        | 10–30%          | Low    |
| P4      | 6. Bigram index                      | 2-char support  | Med    |
| P4      | 8. File clustering                   | 10–20%          | High   |
| P4      | 13. Incremental updates              | UX improvement  | High   |
| P4      | 14. Hot/cold segments                | 20–40% (hot)    | High   |
| P4      | 16. Output optimization              | <100ms          | Low    |

---

## Methodology

For each research area:

1. **Baseline:** Measure current performance with the existing index
2. **Implement:** Build a prototype of the optimization
3. **Benchmark:** A/B test with and without the optimization (3+ trials)
4. **Verify:** Confirm search correctness (output matches `rg` exactly)
5. **Document:** Record findings, code changes, and benchmark data
6. **Decide:** Ship, iterate, or abandon based on impact/effort ratio

### Benchmark protocol

- **Cold cache:** Clear tgrep search cache before each trial. Note: cannot
  drop Linux page cache without root — document this limitation.
- **Warm cache:** Run search twice, measure second run.
- **Trials:** Minimum 3 per pattern, report median.
- **Patterns:** Use the established 19-pattern suite (rare, broad, word,
  case-insensitive).
- **Corpus:** 26K files, 1.15 GB on `/rpool/scratch/tgrep_corpus`.
- **Comparison:** Always compare against `rg -j 4` on the same corpus.
- **Hardware:** Document CPU, RAM, disk type, filesystem.

---

## Hardware Test Matrix

| Platform         | SIMD      | Use                              |
|-----------------|-----------|----------------------------------|
| Current (Zen?)  | SSE4.2    | Baseline, regression testing     |
| AVX2 machine   | AVX2      | SIMD benchmarking                |
| AVX-512 machine| AVX-512   | Maximum SIMD benchmarking         |
| ARM64 (Neon)   | NEON      | Portability testing              |
| Low-RAM machine| Any       | Memory constraint testing        |

---

## Success Metrics

| Metric                      | Current  | Target   | Stretch  |
|-----------------------------|----------|----------|----------|
| Cold rare search speedup    | 8.9x*    | 10x      | 100x     |
| Cold broad search speedup   | 5.8x*    | 10x      | 50x      |
| Warm search speedup         | 31–35x*  | 30x      | 100x     |
| Broad search vs rg          | ~1x      | <1.1x    | <1.0x    |
| Index build time            | 10 min   | 5 min    | 2 min    |
| Index size                  | ~2 GB    | 1 GB     | 500 MB   |
| Index load time             | ~50ms    | <20ms    | <10ms    |
| Search correctness          | 100%     | 100%     | 100%     |

*Previous benchmarks before .qwi regression. Current numbers are degraded.

---

## Deliverables

1. Fixed `.qwi` word-index save path (P0)
2. Persistent index location (`~/.local/share/tgrep/`) (P0)
3. Post-build segment compaction (P1)
4. Index-trust expansion for `-l` and `-c` (P1)
5. mmap-based index access (P1)
6. Ranked prefetch (P1)
7. Posting list compression (P2)
8. AVX2/AVX-512 benchmarks on capable hardware (P2)
9. Updated benchmark suite with all optimizations (final)
10. Updated BENCHMARKS.md, ROADMAP.md, ARCHITECTURE.md (final)

---

## References

- tgrep repo: `/home/john/tgrep`
- KEYSTONE: `/home/john/Documents/KEYSTONE`
- QIHSE: `/home/john/Documents/QIHSE`
- Corpus: `/rpool/scratch/tgrep_corpus` (26K files, 1.15 GB)
- Index state: `/tmp/tgrep_qwi_mmap` (ephemeral — see P0 item 17)
- Previous benchmark results: `benchmarks/benchmark_results.csv`
- Architecture: `ARCHITECTURE.md`
- Roadmap: `ROADMAP.md`
- Benchmark methodology: `BENCHMARKS.md`
