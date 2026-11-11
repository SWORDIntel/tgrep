#!/usr/bin/env python3
"""Measure one large matching file without retaining query output in RAM.

Requires Linux wait4 peak-RSS units (KiB). All executable and output paths are
explicit. A fresh output directory contains the generated corpus, isolated
configuration/state, raw output/error files, and results.json. This measures
formatted-output memory, not the searcher's behavior on a single huge line.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import sys
import time


def digest(path: Path) -> str:
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def inspect_binary(path: Path, expected_prefix: str) -> dict:
    result = subprocess.run(
        [str(path), "--version"],
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        check=True,
        timeout=10,
        env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"},
    )
    version = result.stdout.strip()
    if not version.startswith(expected_prefix):
        raise RuntimeError(f"unexpected executable identity for {path}: {version!r}")
    return {"path": str(path), "sha256": digest(path), "version": version}


def measured_run(
    command: list[str],
    environment: dict[str, str],
    directory: Path,
    timeout: float,
) -> dict:
    directory.mkdir()
    stdout_path = directory / "stdout"
    stderr_path = directory / "stderr"
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        start = time.perf_counter()
        child = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
            env=environment,
            start_new_session=True,
        )
        try:
            while True:
                pid, status, usage = os.wait4(child.pid, os.WNOHANG)
                if pid:
                    child.returncode = os.waitstatus_to_exitcode(status)
                    break
                if time.perf_counter() - start > timeout:
                    raise TimeoutError(f"query exceeded {timeout:g} seconds: {command!r}")
                time.sleep(0.005)
        finally:
            if child.returncode is None:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                _, status, _ = os.wait4(child.pid, 0)
                child.returncode = os.waitstatus_to_exitcode(status)
        elapsed = time.perf_counter() - start
    return {
        "command": command,
        "exit_code": child.returncode,
        "elapsed_seconds": elapsed,
        "peak_rss_kib": usage.ru_maxrss,
        "user_seconds": usage.ru_utime,
        "system_seconds": usage.ru_stime,
        "minor_page_faults": usage.ru_minflt,
        "major_page_faults": usage.ru_majflt,
        "stdout_bytes": stdout_path.stat().st_size,
        "stdout_sha256": digest(stdout_path),
        "stdout_path": str(stdout_path),
        "stderr_bytes": stderr_path.stat().st_size,
        "stderr_path": str(stderr_path),
    }


def positive_integer(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return parsed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-bin", required=True, type=Path)
    parser.add_argument("--candidate-bin", required=True, type=Path)
    parser.add_argument("--rg", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--source-mib", type=positive_integer, default=128)
    parser.add_argument("--trials", type=positive_integer, default=1)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    if platform.system() != "Linux":
        parser.error("this benchmark reports Linux wait4 ru_maxrss in KiB")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    paths = {
        "baseline": args.baseline_bin.resolve(strict=True),
        "candidate": args.candidate_bin.resolve(strict=True),
        "rg": args.rg.resolve(strict=True),
    }
    if any(paths["rg"].samefile(paths[name]) for name in ("baseline", "candidate")):
        parser.error("--rg must name an independent ripgrep executable")
    binaries = {
        name: inspect_binary(path, "ripgrep " if name == "rg" else "tgrep ")
        for name, path in paths.items()
    }
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    corpus = output / "corpus"
    corpus.mkdir()
    temporary = output / "tmp"
    temporary.mkdir(mode=0o700)
    home = output / "home"
    home.mkdir(mode=0o700)
    config = output / "ripgrep-config"
    config.write_text("--threads=4\n", encoding="utf-8")
    source = corpus / "matching.txt"
    line = b"needle " + b"x" * 1016 + b"\n"
    assert len(line) == 1024
    block = line * 1024
    with source.open("wb") as stream:
        for _ in range(args.source_mib):
            stream.write(block)
    source_bytes = source.stat().st_size
    source_sha256 = digest(source)  # Also reads the source before timed queries.
    common_arguments = ["-F", "needle", str(corpus)]
    results = {
        "schema_version": 1,
        "environment": {
            "platform": platform.platform(),
            "python": platform.python_version(),
            "cache_regime": (
                "Source created and read before trials; no OS cache flush. "
                "Each query has a fresh absent index/cache state directory. "
                "No cold-cache, ZFS ARC, or stable latency claim."
            ),
            "measurement": (
                "os.wait4 measures only the queried executable; ru_maxrss is KiB. "
                "stdout is written directly to a file and hashed afterward in bounded chunks. "
                "Elapsed time includes process startup and up to 5 ms exit polling delay."
            ),
        },
        "binaries": binaries,
        "source": {
            "path": str(source),
            "bytes": source_bytes,
            "sha256": source_sha256,
            "line_bytes": len(line),
            "line_count": source_bytes // len(line),
        },
        "runs": [],
        "complete": False,
    }
    try:
        names = ["baseline", "candidate", "rg"]
        for trial in range(args.trials):
            order = names[trial % len(names) :] + names[: trial % len(names)]
            trial_results = []
            for name in order:
                state = output / f"state-{trial}-{name}"
                environment = {
                    "PATH": "/usr/bin:/bin",
                    "LANG": "C",
                    "LC_ALL": "C",
                    "HOME": str(home),
                    "XDG_STATE_HOME": str(state / "xdg"),
                    "TGREP_STATE_DIR": str(state),
                    "RIPGREP_CONFIG_PATH": str(config),
                    "TMPDIR": str(temporary),
                }
                run = measured_run(
                    [str(paths[name]), *common_arguments],
                    environment,
                    output / f"run-{trial}-{name}",
                    args.timeout,
                )
                run.update({"tool": name, "trial": trial, "temporary_files_left": len(list(temporary.iterdir()))})
                results["runs"].append(run)
                trial_results.append(run)
                if run["exit_code"] != 0 or run["stderr_bytes"] != 0 or run["temporary_files_left"]:
                    raise RuntimeError(f"query or spool cleanup failed: {name}, trial {trial}")
                print(
                    f"trial {trial} {name}: {run['stdout_bytes']} output bytes, "
                    f"{run['peak_rss_kib']} KiB peak RSS",
                    flush=True,
                )
            outputs = {(run["stdout_bytes"], run["stdout_sha256"]) for run in trial_results}
            if len(outputs) != 1:
                raise RuntimeError(f"query output differs from ripgrep in trial {trial}")
        if source.stat().st_size != source_bytes or digest(source) != source_sha256:
            raise RuntimeError("source changed during measurement")
        results["complete"] = True
        results["all_outputs_equal_to_rg"] = True
    finally:
        (output / "results.json").write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
    print(f"Saved {output / 'results.json'}", flush=True)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"output-memory measurement failed: {error}", file=sys.stderr)
        raise SystemExit(1)
