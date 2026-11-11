#!/usr/bin/env python3
"""Compare two tgrep executables and independent rg on preserved review corpora.

Inputs are the original audit's *-measurements.json files and immutable corpus
copies. This script never regenerates corpora from the working source tree.
Every measured query must agree on sorted filenames and exit status. Results
are checkpointed after each corpus and include byte-level input verification.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import statistics
import struct
import subprocess
import time
import zlib


def digest_file(path):
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def save_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


def verify_corpus(info):
    root = Path(info["root"])
    expected = {item["path"] for item in info["file_manifest"]}
    actual = set()
    for directory, subdirs, names in os.walk(root, followlinks=False):
        if any((Path(directory) / name).is_symlink() for name in subdirs):
            raise RuntimeError(f"Unexpected corpus directory symlink: {directory}")
        for name in names:
            path = Path(directory) / name
            if not stat.S_ISREG(path.lstat().st_mode):
                raise RuntimeError(f"Unexpected nonregular corpus entry: {path}")
            actual.add(str(path.relative_to(root)))
    if actual != expected:
        raise RuntimeError(f"Corpus file set changed: {root}")
    for item in info["file_manifest"]:
        path = root / item["path"]
        if path.stat().st_size != item["bytes"] or digest_file(path) != item["sha256"]:
            raise RuntimeError(f"Corpus content changed: {path}")
    manifest = json.dumps(info["file_manifest"], sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(manifest).hexdigest()


def footprint(state):
    groups = {}
    records = []
    for directory, _, names in os.walk(state, followlinks=False):
        for name in names:
            path = Path(directory) / name
            metadata = path.lstat()
            if not stat.S_ISREG(metadata.st_mode):
                continue
            relative = path.relative_to(state)
            kind = "wal" if "wal" in relative.parts else (path.suffix or path.name)
            group = groups.setdefault(kind, {"logical_bytes": 0, "allocated_bytes": 0, "files": 0})
            group["logical_bytes"] += metadata.st_size
            group["allocated_bytes"] += metadata.st_blocks * 512
            group["files"] += 1
            records.append({"path": str(relative), "bytes": metadata.st_size,
                            "allocated_bytes": metadata.st_blocks * 512})
    return {"logical_bytes": sum(group["logical_bytes"] for group in groups.values()),
            "allocated_bytes": sum(group["allocated_bytes"] for group in groups.values()),
            "by_kind": groups, "files": sorted(records, key=lambda item: item["path"]),
            "allocated_scope": "Regular-file st_blocks*512, no symlinks or directory blocks; not pool-level physical storage."}


def validate_layouts(state):
    """Independently check v2/v3 sizes, metadata/block CRCs and sidecar binding."""
    def require(condition, detail):
        if not condition:
            raise RuntimeError(f"Invalid generated format in {state}: {detail}")

    result = []
    manifest = json.loads((state / "manifest.json").read_text())
    for name in manifest["segments"]:
        path = state / "segments" / name
        data = path.read_bytes()
        magic, version, generation, _, documents, table, paths, dictionary, postings, block_index, footer = struct.unpack_from("<IHQQI6Q", data)
        require(magic == 0x54475331 and version == 2, "TGS magic/version")
        require(table == 74 and table <= paths <= dictionary <= postings <= footer and block_index == footer,
                "TGS offsets")
        require(footer + 16 == len(data), "TGS footer length")
        fingerprint, footer_magic, footer_docs = struct.unpack_from("<QII", data, footer)
        require(footer_magic == 0x5447464E and footer_docs == documents, "TGS footer identity")
        require(fingerprint == zlib.crc32(data[:postings]), "TGS metadata CRC")
        offset = table
        path_bytes = 0
        doc_ids = set()
        for _ in range(documents):
            identifier, _, length = struct.unpack_from("<III", data, offset)
            require(identifier not in doc_ids, "TGS repeated document ID")
            doc_ids.add(identifier)
            path_bytes += length
            offset += 52 + length
            require(offset <= paths, "TGS document record bounds")
        require(offset == paths and dictionary - paths == 12 * documents + path_bytes, "TGS path table size")
        require((postings - dictionary) % 16 == 0, "TGS dictionary size")
        grams = (postings - dictionary) // 16
        blocks = encoded_bytes = posting_count = 0
        offset = postings
        previous_gram = -1
        for entry in range(dictionary, postings, 16):
            gram, frequency, relative = struct.unpack_from("<IIQ", data, entry)
            require(gram > previous_gram and 0 < frequency <= documents, "TGS dictionary ordering/frequency")
            previous_gram = gram
            require(postings + relative == offset, "TGS contiguous posting ranges")
            posting_count += frequency
            for _ in range((frequency + 127) // 128):
                maximum, size, checksum = struct.unpack_from("<III", data, offset)
                require(offset + 12 + size <= footer and maximum in doc_ids, "TGS block bounds")
                payload = data[offset + 12:offset + 12 + size]
                require(checksum == zlib.crc32(payload, zlib.crc32(data[offset:offset + 8])), "TGS posting CRC")
                blocks += 1
                encoded_bytes += size
                offset += 12 + size
        require(offset == footer, "TGS posting end")
        expected_tgs = 90 + 64 * documents + 2 * path_bytes + 16 * grams + 12 * blocks + encoded_bytes
        require(len(data) == expected_tgs, "TGS size formula")
        row = {"segment": name, "documents": documents, "path_bytes": path_bytes,
               "grams": grams, "posting_blocks": blocks, "posting_count": posting_count,
               "encoded_varint_bytes": encoded_bytes, "tgs_bytes": len(data),
               "tgs_formula_bytes": expected_tgs, "metadata_fingerprint": fingerprint}
        word_path = path.with_suffix(".qwi")
        if word_path.exists():
            word = word_path.read_bytes()
            magic, version, count, word_generation, word_blocks, metadata_crc, entry_size, block_entries, reserved, binding = struct.unpack_from("<IIQQQIIIIQ", word)
            require((magic, version, word_generation, entry_size, block_entries, reserved, binding)
                    == (0x54575149, 3, generation, 12, 512, 0, fingerprint), "QWI header and segment binding")
            require(word_blocks == (count + 511) // 512 and len(word) == 56 + 24 * word_blocks + 12 * count,
                    "QWI size formula")
            payload_start = 56 + 24 * word_blocks
            header = bytearray(word[:56])
            header[32:36] = bytes(4)
            require(metadata_crc == zlib.crc32(word[56:payload_start], zlib.crc32(header)), "QWI metadata CRC")
            previous = None
            total = 0
            for block in range(word_blocks):
                first, last, checksum, entries = struct.unpack_from("<QQII", word, 56 + 24 * block)
                require(entries == min(512, count - total), "QWI block count")
                payload = word[payload_start + total * 12:payload_start + (total + entries) * 12]
                require(checksum == zlib.crc32(payload), "QWI payload CRC")
                require(first == struct.unpack_from(">Q", payload)[0] and last == struct.unpack_from(">Q", payload, len(payload) - 12)[0],
                        "QWI directory boundaries")
                for pair in struct.iter_unpack(">QI", payload):
                    require(pair[1] in doc_ids and (previous is None or previous < pair), "QWI order/document binding")
                    previous = pair
                total += entries
            row.update({"qwi_entries": count, "qwi_blocks": word_blocks, "qwi_bytes": len(word),
                        "qwi_formula_bytes": 56 + 24 * word_blocks + 12 * count})
        result.append(row)
    return result


def environment(state):
    env = os.environ.copy()
    for key in ["TGREP_REAL_RG", "TGREP_PREFETCH", "TGREP_STATE_DIR", "RIPGREP_CONFIG_PATH"]:
        env.pop(key, None)
    env["RIPGREP_CONFIG_PATH"] = os.devnull
    env["TGREP_STATE_DIR"] = str(state)
    return env


def invoke(command, env, timeout=180):
    started = time.perf_counter_ns()
    result = subprocess.run(command, env=env, capture_output=True, timeout=timeout, check=False)
    elapsed_ms = (time.perf_counter_ns() - started) / 1e6
    return result, elapsed_ms


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--ripgrep", type=Path, default=Path(shutil.which("rg") or "/usr/bin/rg"))
    parser.add_argument("--audit-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--trials", type=int, default=9)
    args = parser.parse_args()
    if args.trials < 1:
        parser.error("--trials must be positive")
    output = args.output.resolve()
    if output.exists():
        parser.error("--output must be a fresh directory")
    binaries = {name: getattr(args, name).resolve() for name in ("baseline", "candidate", "ripgrep")}
    if any(os.path.samefile(binaries["ripgrep"], binaries[name]) for name in ("baseline", "candidate")):
        parser.error("ripgrep must be independent of both tgrep executables")
    for path in binaries.values():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"Not an executable: {path}")
    version_env = environment(output / "unused-state")
    identities = {}
    for name, binary in binaries.items():
        observed, _ = invoke([str(binary), "--version"], version_env)
        if observed.returncode != 0:
            raise RuntimeError(f"Cannot identify {name}: {observed.stderr!r}")
        version = observed.stdout.decode(errors="replace").strip()
        if name == "ripgrep" and not version.startswith("ripgrep "):
            raise RuntimeError("The reference executable is not ripgrep")
        identities[name] = {"path": str(binary), "bytes": binary.stat().st_size,
                            "sha256": digest_file(binary), "version": version}
    output.mkdir(parents=True)
    report = {"status": "running", "binaries": identities, "trials_per_query": args.trials,
              "method": "One warmup, rotating three-tool order, fresh processes, warm filesystem cache; four search workers, no OS/ARC cache reset.",
              "corpora": []}
    try:
        for label, rare in [("tgrep", "TGREP_QWI_ENTRY_SIZE"),
                            ("keystone", "dsmil_hash_index_save"),
                            ("cpython", "unicode_encode_call_errorhandler")]:
            original = json.loads((args.audit_dir / f"{label}-measurements.json").read_text())
            info = original["corpus"]
            fingerprint = verify_corpus(info)
            row = {"label": label, "corpus": info, "content_manifest_sha256": fingerprint,
                   "original_audit_footprint": original["footprint_after_build"], "builds": {}, "queries": []}
            report["corpora"].append(row)
            for name in ("baseline", "candidate"):
                state = output / "states" / name / label
                command = [str(binaries[name]), "index", "build", info["root"],
                           "--threads", "2", "--max-memory", "2GiB", "--io-limit", "1GiB/s"]
                print(f"Building {name}/{label}: {info['files']} files, {info['source_bytes']} bytes", flush=True)
                result, wall_ms = invoke(command, environment(state), timeout=900)
                (output / f"{name}-{label}-build.log").write_bytes(result.stderr + result.stdout)
                if result.returncode:
                    raise RuntimeError(f"Build failed for {name}/{label}: {result.stderr[-2000:]!r}")
                status, _ = invoke([str(binaries[name]), "index", "status"], environment(state))
                if status.returncode:
                    raise RuntimeError(f"Status failed for {name}/{label}: {status.stderr!r}")
                row["builds"][name] = {"state": str(state), "command": command, "wall_ms": wall_ms,
                                       "exit_code": result.returncode, "footprint_after_build": footprint(state),
                                       "status_stdout": status.stdout.decode(errors="replace"),
                                       "status_stderr": status.stderr.decode(errors="replace"),
                                       "format_validation": validate_layouts(state)}
            queries = [(["-l", "-F"], "TGREP_REVIEW_ABSENT_918273645"),
                       (["-l", "-F"], "return"), (["-l", "-w"], "return"),
                       (["-l", "-i"], "return"), (["-l", "-F"], rare)]
            for flags, pattern in queries:
                query = {"flags": flags, "pattern": pattern, "trials": [], "output_equal": True}
                commands = {name: [str(binary), *(["--threads", "4"] if name == "ripgrep" else []),
                                   *flags, pattern, info["root"]] for name, binary in binaries.items()}
                envs = {name: environment(output / "states" / name / label) for name in binaries}
                for name in binaries:
                    warmup, _ = invoke(commands[name], envs[name])
                    if warmup.returncode not in (0, 1) or warmup.stderr:
                        raise RuntimeError(f"Warmup failed: {name}/{label}: {warmup.stderr!r}")
                names = list(binaries)
                for trial in range(args.trials):
                    observations = {}
                    lines = {}
                    order = names[trial % len(names):] + names[:trial % len(names)]
                    for name in order:
                        observed, wall_ms = invoke(commands[name], envs[name])
                        if observed.returncode not in (0, 1) or observed.stderr:
                            raise RuntimeError(f"Query failed: {name}/{label}: {observed.stderr!r}")
                        lines[name] = sorted(observed.stdout.splitlines())
                        observations[name] = {"wall_ms": wall_ms, "exit_code": observed.returncode,
                                              "matched_files": len(lines[name]),
                                              "output_sha256_sorted": hashlib.sha256(b"\n".join(lines[name])).hexdigest()}
                    equal = all(lines[name] == lines["ripgrep"] and
                                observations[name]["exit_code"] == observations["ripgrep"]["exit_code"]
                                for name in ("baseline", "candidate"))
                    query["output_equal"] &= equal
                    query["trials"].append({"order": order, **observations})
                    if not equal:
                        raise RuntimeError(f"Oracle mismatch: {label}, {flags}, {pattern}")
                query["median_ms"] = {name: statistics.median(item[name]["wall_ms"] for item in query["trials"])
                                      for name in binaries}
                query["matched_files"] = len(lines["ripgrep"])
                explain, _ = invoke([str(binaries["candidate"]), "--explain", *flags, pattern, info["root"]], envs["candidate"])
                if explain.returncode:
                    raise RuntimeError(f"Explain failed: {label}: {explain.stderr!r}")
                query["candidate_explain"] = (explain.stdout + explain.stderr).decode(errors="replace")
                row["queries"].append(query)
                print(f"{label} {' '.join(flags)} {pattern}: {query['median_ms']}, equal=True", flush=True)
            for name in ("baseline", "candidate"):
                row["builds"][name]["footprint_after_queries"] = footprint(Path(row["builds"][name]["state"]))
            if verify_corpus(info) != fingerprint:
                raise RuntimeError(f"Corpus manifest changed during run: {label}")
            row["source_verified_after"] = True
            save_json(output / "results.json", report)
        report["status"] = "passed"
    except BaseException as error:
        report["status"] = "failed"
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        save_json(output / "results.json", report)


if __name__ == "__main__":
    main()
