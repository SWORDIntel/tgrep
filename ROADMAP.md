# tgrep roadmap

## Review repair reconstruction — in progress

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
- [ ] Pass release tests, strict Clippy, formatting and native sanitizers.
- [ ] Remeasure immutable corpora, output memory and compaction; package source/executable.

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
