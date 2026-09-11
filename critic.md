# tgrep ARCHITECTURE.md — Critic

The architecture document is 888 lines. The actual architecture is maybe
250. The rest is corporate safety padding, hedging, and over-engineering
that a grep tool on a personal workstation does not need.

## What went wrong

GPT Astra wrote this like a document heading to a risk committee, not a
working architecture for a personal project. Every design decision has an
escape hatch. Every number has a disclaimer. Every simple concept gets
wrapped in enterprise operational machinery. The result reads like a
compliance filing with a trigram index buried somewhere inside it.

A competent engineer does not need a guide to avoid this. You write what
the system is, what it does, and how it works. You don't pre-apologize
for every number, you don't build a Kubernetes admission controller for a
grep tool, and you don't lecture the reader about statistical honesty in
a benchmark plan for a personal project.

## The bloat, by category

### 1. Hedging and disclaimers (~15-20 instances, scattered)

Every concrete statement is wrapped in defensive qualifiers:

- "Status: proposed architecture only. No implementation, benchmark
  execution, index creation, service installation, or ZFS changes are
  included in this work." (line 3)
- "These figures are a point-in-time inventory, not capacity
  reservations." (line 45)
- "These are admission-control budgets to refine after corpus sampling,
  not predicted compression ratios." (line 392)
- "These are proposed conservative starting limits, to be tuned using
  the actual build benchmark." (line 511)
- "These are tunable admission rules, not proof of zero system impact."
  (line 543)
- "These are deliberately ambitious, unmeasured goals to guide
  experiments." (line 795)
- "Phases describe dependency order, not a calendar estimate." (line 846)

Every number or design choice gets a "this is just a proposal, don't
blame me" escape hatch. This is the most pervasive form of bloat and adds
zero technical value. Delete all of it.

### 2. Resource governor (Section 10, ~40 lines)

A grep tool does not need:

- cgroup v2 CPU and memory controls with fallback logic for
  non-delegated controllers (line 525)
- PSI pressure stall monitoring with `some avg10` thresholds and
  30-second cooldowns (lines 537-545)
- Token-bucket I/O rate limiting at 20 MiB/s read, 10 MiB/s write
  (lines 519-520)
- Directory traversal throttling at 500 entries/s (line 521)
- Memory pressure containment with kernel termination as "a last
  resort" (line 528)
- "Report which controls are actually active rather than claiming kernel
  enforcement" (line 528) — defensive CYA language

This is a Kubernetes-style admission controller for a tool that searches
text files. Replace the entire section with `--threads` and
`--max-memory` flags. ~40 lines down to 3-4 sentences.

### 3. Durability and recovery (Section 9, ~50 lines)

