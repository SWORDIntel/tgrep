# Native vendor inventory

The maintained machine-readable inventory is [`vendor/manifest.json`](../vendor/manifest.json).
It records SHA-256 and logical size for every retained vendor file, the exact native
translation units declared in [`build.rs`](../build.rs), and each translation unit's
local header dependencies observed by the C preprocessor. The manifest excludes
itself and its temporary update file.

## Refresh and verify

Run from the repository root:

```bash
python3 scripts/vendor_inventory.py --check
python3 scripts/vendor_inventory.py --refresh
git diff -- vendor/manifest.json
```

`--check` exits nonzero when file content, the compiler profile, header reachability,
the build's source list, or the baseline comparison has changed. Refresh intentionally
accepts the current working-tree contents; review the resulting diff before committing.
No dependency installation or network access occurs. Python 3.9+, Git, and a GCC-compatible
C preprocessor are required, and the reviewed baseline commit must be available locally.

The probe uses the literal source/include/flag declarations in `build.rs`, plus
`TGREP_SIMD_FLAGS` or that file's default. `--cc` selects the compiler command and
`--simd-flags` overrides SIMD flags. It records this explicit profile; it does not
claim to reproduce arbitrary Cargo target-specific `CFLAGS` overrides. A different
profile can legitimately change the dependency inventory. System headers are excluded
by `-MM`; this is a repository source inventory, not a complete toolchain SBOM.

To recompute the optional KEYSTONE comparison from an existing local checkout:

```bash
python3 scripts/vendor_inventory.py --refresh --keystone-repo /path/to/KEYSTONE
```

Without that argument, refresh/check reuse the committed reference hashes and compare
the current files against them. The script never fetches the comparison repository.

## Current build footprint

The recorded profile is GCC 14.2.0 on `x86_64-linux-gnu`, with `-std=c11`,
`-D_GNU_SOURCE`, `-Wall`, `-Wextra`, `-O2`, and the default `-msse4.2`.

| Inventory category | Files | Logical bytes |
|---|---:|---:|
| All retained vendor files | 187 | 9,563,410 |
| Vendor translation units and reachable headers in this profile | 28 | 413,292 |
| Vendor files outside this profile's translation-unit/header closure | 159 | 9,150,118 |

The build compiles **15 native translation units**: seven local wrappers and eight
vendored C sources. Those units reach **20 local headers**, all vendored. The manifest
records the exact per-unit header mapping, rather than inferring usage from filenames.

| Owner | Compiled translation units |
|---|---|
| tgrep | `native/hash_wrapper.c`, `native/keystone_wrapper.c`, `native/qihse_cache_wrapper.c`, `native/qihse_opt_wrapper.c`, `native/qihse_wal_wrapper.c`, `native/qihse_wrapper.c`, `native/word_index_mmap.c` |
| KEYSTONE | `src/dsmil_hash_indexer.c`, `src/keystone.c`, `src/keystone_avx512.c`, `src/keystone_avx512_search.c`, `src/keystone_trigram.c` |
| QIHSE | `src/frieze/qihse_btree.c`, `src/tractable/qihse_table_store.c`, `src/tractable/qihse_wal.c` |

KEYSTONE and QIHSE paths in the table are relative to their respective `vendor/` directories.

No vendor files were removed by this repair. The 159 files outside the recorded build
closure remain available for separate review, including eight checked-in object files
and eight dependency files. The largest such file is KEYSTONE's `include/simdjson.h`
at 7,711,281 bytes. A closure for one compile profile does not establish that deletion
is safe for every platform, feature setting, standalone native test, or downstream use.

## Provenance and local changes

The comparison baseline is tgrep repository commit
`22d146ad3751597711b57116060e84cf169fc87b`. This is the source snapshot reviewed before
these repairs. **It is not an upstream import revision.** The inventory records current
and baseline SHA-256 for modified files, and distinguishes additions and removals.

| Vendored file changed since the reviewed tgrep baseline | Current local change |
|---|---|
| `vendor/QIHSE/include/qihse_wal.h` | Documents and declares WAL v2 framing, independent header integrity, and legacy compatibility. |
| `vendor/QIHSE/src/tractable/qihse_wal.c` | Validated append/replay/checkpoint behavior, checked v2 framing, atomic segment-header publication, and conservative legacy recovery. |

The other 185 retained vendor files match that reviewed baseline byte for byte. This
does not establish that they match their original upstream imports: any earlier local
changes are already part of the reviewed baseline. The seven tgrep wrappers are local
project sources, outside the vendor-file baseline table; their current hashes are
included with the translation-unit mapping.

An additional bytewise comparison used the available local
[SWORDIntel/KEYSTONE](https://github.com/SWORDIntel/KEYSTONE) checkout at
`da8e31fc92a4bb35b572b2935aed8d8377cc9e1b`. Among the 30 retained KEYSTONE files,
**13 are identical and 17 differ** from the same paths at that revision. The manifest
lists each result and reference SHA-256. This comparison revision is not established
as the original import point; the differences cannot all be attributed to local tgrep
edits or used as an upstream patch history.

The original KEYSTONE and QIHSE import revisions remain **unknown**. No verified QIHSE
upstream comparison revision is recorded. [`KEYSTONE_CHANGES.md`](../KEYSTONE_CHANGES.md)
provides design/history context but does not replace immutable import provenance.

## Maintenance rule

After changing native build inputs, refresh the inventory under the intended compile
profile, review the hash/reachability changes, and run `--check`. Keep any future vendor
reduction separate from this inventory update, with the affected build profiles and
native fixtures validated. Recording a verified original import revision later should
add that evidence explicitly while preserving this reviewed-baseline comparison.
