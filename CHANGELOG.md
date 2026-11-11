# Changelog

## Unreleased

- Automatically rebuild legacy or unreadable indexes during indexed search from
  their saved roots; fall back to live scanning if repair fails.

## 1.2.0 — review repairs

- Version WAL segment/record headers with independent integrity, complete legacy
  replay, new-format appends and unchanged rejection of ambiguous legacy tails.
- Bound per-file output with private anonymous spilling; propagate flush errors
  and cancel workers on broken pipes.
- Route broad small-file queries using checked frequency bounds; reuse metadata
  already inspected for root validation.
- Merge checked immutable snapshots without source reads, remapping sparse IDs
  and word entries while preserving roots/freshness and uncertain transactions.
- Inventory vendored files, linked translation units and included headers.
- Require warning-free Clippy validation for this repair branch.

## 1.1.0 — reconstructed first-pass repairs

- TGS v2 metadata/posting integrity without the duplicate block table.
- Compact QWI v3 blocks bound to TGS; remove THI production/query filtering.
- UTF-16 BOM scan-only handling, binary word coverage, `.rgignore`, state exclusion.
- Conservative legacy/corruption fallback and explicit indexed-only errors.
- Canonical identity reuse, relative cache IDs, hint-only caches and reduced
  streaming/optimizer overhead.
- Bounded completed-file queue, cancellation and raw wrapper argument delegation.
- WAL LSN continuity, strict validation, short writes, sync errors and checkpoints.
- Complete active/obsolete/temporary/auxiliary logical and allocated byte reports.
- Correct Cargo output handling, independent benchmark baselines, shell timing
  and source/build version identification.
- Archive historical claims; document current live-search behavior.

The repair source was reconstructed after an environment rollback. New validation
and commit identities are recorded independently of lost source commits. Current
measurement evidence is in `BENCHMARKS.md`.
