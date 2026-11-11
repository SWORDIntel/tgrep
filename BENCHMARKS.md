# tgrep verified repair measurements — 2026-11-11

## Measurement identities and conditions

Measured application source: `e61f441fc2ab666995d9cd760b4bcc85ee23fb24`. Executable: **4,400,648 bytes**, SHA-256 `a5b18b184f146ed223d0c99e034e9c8f8d31c7609c12eb768edd6e74b3c76761`. Comparison executable: recovered tgrep 1.1.0 (`dd9566441707`), SHA-256 `ffaefbed7ffd79197e634fabba48352a5e6718071981ab561a0d4491803bf119`. Oracle: independent ripgrep 15.2.0.

The preserved source-only corpora contain **5,133 files and 127,985,471 bytes**. Their complete file sets, sizes and SHA-256 hashes matched the original audit before and after measurement. Source revisions: tgrep `22d146ad3751597711b57116060e84cf169fc87b`, KEYSTONE `da8e31fc92a4bb35b572b2935aed8d8377cc9e1b`, CPython `0ada998ebbf392238719358432cd723a622fcd5e`. These are fixed corpus snapshots, not the repaired working tree or whole repositories with build artifacts.

Both fresh tgrep versions used two discovery workers, a **1GiB cooperative build budget** and 1GiB/s logical I/O pacing. Queries used four workers, one warmup, nine fresh-process trials and rotating three-tool order. Filesystem/application caches were warm; no OS cache or ZFS ARC reset was performed. No heavy build/sanitizer job ran concurrently with timed queries. This container/overlay run has no CPU pinning or deployment-filesystem validation, and the medians are observations rather than a statistical significance claim.

## Produced index sizes

The original column uses the preserved audit ZIP's **per-file inventories**, independently summed and byte-matched to the original raw JSON. These correct the slightly different totals quoted in the earlier first-pass narrative. Final state includes TGS, QWI, metadata, file-list cache, WAL and lock files immediately after build/status. **MiB means 1,048,576 bytes.**

| Corpus | Files | Source bytes | Original audit state bytes | Repaired state bytes | Repaired MiB | Reduction |
|---|---:|---:|---:|---:|---:|---:|
| tgrep | 237 | 10,418,350 | 8,110,632 | 3,105,007 | 2.961 | 61.72% |
| KEYSTONE | 244 | 11,502,625 | 8,767,846 | 3,385,451 | 3.229 | 61.39% |
| CPython | 4,652 | 106,064,496 | 155,828,387 | 50,701,691 | 48.353 | 67.46% |

| Corpus | TGS bytes | QWI bytes | Metadata/cache/WAL bytes | Allocated file bytes | Fresh 1.1 state bytes | 1.2 after query bytes |
|---|---:|---:|---:|---:|---:|---:|
| tgrep | 1,859,889 | 1,203,932 | 41,186 | 3,121,152 | 3,105,815 | 3,105,132 |
| KEYSTONE | 2,050,834 | 1,292,732 | 41,885 | 3,403,776 | 3,373,358 | 3,385,589 |
| CPython | 23,032,738 | 26,887,180 | 781,773 | 50,720,768 | 50,697,997 | 50,701,836 |

No THI files are produced. Allocated file bytes are regular-file `st_blocks × 512`; they exclude directory blocks and do not measure ZFS compression, deduplication or pool-level physical storage. All corpus files were represented in the measured final indexes: 237, 244 and 4,652 documents respectively.

The historical original builds used a 2GiB budget; the fresh paired builds used 1GiB. Treat the historical table as observed footprint reduction across the complete repair, not a controlled build-speed or isolated-format experiment. The fresh 1.1 and 1.2 builds already share compact TGS/QWI formats. Discovery order and segment partitioning affect varint lengths, so small paired differences do not establish an additional 1.2 compression improvement.

### Why the footprint fell

TGS v2 removes the duplicated 12-byte-per-block table. QWI v3 replaces the old 24-byte entries with 12-byte hash/document entries plus checked block directories. The redundant/unbound THI sidecar is no longer written. On the archived CPython build, THI alone occupied 73,034,589 bytes; old QWI occupied 53,564,888 bytes versus 26,887,180 bytes in the final two-segment build.

The independent measurement parser checked every generated metadata/posting/word block CRC, document ordering/bounds and QWI-to-TGS binding. It also confirmed exact layout formulas: **TGS bytes = `90 + 64D + 2L + 16G + 12B + V`** and **QWI bytes = `56 + 24 ceil(W/512) + 12W`**, where D is documents, L raw path bytes, G grams, B posting blocks, V encoded varint bytes and W word entries. CRCs detect corruption; they are not cryptographic authentication.

## Search parity and latency

**All 135 paired trials passed** sorted filename and exit-status parity for both tgrep versions against independent ripgrep: 15 query cases × nine trials, comprising 405 measured processes. These are sampled semantic checks, supported by the separate CLI regression suite; they are not a proof over all ripgrep syntax.

