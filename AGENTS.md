# tgrep — Agent Guide

## End-of-turn progress report

After every turn where code changes were made or a phase completed,
invoke the `/progress-report` skill before ending the turn. This generates
a compact progress bar and roadmap summary from `ROADMAP.md`.

If the skill is unavailable (e.g. first session), manually produce the
progress bar by reading `ROADMAP.md` and counting completed phases.

## Project overview

tgrep is a persistent trigram/word-index filter for live-file searches.
Read `README.md` for commands, `ARCHITECTURE.md` for invariants, `ROADMAP.md`
for unfinished work and `BENCHMARKS.md` for measurements. `PLAN.md` and
`docs/history/` preserve historical targets/snapshots. Old test counts,
workstation assumptions and timing ratios are not current validation.

## Build and validation

```bash
cargo build --release --locked --bin tgrep
cargo test --release --locked -j 4
cargo clippy --all-targets --locked -j 4 -- -D warnings
cargo fmt --all -- --check
python3 scripts/vendor_inventory.py --check
```

CLI tests require independent ripgrep. Measure release builds with explicit
state outside immutable corpora; compare outputs and timings. Review harnesses
record hashes, byte counts and measurement conditions. Do not run heavy builds
or sanitizer jobs alongside benchmarks.

## Storage and search invariants

Keep published segments immutable. Corruption broadens ordinary search to
scanning or produces an indexed-only error, never a false negative. Cache hits
only order verification. Preserve root bindings and freshness through compaction.
After attempted COMMIT or manifest publication, retain files on ambiguous failure.

Root `build.rs` lists linked native sources. Vendor inventory records hashes and
compiler-discovered headers; it does not establish imported upstream provenance
or justify removing unlinked files without further review.
