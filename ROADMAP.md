# tgrep roadmap

## Review repair reconstruction — complete

Audit baseline: `22d146ad3751597711b57116060e84cf169fc87b`. The environment
reverted before previous repaired source delivery. This branch reconstructs the
repair set from retained implementation history and validates it afresh.
Historical test results are not completion gates here.

- [x] Restore search correctness, wrapper compatibility and cache identity fixes.
- [x] Restore checked compact TGS/QWI, builder provenance and byte accounting.
- [x] Restore build/install/benchmark tools and fault-tested WAL foundation.
- [x] Reapply WAL v2 framing and conservative legacy migration.
- [x] Reapply bounded per-file output and cost-based query routing.
- [x] Reapply checked snapshot compaction and vendor inventory.
- [x] Pass release tests, strict Clippy, formatting and native sanitizers.
- [x] Remeasure immutable corpora, output memory and compaction; package source/executable.

Fresh application-source validation at `e61f441fc2ab666995d9cd760b4bcc85ee23fb24`:
253 release harness entries passed, strict Clippy/formatting/vendor inventory passed,
17 WAL and 12 QWI sanitizer groups passed, 135 paired search trials matched independent
ripgrep, nine large-output runs agreed, and 12 compaction query comparisons passed.
The exact executable and source bundle accompany the review. See `BENCHMARKS.md`
for measured sizes, latency regressions, memory/compaction results and evidence hashes.
These eight repair gates are separate from the continuing work below.

## Continuing engineering work

- [ ] Incremental refresh with explicit deletion/rename semantics and atomic publication.
- [ ] Profile traversal/metadata overhead on the deployment filesystem.
- [ ] Evaluate planner thresholds on representative selective/common queries.
- [ ] Runtime SIMD dispatch and a documented portable CPU floor.
- [ ] Establish imported vendor provenance before removal/replacement.
- [ ] Hard memory/disk isolation where cooperative budgets are insufficient.
- [ ] Validate power-loss durability on representative hardware/filesystems.

## Historical implementation phases

Phases 0–11 were recorded as complete: 12/12 implementation phases. Old phase
records, test counts and benchmark tables are in `docs/history/ROADMAP.md`;
they do not establish current repair verification. `refresh` remains a full
source rebuild. Current `compact` merges checked immutable snapshots.
