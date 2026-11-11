# tgrep — persistent indexes for live-file search

tgrep uses file-level trigram and word-hash indexes to select candidates, then
checks source content with ripgrep's matching libraries. Ordinary searches walk
the requested filesystem, respect ignore rules, and scan files outside valid
index coverage. An index is a filter; cached results are ordering hints.

Current source version: **1.2.0**. The accompanying [SRSL-1.1 licence](LICENSE)
applies to tgrep; third-party components retain their own licences. See
[CHANGELOG.md](CHANGELOG.md), [ARCHITECTURE.md](ARCHITECTURE.md), and
[BENCHMARKS.md](BENCHMARKS.md) for behavior, measurements and limits.

## Build and run

Linux, Rust, a C compiler, and independent ripgrep are required for the full
validation suite. Default native compilation uses SSE4.2; the executable requires
a compatible CPU. `compile.sh` may select more host-specific SIMD flags. The
scripts do not implement cross compilation or runtime CPU multiversioning.

```bash
cargo build --release --locked --bin tgrep
./target/release/tgrep --version
TGREP_STATE_DIR=/path/to/tgrep-state ./target/release/tgrep index build /path/to/source
TGREP_STATE_DIR=/path/to/tgrep-state ./target/release/tgrep -l -F needle /path/to/source
TGREP_STATE_DIR=/path/to/tgrep-state ./target/release/tgrep index status
```

The Cargo command uses Cargo's configured target directory. `compile.sh` and
`install.sh` resolve that directory from Cargo metadata, explicitly build the
native host target, and use `<target_directory>/<host>/<profile>/tgrep`. They
reject `CARGO_BUILD_TARGET` before cleaning or installing. `--version` reports
package version, source revision/dirty state, profile and target; an archive
without Git metadata reports `source-archive`.

State resolves through `TGREP_STATE_DIR`, `$XDG_STATE_HOME/tgrep`,
`$HOME/.local/state/tgrep`, then `.tgrep` if no home is available. Keep it outside
source roots when possible. The builder also excludes its canonical state
directory, including configured symlink aliases, and rejects roots inside state.

## Searching

The direct CLI accepts `-w`, `-l`, `-c`, `-n`, `-q`, `-i`, `-S`, `-F`, repeated
`-e`, `--explain`, `--list-files` and `--indexed-only`. `--explain` reports a plan
without executing the query. Direct `-j`, `--hidden`, `-g`, `-H` and `-I` flags
are not exposed; the implemented config subset covers workers, hidden files,
globs and case mode. The wrapper delegates unsupported commands/configuration.

Traversal observes `.gitignore`, `.ignore` and `.rgignore`. Freshness comparisons
include device, inode, size, mtime and ctime. New and changed files are scanned;
matching metadata or cache hits never authorize an unverified result. This is
live search, not a filesystem snapshot. Case-insensitive/unindexable queries
stream. Broad queries over small indexed files can also stream when checked
dictionary frequencies predict little useful pruning.

`--indexed-only` bypasses cost-based streaming, excludes uncovered files, and
errors when safe indexed execution is unavailable. It is a diagnostic mode,
not a complete live-search result.

Completed-file output uses a bounded queue. Per-file output memory is capped at
1 MiB before spilling to a private anonymous temporary file. Spill/copy buffers
are bounded; write and flush failures propagate. Broken pipes cancel workers and
close temporary files. Large output can consume temporary disk space and add I/O.
This bounds output buffering, not total process RSS or matcher working memory.

## Optional rg wrapper

```bash
./install.sh --no-index --no-zfs
```

The installer builds tgrep, locates independent rg, and creates the wrapper in
`~/.local/bin`. Supported nonterminal searches can use tgrep. Unsupported flags,
non-UTF-8 arguments, terminal output, and wrapper `--help`/`--version` delegate
with original argument bytes. Use `tgrep --version` to identify tgrep itself.
`./install.sh --uninstall` removes the installation while preserving state.

## Index lifecycle

| Command | Behavior |
| --- | --- |
| `index build ROOT...` | Full source build with atomic manifest replacement. |
| `index status` | Validate active segments; report active, obsolete, temporary, auxiliary, logical and allocated bytes. |
| `index refresh` / `index rebuild` | Full source rebuild of bound roots. |
| `index compact` | Merge checked immutable snapshots without source reads; preserve freshness metadata and root bindings. |
| `index recover` | Validate WAL, retain committed segments and clean unreferenced files; does not publish a new manifest. |
| `index pause` / `index resume` | Cooperative file-boundary pause; after an interrupted process, resume starts a full rebuild from saved options. |

```bash
tgrep index build /path/to/source --threads 2 --max-memory 2GiB --io-limit 1GiB/s
```

Memory reservations/RSS checks and logical I/O pacing are cooperative, not OS
limits. Operations can burst between checks. Budget failures preserve the old
manifest. Files larger than 64 MiB and BOM-marked UTF-16 remain scan-only. Binary
files may contribute index entries; source verification decides output behavior.

New builds write **TGS v2** and **QWI v3**, with checked metadata/postings and word
blocks bound to the TGS generation and metadata fingerprint. Legacy TGS v1 lacks
safe encoding provenance. Search automatically rebuilds legacy or unreadable
segments from saved roots and publishes the replacement atomically. If rebuilding
fails, normal search scans source files and `--indexed-only` reports the failure.
Index repair never removes source files. Unbound legacy THI is neither produced
nor used for query pruning.

**WAL v2** protects segment identity and record headers as well as payloads.
Complete legacy records remain readable; appends create a new v2 segment.
Incomplete legacy tails are rejected unchanged. Only an incomplete final v2
record in the last segment can be repaired after validation. WAL v2 is not
downgrade-compatible. Do not open migrated state with older binaries; retain a
separate pre-migration state when downgrade is required. The repaired 1.1 reader
rejects v2, but pre-repair readers cannot be assumed to fail safely.

Compaction checks every input posting and present word sidecar. Missing QWI
omits the merged sidecar; a corrupt present QWI fails. Empty state is a no-op;
one segment is fully checked without rewriting. Old snapshots stay usable until
replacement publication. Files with uncertain commit/publication outcomes are
retained. Compaction does not add files or refresh source changes.

## Validation and remaining scope

```bash
cargo test --release --locked -j 4
cargo clippy --all-targets --locked -j 4 -- -D warnings
cargo fmt --all -- --check
python3 scripts/vendor_inventory.py --check
```

CLI tests require real rg and do not silently skip the oracle. Review harnesses
record executable/corpus hashes, independent byte counts, parity and timing
conditions. Historical performance claims are preserved under `docs/history/`;
they do not describe current live-file behavior.

Incremental refresh, deployment-filesystem performance tuning and runtime SIMD
dispatch remain open. [docs/VENDOR_INVENTORY.md](docs/VENDOR_INVENTORY.md) records
native scope without treating unlinked files as automatically removable.

The tracked historical 1.0.0 Debian executable lacks established source
provenance. Build current source. `scripts/build_deb.sh` derives dependencies
from the staged ELF and fails if Debian library metadata is missing; it does
not guess a libc floor. See [ROADMAP.md](ROADMAP.md).
