# tgrep Security Review — Pre-Public-Release

Date: 2026-09-25. Two tracks: static review (all of src/, native/*.c, build/install
scripts, git history) + dynamic harness (25 state-file corruption mutations, hostile
search corpus, hostile-tree index build, cache subcommands, kill -9 recovery, env-hijack PoC).
Verdict: **NOT ready for public release.** No memory-safety finding is exploitable
for remote code execution, but a crafted index file can crash the process (DoS) and,
with a writable state dir, silently manipulate search results — which matters more
than usual because install.sh puts tgrep *in front of* `rg` itself.

## Fix status (2026-09-25, post-remediation pass)

All findings fixed and re-attacked with magic-preserving crafted files unless
noted. Verification: 61/61 tests, hostile corpus rebuild + search, crafted
evil-qsc (0xFFFFFFFF count) / evil-tgs (offset-order violation) now rejected
gracefully (streaming fallback, correct results), state dirs 0700, license =
AGPL-3.0-only.

- F1/F2 .qwi overflow + alloc amplification — FIXED (division-first validation in
  C; max_results capped at 1<<22).
- F3 tmp symlink/clobber — FIXED (pid-suffixed O_EXCL|O_NOFOLLOW 0600 in C;
  create_new in Rust).
- F5/F6/F7 store.rs offset/underflow/capacity — FIXED (checked validation of all
  header offsets + section ordering at open; saturating_sub; decode capacity
  bounded by remaining bytes).
- Footer checksum — FIXED (verified at open; memoized per (dev,ino,mtime,len)
  because segments are immutable and a search reopens hundreds of files;
  simple_checksum rewritten with chunks_exact after the naive loop measured
  ~55 MB/s and stalled segment-heavy commands).
- F8 cache reserve caps — FIXED (reserved bounded by remaining file bytes).
- F9 char-boundary panic — FIXED (truncate_pattern helper, both sites).
- F11 WAL trampoline — MITIGATED (null/zero-len key rejected; WAL parser now
  vendored and auditable).
- F12 ABI drift — FIXED (_Static_assert(sizeof==80) in C).
- F14 rg look-alike — MOSTLY FIXED (delegate probes --version and requires
  "ripgrep" in output). Residual, accepted: the probe itself executes the
  target once with no user arguments before refusing. Document TGREP_REAL_RG /
  ~/.config/tgrep/rg_path as trusted input.
- F16 vendor + de-hardcode — FIXED (vendor/KEYSTONE + vendor/QIHSE committed
  trees; build.rs, bench.rs, bench_suite.rs use manifest-relative paths).
- F17 LICENSE — FIXED (AGPL-3.0-only; Cargo.toml license field set).
- F18 uninstall doc path — FIXED.
- F15 build TOCTOU — FIXED (O_NONBLOCK open before read; FIFO swap no longer
  blocks the build).
- F10/F13 results-integrity design — OPEN, needs a product decision: `-l`
  cache hits and `-w` index-trust still answer without reading files, and the
  rg wrapper (install.sh) makes that unfalsifiable from the CLI. Options:
  don't install the rg wrapper by default / stderr notice when tgrep answers /
  cheap content verification for -l cache hits.
- Index eviction — OPEN, feature backlog (no command removes stale segments;
  tonight's testing bloated a state dir to 8+ GB with 1005 segments before a
  manual wipe).

## Trust model

State dir defaults to `~/.local/state/tgrep` (0755/0644). Findings marked
[state-dir] require a writable/shared state dir (`TGREP_STATE_DIR`/`XDG_STATE_HOME`
or account compromise). Everything else requires only files in a searched directory.

## Findings

| # | Sev | Where | What |
|---|-----|-------|------|
| F1 | HIGH | native/word_index_mmap.c:202,225-273 | `count * 24` integer overflow in .qwi validation → OOB mmap reads (SIGSEGV / adjacent-page disclosure) from a crafted `.qwi` [state-dir] |
| F2 | HIGH | src/search.rs:1188 → native.rs:1079 | `vec![0u64; qwi.size()]` amplifies F1's count into a 2^66-byte allocation abort on any `-w` search |
| F3 | MED | native/qihse_wrapper.c:44-50, src/cache_v2.rs:207-210 | Atomic-write temp files use fixed `.tmp` names, no `O_EXCL`/`O_NOFOLLOW` → symlink clobber of arbitrary user files + concurrent-writer races [state-dir] |
| F5 | MED-HIGH | src/store.rs:654 | `(dict_end - dict_start)` usize underflow → huge reservation abort from crafted `.tgs` (via plain `index status`) |
| F6 | MED-HIGH | src/store.rs:602-603 | `Vec::with_capacity(doc_freq)` with attacker u32 → 17 GB abort on any indexed search touching the segment |
| F7 | MED | src/store.rs:371-372,441-454,575-580 | Header offsets unvalidated vs `data.len()` (`checked_add` missing; `2^64-8` footer bypass) → slice panics on every invocation |
| F8 | MED | src/cache_v2.rs:294-301,348 | `reserve()` on attacker count from a 12-byte crafted cache (magic-preserving) → allocation abort |
| F9 | MED-LOW | src/cache.rs:190,537 | `&pattern[..27]` byte slicing can split a UTF-8 char → repeatable panic |
| F10 | MED | src/search.rs:335-355,577-599 | `-l` cache hits and `-w` index-trust path return results **without reading files** → poisoned state dir controls printed results |
| F13 | MED | src/rg_compat.rs:435-478, main.rs:121-127 | In rg-compat mode tgrep answers instead of real rg → index tampering becomes silent search-result tampering, unfalsifiable from the CLI |
| F14 | LOW | src/rg_compat.rs:57-100 | `TGREP_REAL_RG`/`rg_path`/PATH binary never verified as ripgrep (dynamically confirmed: arbitrary binary executes). Same-privilege persistence vector |
| F4 | LOW | word_index_mmap.c:166-180, store.rs:364 | mmap vs concurrent truncation → SIGBUS (uncatchable) |
| F15 | LOW | src/build.rs:378-391 | stat→read TOCTOU: file swapped to FIFO mid-build blocks forever; oversized growth reads unbounded. Walk itself is safe (no symlink follow, type-gated — dynamically confirmed: /etc/passwd symlink not indexed, FIFO skipped) |
| F11 | MED | src/native.rs:772-795 | WAL replay trampoline trusts C (ptr,len) pairs; the WAL parser itself lives OUTSIDE the repo — unauditable |
| F12 | LOW-MED | src/native.rs:361-364 | `OptimizationDb = [u8; 80]` must match C `pthread_mutex_t` layout; no static assertion |
| F16 | CRITICAL (release) | build.rs:17-30,58-71 | Build hardcodes `/home/john/Documents/KEYSTONE` + `/QIHSE` sources; headers not vendored → nobody else can build, and the parsers consuming attacker-facing `.thi`/`.qwi`/WAL are unreviewable |
| F17 | HIGH (release) | — | No LICENSE file |
| F18 | LOW | install.sh:64 | Uninstall doc says `~/.local/share/tgrep`; code uses `~/.local/state/tgrep` — stale data left behind |
| — | LOW | index lifecycle | No eviction/clear for segments (1.2 GB observed accumulating); `rebuild`/`compact` exist but nothing drops stale roots |
| — | LOW | src/cache.rs | Symlink loops get listed in file_list_cache.json (harmless, cosmetic) |

## Confirmed clean (dynamic + static)

25 corruption mutations across 5 state files (parse-reject → safe fallback);
hostile corpus (50 MB sparse, 10 MB line, invalid UTF-8+NUL, fake PE, FIFO,
unreadable, escape symlink NOT followed, symlink loop terminates); hostile-tree
build; cache subcommands under corruption; kill -9 build recovery; arg passing to
rg is argv-direct (no shell/injection); precache uses current_exe(); hash_wrapper/
keystone_wrapper/qihse_opt/cache_wrapper C bounds checks correct; Cargo.lock deps
clean of known-bad pins; no secrets in code or git history.

## Fix order for release

1. Vendor the KEYSTONE/QIHSE C sources + headers; remove every `/home/john/...`
   literal (build.rs, bench.rs:317, bench_suite.rs:505). (F16)
2. Add LICENSE (pick: MIT/Apache-2.0 — ripgrep-family deps are MIT/Unlicense). (F17)
3. .qwi: overflow-checked count validation in C + cap `max_results` at segment doc
   count. (F1/F2)
4. store.rs: `checked_add`-validate all header offsets at parse; `saturating_sub`
   at :654; cap posting decode by remaining bytes; verify the footer checksum on
   open (it is written but never checked). (F5-F7)
5. State-dir hardening: O_EXCL|O_NOFOLLOW + randomized tmp names; 0700 dir/0600
   files; cap cache reservations by remaining file length; char-boundary-safe
   pattern truncation. (F3, F8, F9)
6. Decide the rg-compat story: consider not installing the `rg` wrapper by
   default, or print a one-line notice when tgrep answers instead of real rg,
   and/or require cheap content verification for `-l` cache hits. (F10, F13, F14)
7. Backlog: WAL parser vendoring + trampoline pointer checks, OptimizationDb
   static assertion, build TOCTOU O_NONBLOCK, uninstall doc path, index eviction.

## Reproduce artifacts

Dynamic harness corpus + scripts: /tmp/tgrep_sec (volatile). Original corrupted-vs-
clean state files under /tmp/orig.bak. Env-hijack PoC: `TGREP_REAL_RG=<script> tgrep
--rg-compat …` → script executes (confirmed).
