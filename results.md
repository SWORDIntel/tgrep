# SITREP
The tgrep indexer is currently 10–25× slower than expected on a 26K‐file corpus (1.15 GB, ZFS, SSE4.2), due to a missing **.qwi** (word-index) file and fallback to slow 
trigram-only search. The objective is to restore and exceed prior performance (10×–100× speedups over ripgrep) while matching its semantics. The critical blocker is the `.qwi` 
save failure. Key issues: excessive index segments (96 vs ~75), no AVX use on capable hardware, naive prefetch I/O, lack of index trusts, and no compression. This creates high 
I/O and CPU overhead (millions of small reads, many file opens). We assume a stable corpus (mtime+size mostly unchanged) and modern Linux (5.x+), and we target code paths in the 
C/C++ tgrep/KeyStone/QIHSE code.
# EXECUTION
- **1. Fix QIHSE word-index save (P0, blocking):** Trace the `QIHSE::word_index_save` call. Confirm the state directory is correct and file descriptors are open with write 
permissions. Compare commits (e.g. revert wrapper changes since `fc1ff86`). Ensure the B+ tree is open when saving. A likely fix: explicitly open the sidecar file path before 
closing the tree handle. Example C++ fix (pseudo):
  ```cpp
  // After building word index B+ tree:
  if (tree.is_open()) { std::string qwi_path = state_dir + "/index.qwi"; FILE *f = fopen(qwi_path.c_str(), "wb"); if (!f) perror("open .qwi"); tree.save(f); // custom save method 
      fclose(f);
  }
  ``` Verify that `.qwi` files appear. With the fix, **-w** searches should use the memory‐mapped word index instead of fallback. - **2. Index compaction (P1):** Merge small 
index segments post-build into larger ones to reduce file count. For example, read all `.tgs` segments and append in sorted order to a few larger files. This is akin to LSM 
compaction: it limits “number of SSTables” consulted. A strategy: compute a sorted posting list per term across segments, write 8–16 new segments. Example shell step 
(conceptual):
  ```sh
  # Hypothetical merge script
  tgrep-merge-segments --input-dir /tmp/tgrep_index --output-dir /tmp/tgrep_index_compacted ``` Or implement within `index-build`: buffer postings and flush at larger thresholds. 
  The goal is <16 segments for 26K files. Fewer segments cut open-file calls and merging cost.
- **3. SIMD acceleration (P2):** Enable AVX2/AVX-512 code paths for heavy loops (trigram extraction, candidate intersection, linear scans). Build multiversion binaries: use 
`__builtin_cpu_supports()` at runtime to select the best code path. Example compile flags:
  ```sh CFLAGS="-O3 -march=native" ./configure && make -j ``` Or one-liners: `gcc -O3 -mavx2 -o tgrep_avx2 src/*.c`, and similarly for AVX-512. Benchmarks show AVX2 can be ~7× 
  faster on string compare; we expect ~1.5–2× on index build and ~1.3× on search. Validate on Haswell/Zen: run `tgrep` with AVX2 vs SSE4.2 on key queries.
