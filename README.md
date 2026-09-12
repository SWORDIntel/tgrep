# tgrep

Persistent trigram-indexed search that drops in as a ripgrep accelerator.

tgrep builds a persistent trigram index over your codebase and uses it to
skip files that can't match your search pattern. When installed as an `rg`
wrapper, it transparently routes searches to the index when beneficial and
delegates to real ripgrep for everything else — all `rg` flags work
identically.

## Performance

Benchmarked on a 26K-file, 1.15 GB corpus (10 trials, cold cache):

| Category | vs ripgrep (cold) | vs ripgrep (warm) |
|----------|-------------------|-------------------|
| Rare patterns | **5.5–12.8x faster** | 12–54x faster |
| Broad patterns | **2.4–8.4x faster** | 24–42x faster |
| Word search (`-w`) | **1.3–10.7x faster** | 20–46x faster |
| Case-insensitive | within 10% | 29–36x faster |

Previously, `-w` word searches were 1.0–1.1x slower than rg. The index-trust
optimization (mtime-based verification skip) now makes them 4.8–10.7x faster.

See `BENCHMARKS.md` for full results and `benchmarks/graphs/` for visualizations.

## Quick start

```bash
# Install tgrep and create the rg wrapper
./install.sh

# Build an index for your codebase
tgrep index build /path/to/code

# Search — uses the index automatically
rg "pattern" /path/to/code          # faster than real rg
rg -w "word" /path/to/code          # much faster (hash index + index-trust)
rg -A 2 "pattern" /path/to/code     # delegates to real rg (unsupported flag)
rg --help                           # identical to real rg
rg --version                        # identical to real rg
```

## How it works

tgrep sits between you and ripgrep:

```
rg command
    │
    ▼
┌──────────────┐     indexed search?     ┌──────────────┐
│  tgrep router │───── yes ──────────────▶│  tgrep index │
└──────────────┘                          └──────────────┘
    │ no                                         │
    ▼                                            ▼
┌──────────────┐                          ┌──────────────┐
│  real ripgrep │◀──── results merged ────│  grep-printer │
└──────────────┘                          └──────────────┘
```

1. **Index exists and pattern benefits?** → tgrep uses the trigram index
   to find candidate files, then verifies matches with `grep-searcher`
   (same engine ripgrep uses).
2. **Unsupported flags or no index?** → delegates to real ripgrep with
   all original arguments. Output is identical.
3. **`-w` word search with index-trust?** → skips content verification
   for files whose mtime matches the file list cache (file unchanged
   since indexing). Changed files are still verified.

## Installation

### Prerequisites

- Rust (rustup/cargo)
- ripgrep (`rg`) installed and in PATH
- C compiler (gcc/clang) for native KEYSTONE/QIHSE extensions

### Install

```bash
git clone <repo-url> tgrep
cd tgrep
./install.sh
```

This will:
1. Build tgrep in release mode
2. Find your real `rg` binary
3. Install `tgrep` to `~/.local/bin/tgrep`
4. Create an `rg` wrapper at `~/.local/bin/rg`
5. Store the real rg path for delegation

Make sure `~/.local/bin` is in your `PATH` (check your shell config).

### Uninstall

```bash
./install.sh --uninstall
```

Restores direct ripgrep access. Index data in `~/.local/share/tgrep` is
preserved — remove it manually if desired.

## Usage

### Indexing

```bash
# Build a fresh index
tgrep index build /path/to/code

# Check index status
tgrep index status

# Recover from WAL after a crash
tgrep index recover
```

### Searching

After installation, use `rg` as normal — tgrep handles routing automatically.

You can also use `tgrep` directly:

```bash
tgrep "pattern" /path/to/code          # basic search
tgrep -w "word" /path/to/code          # whole-word (hash index fast path)
tgrep -l "pattern" /path/to/code       # files with matches
tgrep -c "pattern" /path/to/code       # count matches
tgrep -n "pattern" /path/to/code       # line numbers
tgrep -i "pattern" /path/to/code       # case-insensitive (streaming)
tgrep --explain "pattern" /path/to/code # show query plan
```

### Cache

tgrep caches `-l` (files-with-matches) results for instant repeat searches:

```bash
tgrep cache status       # show cache stats
tgrep cache clear        # clear all cached results
tgrep cache compact      # compact the cache
tgrep cache dashboard    # live cache + KEYSTONE telemetry
```