| Corpus | Query | Matching files | 1.1 median ms | 1.2 median ms | rg median ms | 1.2 time change |
|---|---|---:|---:|---:|---:|---:|
| tgrep | absent fixed literal | 0 | 28.23 | 25.97 | 25.50 | -8.0% |
| tgrep | fixed `return` | 139 | 34.05 | 40.19 | 28.30 | +18.0% |
| tgrep | word `return` | 117 | 25.90 | 29.96 | 16.34 | +15.6% |
| tgrep | case-insensitive `return` | 173 | 21.20 | 20.17 | 17.55 | -4.9% |
| tgrep | selective fixed literal | 1 | 18.14 | 21.21 | 17.73 | +16.9% |
| KEYSTONE | absent fixed literal | 0 | 29.91 | 56.02 | 23.44 | +87.3% |
| KEYSTONE | fixed `return` | 167 | 37.94 | 32.20 | 18.05 | -15.1% |
| KEYSTONE | word `return` | 150 | 38.07 | 32.52 | 25.28 | -14.6% |
| KEYSTONE | case-insensitive `return` | 174 | 24.28 | 19.96 | 23.03 | -17.8% |
| KEYSTONE | selective fixed literal | 4 | 22.62 | 30.14 | 14.84 | +33.2% |
| CPython | absent fixed literal | 0 | 124.04 | 133.01 | 39.74 | +7.2% |
| CPython | fixed `return` | 3,125 | 194.35 | 116.81 | 61.06 | -39.9% |
| CPython | word `return` | 2,994 | 258.22 | 176.84 | 97.57 | -31.5% |
| CPython | case-insensitive `return` | 3,191 | 261.54 | 125.80 | 97.77 | -51.9% |
| CPython | selective fixed literal | 3 | 141.82 | 142.54 | 42.54 | +0.5% |

The selective literals were `TGREP_QWI_ENTRY_SIZE`, `dsmil_hash_index_save` and `unicode_encode_call_errorhandler`. The absent literal was `TGREP_REVIEW_ABSENT_918273645`. All cases used `-l`; fixed literals used `-F`, word queries `-w`, and case-insensitive queries `-i`.

Eight of 15 candidate medians improved over repaired 1.1; seven increased. CPython common-query times fell **39.9% fixed, 31.5% word and 51.9% case-insensitive**, while its absent query increased 7.2% and selective query increased 0.5%. The KEYSTONE absent-query median increased 87.3%. Ripgrep was faster in 14 of 15 measured cases. This evidence supports the common-query improvements and preserves the regressions; it does **not** support a general claim that tgrep is faster than ripgrep. Traversal/metadata and segment-open costs remain deployment-profiling work.

## Large-output memory

A generated **128MiB** source contained 131,072 lines of 1,024 bytes. Three rotating-order trials per executable ran `-F needle` with fresh absent state, four workers and direct-to-file stdout. Linux `wait4` measured each executable's peak RSS; the harness hashed output afterward in bounded chunks. The source was read before trials and caches were not reset.

| Tool | Peak RSS by trial, KiB | Median KiB |
|---|---|---:|
| tgrep 1.1 | 145,824, 145,936, 145,828 | 145,828 |
| tgrep 1.2 | 14,736, 14,752, 14,756 | 14,752 |
| ripgrep 15.2.0 | 152,872, 152,880, 152,760 | 152,872 |

The candidate reduced median peak RSS **89.88%**. All nine runs produced **145,096,704 identical output bytes**, SHA-256 `85f08a58956f5aab38ad08d5aae4e30746c349ba375b24b55c68427978526c22`, with zero stderr bytes and no temporary files remaining. This measures formatted-output buffering for bounded-size input lines; it does not establish a total RSS ceiling or bound matcher memory for a huge single line. Spilling trades memory for temporary-disk I/O and capacity.

## Snapshot compaction

The final CPython state was copied privately. Compaction merged **two segments into one** in **6.932 seconds** (one warm observation). It preserved **4,652 documents, 9,625,506 trigram postings and 2,231,869 word entries**. The supplied state and source corpus content inventories remained unchanged.

| Measurement | Before | Immediately after compact |
|---|---:|---:|
| Logical state bytes | 50,701,836 | 47,729,281 |
| Allocated file bytes | 50,724,864 | 47,751,168 |
| Active TGS bytes | 23,032,738 | 20,060,080 |
| Active QWI bytes | 26,887,180 | 26,887,124 |

Net reduction: **2,972,555 bytes (5.86%)**. The immediate post-compaction state was **45.518MiB**, 69.37% below the archived original CPython state. Later verification queries grew the hint cache by 133 bytes, producing 47,729,414 total bytes; this is recorded separately.

All ten ordinary before/after query checks and both selective indexed-only checks matched ripgrep. Independent full format validation passed before and after the merge. This is snapshot compaction, not a refresh: it does not discover new or changed source files.

## Evidence and rerun scope

The companion `tgrep_1.2.0_verified_2026-11-11.zip` contains full raw JSON, corpus manifests, individual trials, explain plans, per-file state inventories, logs, validation metadata and the exact measured executable. Raw measurement identities:

- `paired-e61f441.json` — SHA-256 `e0aecdf4bc0044d383021008f63515954a275c3b26ce40fba508bc713a0aa1ac`.
- `output-memory-e61f441.json` — SHA-256 `9d7f7c0b045e4cf9b8d4c5d46997d9c8ffad7d1a18673704b3b2245cb8f122c2`.
- `compaction-e61f441.json` — SHA-256 `7db05590ad9ea8cc3033e4675bcb346106e99a1da49335925a8377128a813c6d`.

The source bundle contains the reusable `benchmarks/measure_review.py`, `benchmarks/measure_output_memory.py` and `benchmarks/measure_compaction.py` harnesses. Measurement directories must be new, corpus copies must remain immutable, and independent rg must be selected explicitly. Recreating the exact old source mtimes/inodes or discovery ordering is not required for correctness, but can change metadata fingerprints, segment splits and encoded sizes.

Earlier reconstructed-source attempts are retained in the evidence provenance: a 2GiB run disappeared during the baseline CPython build with no recovered exit status and unknown cause; a 512MiB baseline build correctly refused its memory reservation; a subsequent 1GiB run passed on superseded c79a365 source. The final e61f441 run at 1GiB passed and is the sole source of the current performance tables. No OOM diagnosis is asserted for the interrupted run.