- **4. Advanced prefetch:** - *4a. Ranked prefetch (P1):* Score each file by its rarest trigram in the query and sort candidates descending. Prefetch 
  (`posix_fadvise(fd,OFF,LEN,POSIX_FADV_WILLNEED)`) in that order. In C++:
    ```cpp sort(candidates.begin(), candidates.end(), [](File f1, File f2){ return f1.score > f2.score;
    });
    for (auto &f: candidates) posix_fadvise(f.fd, 0, 0, POSIX_FADV_WILLNEED); ``` Early verification of high-score files can cancel the rest. - *4b. Incremental prefetch (P2):* 
  Instead of waiting for full intersection, pipeline posting-list reads and prefetch chunks as soon as possible. Pseudocode:
    ```cpp vector<list<File>> postings = load_posting_lists(trigrams); vector<File> candidates = postings[0]; for (int i = 1; i < postings.size(); i++) { 
        prefetch_files(candidates); candidates = intersect(candidates, postings[i]);
    }
    prefetch_files(candidates); // final prefetch ``` This overlaps disk reads with I/O. - *4c. io_uring async reads (P2):* Use Linux `io_uring` (kernel 5.1+) to issue all file 
  reads concurrently. For example, with liburing or Rust’s uring-file:
    ```cpp io_uring ring; io_uring_queue_init(1024, &ring, 0); for (auto &f: candidates) { io_uring_prep_read(&cqe, f.fd, buf, buf_size, 0); io_uring_sqe_set_data(&sqe, f.ptr);
    }
    io_uring_submit(&ring);
    // then reap completions as verification proceeds
    ``` io_uring excels at overlapping I/O. Fallback to `posix_fadvise` on older kernels (choose at runtime). - *4d. Readahead vs fadvise:* On Linux, `readahead(fd, off, len)` 
  internally calls `vfs_fadvise(..., POSIX_FADV_WILLNEED)`. In practice `readahead()` is a thin wrapper with extra error checks. We can optionally use `readahead()` for 
  sequential access; but results should be similar. Test both on ZFS/ext4.
- **5. Index-trust expansion (P1):** For unchanged files, skip content I/O. Store file `mtime+size` in the index. If `-l` (files-only) is requested and the trigram index says 
“contains term” and file unchanged, declare match without reading file. For `-c` (count), store per-file match counts or line offsets in index. For example:
  ```cpp if (file.mtime == index_entry.mtime && file.size == index_entry.size) { if (mode == COUNT) { print index_entry.count; } else if (mode == FILES) { print file.name; } 
      continue; // skip file I/O
  }
  ``` Similarly, for `-i -l` store a lowercased index (normalize trigrams at build). This yields ~10–100× speedups on warm indexes (zero file reads). - **6. Multi-level indexing 
(P4):** Add bigram (2-gram) index for short patterns. Maintain both unigram and bigram tables (char→files, digram→files) alongside trigrams. At query time: if pattern length = 1 
use the unigram list; if 2 use bigram list; otherwise trigram. Bigrams double index size but cover short queries. Existing research notes bigrams have few keys but huge lists, so 
use them only when needed. For 2-char queries like “of”, we’d look up the bigram “of” posting list. Validate: two-char queries no longer full-scan, and measure build-size 
increase (should be <50%). - **7. Bloom filter pre-filtering (P3):** Build a Bloom filter per file for its trigram set (e.g. 1024–2048 bits). At query time, for each trigram 
required, check each candidate’s Bloom filter bitmask first: if the bit is 0 for any trigram, skip the file entirely. This avoids opening/posting-list for absent terms. Also a 
global Bloom filter can quickly detect completely absent queries. Bloom filters are O(1) to check and add ~0–5% to index size. Search literature shows per-SSTable Bloom filters 
skip unnecessary reads. Aim for a false-positive rate <1%; tune bits-per-entry accordingly. - **8. File clustering (P4):** Use K-means or LSH on files’ trigram-histogram vectors 
to group similar files (e.g. 50–200 files per cluster). Store a cluster ID. When any file in a cluster matches early, prefetch others in that cluster, as they likely share 
content. This could improve prefetch efficiency on broad patterns. Validate by measuring prefetch hit rate. (This is complex and may have diminishing returns.) - **9. 
ZFS-specific tuning (P3):** Since the corpus is on ZFS, optimize recordsize and ARC. For SSD-backed vdevs, set a small recordsize (e.g. 16K–32K) to reduce I/O amplification. 
Example:
  ```sh sudo zfs set recordsize=16K rpool/scratch ``` Set `primarycache=all` to cache data. If data compresses well (source code is text), enable `compression=lz4` or `zstd`; 
  smaller I/O may benefit from compression. Example: ```sh sudo zfs set compression=lz4 rpool/scratch ``` Compare performance on ZFS vs ext4/tmpfs. Ensure host has ample RAM so 
  ZFS ARC can hold the working set. If low-RAM, consider a small L2ARC on NVMe. These tweaks aim for ~10–30% I/O reduction without changing tgrep logic.
