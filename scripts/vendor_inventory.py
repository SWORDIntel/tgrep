#!/usr/bin/env python3
"""Refresh/check native source hashes and compiler-observed header dependencies."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "vendor/manifest.json"
BASELINE = "22d146ad3751597711b57116060e84cf169fc87b"
KEYSTONE_REFERENCE = "da8e31fc92a4bb35b572b2935aed8d8377cc9e1b"


def run(arguments, cwd=ROOT):
    return subprocess.run(arguments, cwd=cwd, check=True, capture_output=True).stdout


def digest(data):
    return hashlib.sha256(data).hexdigest()


def tree(repository, revision, prefix=None, wanted=None):
    command = ["git", "ls-tree", "-rz", "--name-only", revision]
    if prefix:
        command += ["--", prefix]
    names = run(command, repository).decode().split("\0")
    return {
        name: digest(run(["git", "show", f"{revision}:{name}"], repository))
        for name in names if name and (wanted is None or name in wanted)
    }


def native_build(compiler, simd):
    source = (ROOT / "build.rs").read_text().split('build.compile("tgrep_native")')[0]
    values = {}
    for method in ("file", "include", "flag"):
        values[method] = re.findall(r'\.' + method + r'\("([^"]+)"\)', source)
    for method in ("file", "include"):
        if len(values[method]) != len(re.findall(r'\.' + method + r'\(', source)):
            raise ValueError(f"build.rs has a dynamic {method} call; update inventory parsing")
    if '.files(' in source or source.count('build.flag(flag)') != 1:
        raise ValueError("build.rs native source/flag selection changed; update inventory parsing")
    default_simd = re.search(
        r'var\("TGREP_SIMD_FLAGS"\).*?unwrap_or_else\(\|_\|\s*"([^"]+)"\.to_string\(\)\)',
        source, re.S,
    )
    if not default_simd:
        raise ValueError("cannot determine build.rs default SIMD flags")
    selected = simd if simd is not None else os.environ.get("TGREP_SIMD_FLAGS", default_simd[1])
    flags = values["flag"] + shlex.split(selected)
    includes = [item for directory in values["include"] for item in ("-I", directory)]
    units = {}
    for name in values["file"]:
        output = run(compiler + flags + includes + ["-MM", "-MT", "inventory", name]).decode()
        dependencies = shlex.split(output.replace("\\\n", " "))
        if not dependencies or dependencies[0] != "inventory:":
            raise ValueError(f"unexpected compiler dependency output for {name}")
        headers = set()
        for dependency in dependencies[1:]:
            relative = (ROOT / dependency).resolve().relative_to(ROOT).as_posix()
            if relative != name:
                headers.add(relative)
        units[name] = {"sha256": digest((ROOT / name).read_bytes()), "headers": sorted(headers)}
    return {
        "compiler_command": compiler,
        "compiler_version": run(compiler + ["--version"]).decode().splitlines()[0],
        "compiler_target": run(compiler + ["-dumpmachine"]).decode().strip(),
        "flags": flags, "include_directories": values["include"], "translation_units": units,
    }


def inventory(arguments, previous):
    baseline = tree(ROOT, BASELINE, "vendor")
    paths = sorted(path for path in (ROOT / "vendor").rglob("*")
                   if path.is_file() and path not in (MANIFEST, MANIFEST.with_suffix(".json.tmp")))
    files = {}
    for path in paths:
        if path.is_symlink():
            raise ValueError(f"inventory refuses to follow vendor symlink: {path.relative_to(ROOT)}")
        name = path.relative_to(ROOT).as_posix()
        data = path.read_bytes()
        sha = digest(data)
        status = "added" if name not in baseline else "unchanged" if sha == baseline[name] else "modified"
        files[name] = {"bytes": len(data), "sha256": sha, "reviewed_baseline": status}
        if status == "modified":
            files[name]["reviewed_baseline_sha256"] = baseline[name]
    reference = previous.get("keystone_comparison", {}).get("reference_sha256", {})
    if arguments.keystone_repo:
        wanted = {name.removeprefix("vendor/KEYSTONE/") for name in files
                  if name.startswith("vendor/KEYSTONE/")}
        available = tree(arguments.keystone_repo, KEYSTONE_REFERENCE, wanted=wanted)
        reference = {name.removeprefix("vendor/KEYSTONE/"): available.get(name.removeprefix("vendor/KEYSTONE/"))
                     for name in files if name.startswith("vendor/KEYSTONE/")}
    comparison = {
        name.removeprefix("vendor/KEYSTONE/"): (
            "not_in_reference_inventory" if name.removeprefix("vendor/KEYSTONE/") not in reference
            else "absent_at_reference" if reference[name.removeprefix("vendor/KEYSTONE/")] is None
            else "identical" if metadata["sha256"] == reference[name.removeprefix("vendor/KEYSTONE/")]
            else "different"
        ) for name, metadata in files.items() if name.startswith("vendor/KEYSTONE/")
    }
    return {
        "schema_version": 1,
        "reviewed_repository_baseline": BASELINE,
        "baseline_is_upstream_import": False,
        "native_build": native_build(shlex.split(arguments.cc), arguments.simd_flags),
        "vendor_files": files,
        "removed_since_reviewed_baseline": sorted(set(baseline) - set(files)),
        "keystone_comparison": {
            "revision": KEYSTONE_REFERENCE, "is_upstream_import_baseline": False,
            "reference_sha256": reference, "current_comparison": comparison,
        },
        "original_import_revisions": {"KEYSTONE": None, "QIHSE": None},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--refresh", action="store_true")
    mode.add_argument("--check", action="store_true")
    parser.add_argument("--cc", default=os.environ.get("CC", "cc"))
    parser.add_argument("--simd-flags", help="defaults to TGREP_SIMD_FLAGS or build.rs default")
    parser.add_argument("--keystone-repo", type=Path, help="local comparison checkout; never fetches")
    arguments = parser.parse_args()
    previous = json.loads(MANIFEST.read_text()) if MANIFEST.exists() else {}
    current = inventory(arguments, previous)
    if arguments.check:
        if current != previous:
            raise ValueError("vendor inventory is stale; review changes and run --refresh")
    else:
        temporary = MANIFEST.with_suffix(".json.tmp")
        temporary.write_text(json.dumps(current, indent=2, sort_keys=True) + "\n")
        temporary.replace(MANIFEST)
    units = current["native_build"]["translation_units"]
    headers = {header for unit in units.values() for header in unit["headers"]}
    files = current["vendor_files"]
    reached = (set(units) | headers) & set(files)
    print(f"{'Checked' if arguments.check else 'Refreshed'} {len(files)} vendor files; "
          f"{len(units)} native translation units, {len(headers)} reachable local headers, "
          f"{len(reached)} vendor files used by this compile profile")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"vendor inventory: {error}", file=sys.stderr)
        sys.exit(1)