The index is derived data. If it is corrupted, you rebuild it. The
document even admits this (line 432: "An invalid derived index causes
scan/rebuild") then spends 50 lines specifying:

- A 6-step publication protocol with ordered cross-dataset syncs,
  directory syncs, and atomic manifest pointer replacement (lines
  405-416)
- Torn journal tail repair (line 428)
- High-water mark tracking with out-of-order completion handling (lines
  429-431)
- "Never reinterpret old serialized pointers or dump native C structs
  as a storage format" (line 435) — a generic best-practice lecture,
  not a design decision
- "Validate any reused checksum helper against known vectors and
  scalar/accelerated equivalence" (line 439) — telling you to test your
  checksums

This is journal recovery for a banking ledger, not a rebuildable text
index. Cut to ~15 lines: segment format, manifest as commit point,
rebuild on corruption.

### 4. Job state machine (lines 494-500, 553-601)

```
QUEUED -> DISCOVERING/BUILDING -> FINAL_RECONCILIATION -> COMPLETE
                    |                   |
                    +-> PAUSING -> PAUSED
                    +-> WAITING_FOR_RESOURCES
                    +-> FAILED_RETRYABLE
```

Plus durable job state with "source/root identity, profile and format
hashes, requested resource profile, discovery frontier, work-item IDs
and source versions, completed batch receipts, verified segment
IDs/checksums, errors, pause reason, and timestamps" (lines 487-489).

This is a distributed task queue spec, not an index builder. A simple
"indexing in progress: X/Y files" covers 95% of the use case. ~50 lines
to ~10.

### 5. QIHSE security scoping (Section 11, ~25 lines)

The QIHSE adapter is explicitly a phase-6 optional extension (line 844),
yet security scoping is woven through sections 3, 6, 9, and 11:

- "Keep QIHSE security context on every classified-capable retrieval
  path, including indexing, candidate verification, lookup, enumeration,
  cache access, export, and snapshots" (line 643)
- "A forgotten or null context must not authorize data" (line 645)
- "do not index with elevated privileges and later serve that index
  through an unprivileged general-purpose daemon" (line 651)
- "Protected filenames, counts, and statistics are also observable
  data" (line 654)
- "requires the repository's low-clearance/high-data negative tests
  across query, enumeration, and persistence before release" (line 656)

Defer all of this to when QIHSE integration actually happens. ~25 lines
to ~5.

### 6. Benchmark protocol (Section 13, ~45 lines)

- "at least 30 repetitions per practical cell" (line 753)
- "paired trials in randomized order" (line 752)
- "fixed documented warmup, held-out queries" (line 753)
- p50/p95/p99 with confidence intervals (line 770)
- Full amortization formula with variables B, M, R, T (lines 787-791)
- "Do not combine these into a single marketing speedup" (line 723)
- "Report wins and regressions by workload instead of hiding difficult
  cases inside a selected average" (line 806)

This is an academic paper methodology section for a personal benchmark.
The core idea — compare against rg with controlled caches — is 10 lines.
The rest is methodological grandstanding. Cut to ~15 lines.

### 7. Redundant correctness lectures (~7 instances, scattered)

The same invariant restated many times:

- "An index is a necessary-condition filter. It must never decide that
  candidate membership alone is a match." (line 188)
- "Invalid index state, allocation failures, or incomplete enumeration
  must never become a successful empty result." (line 189)
- "Never validate only positive candidates and assume unseen negatives
  are still current." (line 244)
- "A document that cannot fit is retried... it is never marked finished
  merely because it was attempted." (line 569)
- "Exhausting a fixed candidate buffer is not completion; omitted
  candidates can contain the only exact matches." (line 349)
- "Do not turn an ingestion limit into a hidden search exclusion."
  (line 309)
- "An index scope narrower than the requested search is an optimization
  boundary, not a search exclusion" (line 251)

All the same idea: the index is a filter, not a source of truth; when in
doubt, scan. State it once. ~20 scattered lines to ~3.

### 8. Freshness mode over-engineering (Section 6, ~35 lines)

Three freshness modes (live, indexed, snapshot) with extensive hedging
about why none of them are actually correct:

- "live is not a point-in-time snapshot" (line 225)
- "Metadata equality is not a mathematical proof of unchanged bytes on
  every filesystem" (line 227)
- "Inotify events alone do not establish a complete query-time
  filesystem snapshot" (line 239)
- "When version tracking cannot be trusted, use a full scan" (line 228)

For a first implementation, live (always verify against filesystem) is
all you need. The indexed and snapshot modes are premature optimization
with massive correctness complexity. ~35 lines to ~10.

### 9. "Measure before deciding" filler (~10 instances, scattered)

- "Adopt only if real dictionary benchmarks beat binary search or
  direct lookup" (line 57)
- "Keep this out of the first file-level format unless profiling
  justifies it" (line 327)
- "Add packed integers or dense bitmaps only where their measured decode
  and intersection costs justify additional codecs" (line 275)
- "Test a second index over 32/64/128 KiB chunks for large files,
  selecting the size from measured candidate bytes" (line 314)
- "measure whether a compact auxiliary filter is worthwhile later"
  (line 200)

All saying "we'll decide based on benchmarks" — the default assumption
for any competent engineer. Stating it repeatedly is filler. Delete all
of it.

### 10. Phase/gate corporate language (Section 15, ~15 lines)

7 phases with explicit "gates before proceeding" and gate descriptions
like "Reproducible comparison protocol and current-source reuse
decisions" and "Restart correctness, bounded lost work, pause/resume,
resource limits, interrupted publication, quota failure." This reads
like an enterprise PMO template. The phases are fine as a todo list; the
gate language is corporate project management speak. ~15 lines to ~10.

## Trim estimate

| Category | Current (approx) | Target |
|---|---|---|
| Hedging/disclaimers | ~40 scattered | 0 |
| Resource governor | ~40 | ~5 |
| Durability/recovery | ~50 | ~15 |
| Job state machine | ~50 | ~10 |
| QIHSE security scoping | ~25 | ~5 |
| Benchmark protocol | ~45 | ~15 |
| Redundant correctness lectures | ~20 scattered | ~3 |
| Freshness over-engineering | ~35 | ~10 |
| "Measure before deciding" filler | ~15 scattered | 0 |
| Phase/gate corporate language | ~15 | ~10 |
| **Total** | **~335 lines of bloat** | **~73 lines** |

888 lines -> ~350-400 lines. The core architecture (trigram index,
KEYSTONE/QIHSE reuse, segment format, query planner, rg compatibility)
is genuine content. Everything else is padding.