- **10. Memory-mapped index access (P1):** Instead of using `read()` syscalls for each posting chunk, `mmap()` the entire index file(s). After `mmap`, accessing a posting is a 
simple memory read; if it’s in cache, it costs nanoseconds vs microseconds for a syscall. For example, in C:
  ```cpp int fd = open("index.tgs", O_RDONLY); size_t size = lseek(fd, 0, SEEK_END); void* map = mmap(NULL, size, PROT_READ, MAP_SHARED, fd, 0);
  // Then use (uint32_t*)map + offset for posting lists
  ``` Use `madvise(MADV_WILLNEED)` on segments if desired. This eliminates per-segment open/read overhead and can cut index load time by ~30–50%. (Watch 32-bit address limits; 
  assume 64-bit OS.)
- **11. Parallel index loading (P2):** Instead of loading segments sequentially, spawn a thread pool (e.g. `std::async` or `std::thread`) to open and mmap each of the ~96 
segments concurrently. Ensure not to oversubscribe: cap threads to 4–8. Example sketch:
  ```cpp std::vector<std::future<Segment>> futures; for (auto &segfile : segment_files) { futures.push_back(std::async(std::launch::async, [&]{ return load_segment(segfile);
      }));
  }
  for (auto &fut : futures) segments.push_back(fut.get()); ``` This N-folds index load time (up to I/O limits). Use `io_uring` if implemented (see item 4). - **12. Candidate 
ranking & early termination (P3):** Similar to 4a ranking. After intersection, verify highest-score candidates first. For `-l` or `-q` modes (“exists” or list-only), stop once 
one match is found. For `-c`, stop after exhausting all candidates. This prunes work when many candidates have matches. Tradeoff: output order may change (but sorting by path can 
restore consistent order). - **13. Incremental index updates (P4):** Maintain a small metadata (WAL) of the last build state. On rebuild, only re-index files that changed (mtime 
or size). Append these to new segment(s) and mark old segments stale for those files. Periodically run compaction to purge stale entries. Use a simple WAL log to record partial 
builds for crash recovery. Implementing this gives near-instant updates on a few changed files (<5 s for 100 files vs ~10 min full rebuild). Test by touching a few files and 
rerunning index build. - **14. Hot/cold segment separation (P4):** Track global search frequencies of trigrams (e.g. use counters). In index build, write “hot” trigrams (and 
their posting lists) into a special small segment, and “cold” ones into others. The hot segment stays resident in RAM/cache. On queries, load hot segment first; skip cold 
segments when queries use only hot trigrams. This might accelerate common-pattern searches. Evaluate complexity vs benefit. - **15. Posting-list compression (P2):** Instead of 
raw 32-bit file IDs, store delta-encoded IDs with varint or PFor-Delta coding. For example, for list `[5, 9, 20, 21]`, encode deltas `[5,4,11,1]` with variable-byte. Many index 
compression papers report ~3–5× size reduction for docID lists with fast decode. SIMD-friendly codecs (e.g. Varint-GB, Stream-VByte) can decode quickly. Implement a compression 
step when writing segments, and decode in-memory during search. Test that decode overhead is <10% of I/O savings. - **16. grep-printer output (P4):** Buffer and sort output. Use 
a large write buffer (e.g. `setvbuf(stdout, NULL, _IOFBF, 65536)`). For `-l`, accumulate matching file names in a vector, sort by path, then print. This can improve huge-output 
cases; aim for printing 10K matches under 100 ms. - **17. Cross-filesystem index (P0):** Move index from `/tmp` to an XDG-compliant user directory. Per XDG spec, use 
`$XDG_STATE_HOME` or `~/.local/state/tgrep/` for persistent state (or `$XDG_CACHE_HOME` if appropriate). Make the index portable by storing paths relative to the corpus root and 
hashing the root path. On startup, detect if a valid index exists (matching corpus path hash) and use it. For example:
  ```sh export TGPERF_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/tgrep" ``` Ensure the index survives reboot and can be shared across machines with identical corpora (e.g. via 
  portable paths).
