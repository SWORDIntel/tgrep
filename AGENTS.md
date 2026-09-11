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
cargo test           # run all tests (36 tests)
cargo test -- --nocapture  # run tests with output
```

## Benchmark

```bash
# Paired tgrep vs rg benchmark (10 trials, verifies result equality)
TGREP_STATE_DIR=/tmp/tgrep_strong_state2 ./target/release/bench "Pattern" /rpool/scratch/tgrep_corpus 10

# Indexed-only mode (skips unindexed file scan, for measuring pure index performance)
TGREP_STATE_DIR=/tmp/tgrep_strong_state2 ./target/release/bench "Pattern" /rpool/scratch/tgrep_corpus 10 -- --indexed-only

# Debug: list files that would be searched
TGREP_STATE_DIR=/tmp/tgrep_strong_state2 ./target/release/tgrep --list-files "Pattern" /rpool/scratch/tgrep_corpus

# Explain query plan without searching
TGREP_STATE_DIR=/tmp/tgrep_strong_state2 ./target/release/tgrep --explain "Pattern" /rpool/scratch/tgrep_corpus
```

## Key dependencies

- KEYSTONE: `/home/john/Documents/KEYSTONE` (trigram index engine)
- QIHSE: `/home/john/Documents/QIHSE` (persistence, WAL)
- Corpus: `/rpool/scratch/tgrep_corpus` (strong test corpus, 26K files, 1.15 GB, includes hidden files)
- Index state: `/tmp/tgrep_strong_state2` (strong corpus index, 75 segments, file list cache)

## Implementation status

Phases 0–8 complete (73% of 11 phases). Search is functional and verified against rg.
Phase 8 adds hash index fast path for whole-word queries (`-w` flag): 374–1069x faster than rg.
See `ROADMAP.md` for phase progress and `ARCHITECTURE.md` for design details.
