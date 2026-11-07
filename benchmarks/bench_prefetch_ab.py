#!/usr/bin/env python3
"""Paired page-cache-cold A/B benchmark for speculative file prefetch."""

from __future__ import annotations

import argparse
import csv
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path


def run(command: list[str], env: dict[str, str]) -> tuple[int, bytes, int]:
    start = time.perf_counter_ns()
    result = subprocess.run(command, env=env, stdout=subprocess.PIPE, check=False)
    elapsed_ms = (time.perf_counter_ns() - start) / 1_000_000
    if result.returncode not in (0, 1):
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(command)}"
        )
    return result.returncode, result.stdout, round(elapsed_ms * 1000)


def drop_page_cache() -> None:
    # Drop Linux page cache only; keep dentries/inodes. This is system-wide and
    # requires passwordless sudo. ZFS ARC (or other lower-level caches) is not
    # cleared by this operation and remains an explicit benchmark limitation.
    result = subprocess.run(
        ["sudo", "-n", "sh", "-c", "echo 1 > /proc/sys/vm/drop_caches"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(
            "could not drop Linux page cache: "
            + result.stderr.decode(errors="replace").strip()
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("pattern")
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--tgrep", type=Path, required=True)
    parser.add_argument("--trials", type=int, default=5)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.trials < 1:
        parser.error("--trials must be positive")

    env = os.environ.copy()
    env["TGREP_STATE_DIR"] = str(args.state)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    times: dict[str, list[float]] = {"enabled": [], "disabled": []}
    rows: list[dict[str, str | int]] = []
    expected: tuple[int, bytes] | None = None

    for trial in range(1, args.trials + 1):
        order = ("enabled", "disabled") if trial % 2 else ("disabled", "enabled")
        pair: dict[str, tuple[int, bytes, int]] = {}
        for mode in order:
            clear = subprocess.run(
                [str(args.tgrep), "cache", "clear"],
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            if clear.returncode:
                raise RuntimeError(
                    "cache clear failed: " + clear.stderr.decode(errors="replace")
                )
            drop_page_cache()
            command_env = env.copy()
            if mode == "enabled":
                command_env["TGREP_PREFETCH"] = "1"
            else:
                command_env.pop("TGREP_PREFETCH", None)
            command = [
                str(args.tgrep),
                "-F",
                "-l",
                "--",
                args.pattern,
                str(args.corpus),
            ]
            pair[mode] = run(command, command_env)

        enabled, disabled = pair["enabled"], pair["disabled"]
        if enabled[:2] != disabled[:2]:
            raise RuntimeError(f"prefetch modes returned different results at trial {trial}")
        if expected is None:
            expected = enabled[:2]
        elif expected != enabled[:2]:
            raise RuntimeError(f"search results changed at trial {trial}")
        times["enabled"].append(enabled[2] / 1000)
        times["disabled"].append(disabled[2] / 1000)
        rows.append(
            {
                "trial": trial,
                "first": order[0],
                "prefetch_enabled_ms": f"{enabled[2] / 1000:.3f}",
                "prefetch_disabled_ms": f"{disabled[2] / 1000:.3f}",
                "files_matched": len([line for line in enabled[1].splitlines() if line]),
            }
        )
        print(
            f"trial {trial}: enabled={enabled[2] / 1000:.3f}ms "
            f"disabled={disabled[2] / 1000:.3f}ms ({order[0]} first)",
            flush=True,
        )

    with args.output.open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=rows[0].keys())
        writer.writeheader()
        writer.writerows(rows)
    print(f"files matched: {rows[0]['files_matched']}")
    print(
        "median ms: enabled={:.3f}, disabled={:.3f}".format(
            statistics.median(times["enabled"]), statistics.median(times["disabled"])
        )
    )
    print(f"raw paired trials: {args.output}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError) as error:
        print(f"benchmark error: {error}", file=sys.stderr)
        raise SystemExit(2)