# CONTINGENCY (FLANK)
- *QIHSE Save Fails:* If `.qwi` still fails, fall back temporarily to non-word mode (document in logs) and alert the user. Use verbose logging to capture `errno`. As a 
workaround, disable word-index mode (`--no-word-index`) until fixed. - *AVX Detection:* If runtime CPU check mis-detects, add a fallback to the SSE4.2 codepath. For ARM64, ensure 
NEON codepath is compiled or disabled gracefully. - *io_uring Unavailable:* On kernels <5.1 or if `io_uring_setup` fails, detect at startup and revert to synchronous `pread` with 
aggressive `posix_fadvise`. - *Excessive Memory Use:* New features (bitmaps, bloom filters, bigram index) enlarge memory. Monitor RSS; if too high, allow user to disable optional 
features via flags. - *Ranked Prefetch Wrong Order:* If rankings cause missed matches (due to ordering assumptions), revert to normal order. Always verify correctness (output 
must match `rg`). - *Failed Compaction:* If segment merging introduces errors, keep a rollback: do not delete original segments until new ones pass verification.
# COMPLIANCE & REQS
- **Platform:** Linux x86_64 (Zen SSE4.2 baseline; test on Haswell/AVX2, Skylake/AVX-512, ARM64 for NEON). - **Dependencies:** Ensure `liburing` (kernel 5.1+) for io_uring; 
pthreads/C++11 for threading; QIHSE and Keystone libraries at latest stable versions. - **Security:** Follow POSIX best practices (no unsafe `mmap(PROT_WRITE|PROT_EXEC)`). 
Validate all file paths (no symlink/hardlink attacks on index file location). Avoid any `system()` calls with user data. - **CVEs/Lint:** Use up-to-date compilers and libs to 
avoid known issues. Run `scan-build` and `clang-tidy` on code changes. Check that IO routines handle truncated reads. - **Permissions:** Index directories under user home (not 
root). If changing ZFS dataset settings, require sudo. - **Data Handling:** Index contains only file paths and trigram data; no PII. Comply with XDG spec for configuration and 
cache locations.
# VALIDATION
- **Regression Tests:** Run `tgrep` and `rg -j4` on the same corpus with all flags (`-l -c -n`, case-insensitive, regex, fixed string) on diverse patterns (use the 19-pattern 
suite). Ensure identical output and exit codes in all cases. - **Performance Benchmarks:** For each optimization, measure cold and warm search times (3+ runs, median) on the 
26K-file corpus. Compare *before vs after* and against `rg`. Use `time` or high-resolution timers. Key metrics:
  - Build time (target <5 min). - Index size (target ~1 GB). - Search times for “rare” and “broad” patterns. - **Specific tests:** - After #1, check `.qwi` exists and word 
  searches (`tgrep -w`) are 10–30× faster (warm). - After #2, confirm segment count (~<16) and 20–40% faster cold searches. - After #5, measure `tgrep -l pattern` on unchanged 
  files returns in <50 ms. - After #10, measure index load time drop (expect <20 ms). - After #17, simulate reboot: ensure index directory persists and is reused.
- **Correctness:** Use a diff tool (or `cmp`) to ensure `tgrep` vs `rg` output match exactly. Include tests for edge cases: binary files, giant files, symlinks, `--ignore-case`, 
`-w`, `-i`, `-c`, and empty-pattern behaviors. Failure indicators: wrong count, missing matches, crashes, or significant speed regressions.
### References
- Aerospike LSM compaction discussion (compaction merges files, bounding reads). - C++ AVX2 string compare 7× speedup benchmark. - Cursor.ai blog on n-gram indexing (bigrams have 
huge posting lists, trigrams sweet spot). - Inverted index Bloom filters skip files (avoid reads). - mmap vs read: 1M pread syscall ≈1s CPU; mmap drops to nanoseconds. - Linux 
io_uring designed for high-throughput file I/O. - Linux readahead → POSIX_FADV_WILLNEED (same effect). - ZFS tuning (for SSD, use 16K–32K recordsize to avoid write 
amplification). - XDG Base Dir spec: use `$XDG_STATE_HOME` or `~/.local/state` for persistent state (cache: `~/.cache`).