## Supported rg flags

tgrep handles these flags directly (using the index):

| Flag | Description |
|------|-------------|
| `-w, --word-regexp` | Whole-word search (hash index fast path) |
| `-l, --files-with-matches` | Print only filenames |
| `-c, --count` | Count matches per file |
| `-n, --line-number` | Show line numbers |
| `-q, --quiet` | Suppress output (exit code only) |
| `-i, --ignore-case` | Case-insensitive (streaming fallback) |
| `-S, --smart-case` | Smart case detection |
| `-F, --fixed-strings` | Literal string search |
| `-e, --regexp PATTERN` | Multiple patterns |
| `-j, --threads N` | Thread count |
| `--hidden` | Search hidden files |
| `-g, --glob GLOB` | File glob filter |
| `-H, --with-filename` | Print filename (default) |
| `-I, --no-filename` | Suppress filename |

All other flags (`-A`, `-B`, `-C`, `--json`, `--color`, `-r`, `-U`, `-P`,
`-z`, `-o`, `-p`, `--files`, `--type-list`, etc.) are delegated to real
ripgrep with identical behavior.

## Architecture

tgrep combines three components:

- **KEYSTONE** — trigram extraction, candidate selection, posting traversal,
  and batch search with SIMD acceleration (SSE4.2/AVX2/AVX512).
- **QIHSE** — persistence, WAL/recovery, mmap-backed word index (`.qwi`),
  search-result cache, and optimization database.
- **grep-printer** / **grep-searcher** / **grep-regex** — ripgrep-compatible
  output, verification, and regex matching.

### Index format

| Component | Description |
|-----------|-------------|
| `.tgs` segments | Trigram postings with delta-varint encoding |
| `.qwi` sidecars | mmap-backed flat sorted word index (240 MB for 26K files) |
| `.thi` sidecars | Legacy KEYSTONE hash index (fallback) |
| `manifest.json` | Segment list and generation tracking |
| `file_list_cache.json` | File metadata for mtime-based index-trust |
| `optimization.qdb` | Per-query performance optimization database |
| `search_cache.qsc` | Cached `-l` search results |
| `wal/` | Write-ahead log for crash-safe segment publication |

See `ARCHITECTURE.md` for the full design and `KEYSTONE_CHANGES.md` for
KEYSTONE API extension details.

## Benchmarking

```bash
# Comprehensive benchmark (19 patterns, 10 trials)
./benchmarks/run_benchmarks.sh 10

# Generate graphs
python3 benchmarks/make_graphs.py

# Results: benchmarks/benchmark_results.csv, benchmarks/graphs/
```

## Project structure

```
tgrep/
├── install.sh              # Installer (creates rg wrapper)
├── Cargo.toml
├── src/
│   ├── main.rs             # CLI entry point + rg-compat detection
│   ├── rg_compat.rs        # rg compatibility router
│   ├── search.rs           # Search pipeline (trigram/hash/streaming)
│   ├── build.rs            # Index build pipeline
│   ├── store.rs            # Segment format (.tgs)
│   ├── native.rs           # FFI bindings to KEYSTONE/QIHSE
│   ├── cache.rs            # Search-result cache
│   ├── cache_v2.rs         # v2 cache (grouped entries, file IDs)
│   └── index.rs            # Index management commands
├── native/                 # C wrappers for KEYSTONE/QIHSE
│   ├── keystone_wrapper.c
│   ├── word_index_mmap.c    # mmap flat word index (.qwi)
│   ├── qihse_wal_wrapper.c
│   └── ...
├── benchmarks/
│   ├── run_benchmarks.sh   # Benchmark runner
│   ├── make_graphs.py      # Graph generator
│   └── graphs/             # PNG visualizations
└── docs/
    ├── ARCHITECTURE.md
    ├── ROADMAP.md
    ├── BENCHMARKS.md
    └── KEYSTONE_CHANGES.md
```

## Dependencies

- [KEYSTONE](https://github.com/SWORDIntel/KEYSTONE) — trigram index engine
- [QIHSE](https://github.com/SWORDIntel/QIHSE) — persistence, WAL, word index
- [ripgrep](https://github.com/BurntSushi/ripgrep) — the real `rg` for delegation
- [grep-printer](https://docs.rs/grep-printer) — ripgrep-compatible output

## License

See LICENSE file for details.
