# tgrep — Agent Guide

## End-of-turn progress report

After every turn where code changes were made or a phase completed,
invoke the `/progress-report` skill before ending the turn. This generates
a compact progress bar and roadmap summary from `ROADMAP.md`.

If the skill is unavailable (e.g. first session), manually produce the
progress bar by reading `ROADMAP.md` and counting completed phases.

## Project overview

tgrep is a persistent trigram-indexed search tool. See:
- `ARCHITECTURE.md` — system design
- `PLAN.md` — implementation plan (11 phases)
- `ROADMAP.md` — progress tracking
- `KEYSTONE_CHANGES.md` — KEYSTONE API extension details

## Build & test

```bash
cargo build          # compile
cargo build --release # optimized build (required for benchmarks)
cargo test           # run all tests (60 tests)
cargo test -- --nocapture  # run tests with output
```

## Benchmark

```bash
# Comprehensive benchmark suite (19 patterns, 10 trials, cold + warm cache)
./benchmarks/run_benchmarks.sh 10 /tmp/tgrep_qwi_mmap /rpool/scratch/tgrep_corpus

# Generate graphs from benchmark CSV
python3 benchmarks/make_graphs.py

# Paired tgrep vs rg benchmark (10 trials, verifies result equality)
TGREP_STATE_DIR=/tmp/tgrep_qwi_mmap ./target/release/bench "Pattern" /rpool/scratch/tgrep_corpus 10

# Indexed-only mode (skips unindexed file scan, for measuring pure index performance)
TGREP_STATE_DIR=/tmp/tgrep_qwi_mmap ./target/release/bench "Pattern" /rpool/scratch/tgrep_corpus 10 -- --indexed-only

# Debug: list files that would be searched
TGREP_STATE_DIR=/tmp/tgrep_qwi_mmap ./target/release/tgrep --list-files "Pattern" /rpool/scratch/tgrep_corpus

# Explain query plan without searching
TGREP_STATE_DIR=/tmp/tgrep_qwi_mmap ./target/release/tgrep --explain "Pattern" /rpool/scratch/tgrep_corpus
```

See `BENCHMARKS.md` for full results and `benchmarks/graphs/` for visualizations.

## Key dependencies

- KEYSTONE: `/home/john/Documents/KEYSTONE` (trigram index engine)
- QIHSE: `/home/john/Documents/QIHSE` (persistence, WAL)
- Corpus: `/rpool/scratch/tgrep_corpus` (strong test corpus, 26K files, 1.15 GB, includes hidden files)
- Index state: `/tmp/tgrep_qwi_mmap` (mmap .qwi word index, 75 segments, file list cache)

## Implementation status

Phases 0–11 complete (100% of 11 phases). Search is functional and verified against rg.
Post-Phase 11: mmap word index, index-trust optimization, comprehensive benchmark suite.
- Rare patterns: 5.5–12.8x faster than rg (cold), 12–54x (warm)
- Broad patterns: 2.4–8.4x faster than rg (cold), 24–42x (warm)
- Word patterns: 1.3–10.7x faster than rg (cold), 20–46x (warm)
- Case-insensitive: within 10% of rg (cold), 29–36x (warm)
See `ROADMAP.md` for phase progress and `ARCHITECTURE.md` for design details.
