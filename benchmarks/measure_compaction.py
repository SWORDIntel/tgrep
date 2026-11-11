#!/usr/bin/env python3
"""Measure a true segment merge on a private copy of an existing index state.

Requires an immutable source-only corpus with newline-free filenames and one
matching bound root. Never rebuilds the corpus or modifies the supplied state.
Timings are single-run observations under warm caches, not a speedup benchmark.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import struct

from measure_review import digest_file, footprint, invoke as run, save_json as checkpoint, validate_layouts

RARE = {"tgrep": "TGREP_QWI_ENTRY_SIZE", "keystone": "dsmil_hash_index_save",
        "cpython": "unicode_encode_call_errorhandler"}
QUERIES = [(["-l", "-F"], "TGREP_REVIEW_ABSENT_918273645"),
           (["-l", "-F"], "return"), (["-l", "-w"], "return"),
           (["-l", "-i"], "return")]


def inventory(root):
    digest = hashlib.sha256()
    count = total = 0
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"measurement input contains a symlink: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"measurement input contains a nonregular file: {path}")
        relative = os.fsencode(path.relative_to(root))
        if b"\n" in relative or b"\r" in relative:
            raise ValueError("measurement input must have newline-free filenames")
        size = path.stat().st_size
        digest.update(relative + b"\0" + size.to_bytes(8, "little"))
        digest.update(bytes.fromhex(digest_file(path)))
        count += 1
        total += size
    if not count:
        raise ValueError(f"empty measurement input: {root}")
    return {"root": str(root), "files": count, "source_bytes": total,
            "content_manifest_sha256": digest.hexdigest()}


def active_counts(state, manifest):
    counts = {"segments": len(manifest["segments"]), "documents": 0,
              "postings": 0, "word_sidecars": 0, "word_entries": 0}
    for name in manifest["segments"]:
        if Path(name).name != name:
            raise ValueError("manifest segment names must be local basenames")
        path = state / "segments" / name
        data = path.read_bytes()
        magic, version, _, _, docs, _, _, dictionary, postings, _, footer = struct.unpack_from("<IHQQI6Q", data)
        if magic != 0x54475331 or version != 2 or footer + 16 != len(data):
            raise ValueError(f"expected a complete TGS v2 segment: {path}")
        counts["documents"] += docs
        counts["postings"] += sum(struct.unpack_from("<IIQ", data, offset)[1]
                                  for offset in range(dictionary, postings, 16))
        sidecar = path.with_suffix(".qwi")
        if sidecar.exists():
            with sidecar.open("rb") as source:
                magic, version, entries = struct.unpack("<IIQ", source.read(16))
            if magic != 0x54575149 or version != 3:
                raise ValueError(f"expected QWI v3: {sidecar}")
            counts["word_sidecars"] += 1
            counts["word_entries"] += entries
    return counts


def snapshot(state):
    manifest = json.loads((state / "manifest.json").read_text())
    return {"manifest": manifest, "footprint": footprint(state),
            "active": active_counts(state, manifest), "format_validation": validate_layouts(state)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for argument in ("binary", "state", "corpus", "rg", "output-dir"):
        parser.add_argument(f"--{argument}", type=Path, required=True)
    parser.add_argument("--rare-pattern", help="Selective fixed literal; defaults to the known corpus fixture pattern")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    rg = args.rg.resolve(strict=True)
    supplied_state = args.state.resolve(strict=True)
    corpus = args.corpus.resolve(strict=True)
    output = args.output_dir.resolve()
    if output.exists():
        parser.error("--output-dir must be new; preserve prior runs")
    if any(output == root or root in output.parents for root in (supplied_state, corpus)):
        parser.error("--output-dir must be outside the supplied state and corpus")
    if supplied_state == corpus or corpus in supplied_state.parents or supplied_state in corpus.parents:
        parser.error("the supplied state and corpus must be separate directory trees")
    if os.path.samefile(binary, rg):
        parser.error("--rg must be an independent ripgrep binary")
    rare = args.rare_pattern or RARE.get(corpus.name)
    if not rare:
        parser.error("provide --rare-pattern for this corpus name")

    source_before = inventory(corpus)
    state_before = inventory(supplied_state)
    original_manifest = json.loads((supplied_state / "manifest.json").read_text())
    if original_manifest.get("roots") != [str(corpus)]:
        parser.error("the supplied index must bind exactly the requested corpus root")
    output.mkdir(parents=True, mode=0o700)
    state = output / "state"
    # inventory rejects symlinks. Preserve any that appear in a copy race rather
    # than following them, then detect the race with the content inventory.
    shutil.copytree(supplied_state, state, symlinks=True)
    copied = inventory(state)
    if {key: value for key, value in copied.items() if key != "root"} != {
            key: value for key, value in state_before.items() if key != "root"}:
        raise RuntimeError("state changed during copying")
    if inventory(supplied_state) != state_before:
        raise RuntimeError("supplied state changed during copying")
    config = output / "query.ripgreprc"
    config.write_text("--threads=4\n")
    env = os.environ.copy()
    for key in ("TGREP_REAL_RG", "TGREP_STATE_DIR", "TGREP_PREFETCH"):
        env.pop(key, None)
    env.update(TGREP_STATE_DIR=str(state), RIPGREP_CONFIG_PATH=str(config))
    tools = {}
    for name, executable in (("tgrep", binary), ("rg", rg)):
        result, _ = run([str(executable), "--version"], env)
        tools[name] = {"path": str(executable), "sha256": digest_file(executable),
                       "version": result.stdout.decode(errors="replace"), "exit_code": result.returncode}
    if not tools["rg"]["version"].startswith("ripgrep "):
        raise RuntimeError("--rg must identify as ripgrep")
    report = {"platform": platform.platform(), "tools": tools, "corpus": source_before,
              "supplied_state": state_before, "copied_state": str(state),
              "conditions": "Warm filesystem/application caches; single observations, fresh processes, four query workers. No OS cache or ZFS ARC reset.",
              "allocated_scope": "regular-file st_blocks*512, excluding directory blocks and filesystem compression",
              "queries": {}, "initial": snapshot(state)}
    checkpoint(output / "results.json", report)

    def observed(command, name, timeout=120):
        result, milliseconds = run(command, env, timeout)
        (output / f"{name}.stdout").write_bytes(result.stdout)
        (output / f"{name}.stderr").write_bytes(result.stderr)
        return result, {"command": command, "wall_ms": milliseconds,
                        "exit_code": result.returncode,
                        "stdout_file": f"{name}.stdout", "stderr_file": f"{name}.stderr"}

    def query_checks(phase):
        rows = []
        for index, (flags, pattern) in enumerate(QUERIES + [(["-l", "-F"], rare)]):
            command = [*flags, "--", pattern, str(corpus)]
            oracle, oracle_observation = observed([str(rg), "--threads", "4", *command], f"{phase}-query-{index}-rg")
            actual, observation = observed([str(binary), *command], f"{phase}-query-{index}-tgrep")
            expected_paths = sorted(oracle.stdout.splitlines())
            equal = oracle.returncode in (0, 1) and actual.returncode == oracle.returncode and sorted(actual.stdout.splitlines()) == expected_paths
            row = {"flags": flags, "pattern": pattern, "equal": equal,
                   "matched_files": len(expected_paths), "rg": oracle_observation, "tgrep": observation,
                   "sorted_output_sha256": hashlib.sha256(b"\n".join(expected_paths)).hexdigest()}
            rows.append(row)
            report["queries"][phase] = rows
            checkpoint(output / "results.json", report)
            if not equal:
                raise RuntimeError(f"normal search mismatch: {phase}: {flags} {pattern}")
            if index == 4:
                if not expected_paths or len(expected_paths) > max(1, source_before["files"] // 10):
                    raise RuntimeError("rare pattern must match at least one and no more than 10% of corpus files")
                indexed, indexed_observation = observed([str(binary), "--indexed-only", *command], f"{phase}-query-{index}-indexed-only")
                indexed_equal = indexed.returncode == oracle.returncode and sorted(indexed.stdout.splitlines()) == expected_paths
                row["indexed_only"] = {**indexed_observation, "equal": indexed_equal}
                checkpoint(output / "results.json", report)
                if not indexed_equal:
                    raise RuntimeError(f"indexed-only mismatch for covered selective query: {phase}")
        return rows

    for phase in ("before", "after"):
        if phase == "after":
            report["before_compaction"] = snapshot(state)
            result, observation = observed([str(binary), "index", "compact"], "compact", timeout=900)
            report["compaction"] = observation
            report["after_compaction"] = snapshot(state)
            checkpoint(output / "results.json", report)
            if result.returncode:
                raise RuntimeError("compaction failed; inspect compact.stderr")
            before = report["before_compaction"]["active"]
            after = report["after_compaction"]["active"]
            if after["documents"] != before["documents"] or after["postings"] != before["postings"]:
                raise RuntimeError("compaction changed document/posting totals")
            if after["segments"] != min(before["segments"], 1):
                raise RuntimeError("compaction did not merge the segment set")
            if before["word_sidecars"] == before["segments"] and after["word_entries"] != before["word_entries"]:
                raise RuntimeError("compaction changed complete word-posting totals")
        status, status_observation = observed([str(binary), "index", "status"], f"{phase}-status", timeout=300)
        report[f"{phase}_status"] = status_observation
        if status.returncode:
            raise RuntimeError(f"index status failed {phase} compaction")
        query_checks(phase)
    if inventory(corpus) != source_before:
        raise RuntimeError("source corpus changed during compaction measurement")
    if inventory(supplied_state) != state_before:
        raise RuntimeError("supplied state changed during compaction measurement")
    report["source_unchanged"] = report["supplied_state_unchanged"] = True
    report["final"] = snapshot(state)
    checkpoint(output / "results.json", report)
    print(json.dumps({"result": str(output / "results.json"),
                      "before": report["before_compaction"]["active"],
                      "after": report["after_compaction"]["active"],
                      "compaction_ms": report["compaction"]["wall_ms"]}, indent=2))


if __name__ == "__main__":
    main()
