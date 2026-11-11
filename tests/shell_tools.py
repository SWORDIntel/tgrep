#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-SRSL-1.1
"""Exercise copied shell scripts with private HOME, state, commands and build artifacts."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest


class Fixture:
    def __init__(self, case, group, checkout="checkout", prefix="prefix", benchmarks="benchmarks", script=None):
        temporary = tempfile.TemporaryDirectory(prefix="case-", dir=OPTIONS.scratch)
        case.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.repo = self.root / checkout
        self.home = self.root / "home"
        self.bin = self.root / prefix / "bin"
        self.caller = self.root / "caller"
        self.tools = self.root / "commands"
        for path in (self.repo, self.home, self.caller, self.tools, self.root / "tmp"):
            path.mkdir(parents=True)
        self.config = self.home / ".config/tgrep"
        self.state = self.home / ".local/state/tgrep"
        self.config.mkdir(parents=True)
        self.state.mkdir(parents=True)
        (self.config / "rg_path").write_text("existing configuration\n")
        (self.state / "sentinel").write_text("existing index\n")
        self.settings_path = self.root / "settings.json"
        self.settings = {
            "root": str(self.root), "repo": str(self.repo), "bin": str(self.bin),
            "config_dir": str(self.config), "group": group,
            "log": str(self.root / "events.jsonl"), "output": str(self.root / "output.json"),
            "fallback": str(self.root / "fallback-rg"), "candidates": [],
        }
        self.helper = OPTIONS.root / "tests/shell/fake_tools.py"
        self.shell_helper = OPTIONS.root / "tests/shell/fake_tools.sh"
        self.env = {
            **os.environ,
            "HOME": str(self.home),
            "XDG_CONFIG_HOME": str(self.home / ".config"),
            "XDG_STATE_HOME": str(self.home / ".local/state"),
            "XDG_CACHE_HOME": str(self.home / ".cache"),
            "TMPDIR": str(self.root / "tmp"),
            "PATH": f"{self.tools}:/usr/bin:/bin",
            "TGREP_FAKE_HOST_TRIPLE": "x86_64-unknown-linux-gnu",
            "TGREP_SHELL_FIXTURE": str(self.settings_path),
            "TGREP_TEST_MARKER": str(self.root / "injection-marker"),
            "TGREP_FAKE_QUERY_LOG": str(self.root / "queries.bin"),
            "TGREP_FAKE_SEARCH_MODE": "match",
            "TGREP_FAKE_SIBLING_SENTINEL": str(self.root / "checkout/target/sentinel"),
        }
        for name in ("readlink", "which", "zfs", "zpool", "sudo", "sync", "cargo",
                     "install", "mkdir", "rm", "touch", "rustc"):
            self.wrapper(self.tools / name, name)
        if group == "packaging":
            for name in ("dpkg", "dpkg-architecture", "dpkg-shlibdeps", "dpkg-deb",
                         "grep", "rg", "seq", "find"):
                self.wrapper(self.tools / name, name)
        self.wrapper(Path(self.settings["fallback"]), "probe-invalid")
        self.real_rg = self.candidate("real-rg")
        self.settings["candidates"] = [str(self.real_rg)]
        relative = {"installer": "install.sh", "compile": "compile.sh",
                    "runner": "benchmarks/run_benchmarks.sh",
                    "packaging": "scripts/build_deb.sh"}[group]
        relative = script or relative
        destination = f"{benchmarks}/run_benchmarks.sh" if group == "runner" else relative
        self.script = self.repo / destination
        self.script.parent.mkdir(parents=True, exist_ok=True)
        self.script.write_bytes((OPTIONS.root / relative).read_bytes())
        if group == "installer":
            self.wrapper(self.repo / "compile.sh", "compile")
        if group == "runner":
            self.wrapper(self.repo / "target/release/tgrep", "tgrep")
            self.wrapper(self.tools / "rg", "rg")
            self.env["RG_BIN"] = str(self.tools / "rg")
            (self.caller / "relative-corpus").mkdir()
        if group == "packaging":
            # Synthetic license/header and control data test mechanics, never release provenance.
            (self.repo / "LICENSE").write_text("GNU AFFERO GENERAL PUBLIC LICENSE\nfixture only\n")
            (self.repo / "README.md").write_text("synthetic package documentation\n")
            control = self.repo / "packaging/deb/root/DEBIAN"
            control.mkdir(parents=True)
            (control / "control").write_text("Package: tgrep\nVersion: 0.0.0\n"
                                             "Architecture: amd64\nDepends: libc6\n")
            (control / "postinst").write_text("#!/bin/sh\nexit 0\n")
            corpus = self.root / "corpus"
            corpus.mkdir()
            self.env.update({
                "TGREP_DIST_DIR": str(self.root / "review-output"),
                "TGREP_CORPUS": str(corpus), "TGREP_NVME_CORPUS": str(corpus),
                "TGREP_ZFS_CORPUS": str(corpus),
                "TGREP_FAKE_PACKAGE_CONTROL": str(self.root / "captured-control"),
            })
        self.wrapper(self.root / "backend-template", "tgrep", structured=True)
        self.env.update({
            "TGREP_FAKE_ROOT": str(self.root),
            "TGREP_FAKE_REPO": str(self.repo),
            "TGREP_FAKE_BIN": str(self.bin),
            "TGREP_FAKE_CONFIG_DIR": str(self.config),
            "TGREP_FAKE_FALLBACK": self.settings["fallback"],
            "TGREP_FAKE_BACKEND": str(self.root / "backend-template"),
        })
        self.save()

    def save(self, **changes):
        self.settings.update(changes)
        self.env.update({
            "TGREP_FAKE_SEARCH_MODE": self.settings.get("search_mode", "match"),
            "TGREP_FAKE_CANDIDATES": "\n".join(self.settings["candidates"]),
            "TGREP_FAKE_BUILD_FAILURE": str(self.settings.get("build_failure", False)).lower(),
            "TGREP_FAKE_ARRIVE": str(self.settings.get("arrive_during_build", False)).lower(),
            "TGREP_FAKE_FAIL_MKDIR": self.settings.get("fail_mkdir", ""),
            "TGREP_FAKE_FAIL_INSTALL": str(self.settings.get("fail_install", 0)),
            "TGREP_FAKE_FAIL_REMOVE": self.settings.get("fail_remove", ""),
        })
        self.settings_path.write_text(json.dumps(self.settings))

    def wrapper(self, path, role, structured=False):
        path.parent.mkdir(parents=True, exist_ok=True)
        if self.settings["group"] == "runner" and role in ("tgrep", "rg", "wrong-tgrep"):
            # Keep the 19-query fixture inexpensive without changing production patterns.
            path.write_text("#!/bin/sh\n" + "role=" + shlex.quote(role) + "\n" + r'''{
    printf '%s\000%s\000%s\000' "$role" "$PWD" "$0"
    printf '%s\000' "$@"
    printf '\000'
} >> "$TGREP_FAKE_QUERY_LOG"
if [ "$role" = wrong-tgrep ]; then rm -f -- "$TGREP_FAKE_SIBLING_SENTINEL"; fi
if [ "$1" = --version ]; then printf 'ripgrep 15.1.0\n'; exit 0; fi
mode="$TGREP_FAKE_SEARCH_MODE"
if [ "$1" = cache ]; then
    [ "$mode" != cache_error ] || exit 2
    exit 0
fi
if [ "$mode" = error ] && [ "$role" != rg ]; then exit 2; fi
if [ "$mode" = none ]; then exit 1; fi
if [ "$mode" = mismatch ] && [ "$role" = rg ]; then
    printf 'different-file\n'
else
    printf 'fileA\n'
fi
''')
            path.chmod(0o755)
            return path
        if role == "cargo" or structured or (role == "tgrep" and self.settings["group"] == "installer"):
            arguments = (sys.executable, str(self.helper), role)
        else:
            arguments = ("/bin/bash", str(self.shell_helper), role)
        command = " ".join(shlex.quote(value) for value in arguments)
        path.write_text(f'#!/bin/sh\nexec {command} "$0" "$@"\n')
        path.chmod(0o755)
        return path

    def candidate(self, name, mode="valid"):
        return self.wrapper(self.root / name, "probe-" + mode)

    def run(self, *arguments):
        self.save()
        return subprocess.run(["/bin/bash", str(self.script), *arguments], cwd=self.caller,
                              env=self.env, capture_output=True, text=True, timeout=30)

    def install(self):
        return self.run("--prefix", str(self.bin.parent), "--no-index", "--no-zfs")

    def uninstall(self):
        return self.run("--prefix", str(self.bin.parent), "--uninstall")

    def events(self, tool=None):
        path = Path(self.settings["log"])
        events = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        query_log = self.root / "queries.bin"
        if query_log.exists():
            for entry in query_log.read_bytes().split(b"\0\0"):
                if entry:
                    role, cwd, invoked, *arguments = map(os.fsdecode, entry.split(b"\0"))
                    events.append({"tool": role, "cwd": cwd, "args": arguments,
                                   "invoked": str(Path(invoked).resolve())})
        return [event for event in events if tool is None or event["tool"] == tool]

    def home_snapshot(self):
        return {str(path.relative_to(self.home)): path.read_bytes()
                for path in self.home.rglob("*") if path.is_file()}

    def receipt(self):
        return self.bin / ".tgrep-install.sha256"

    def write_receipt(self):
        hashes = [hashlib.sha256((self.bin / name).read_bytes()).hexdigest()
                  for name in ("tgrep", "rg")]
        self.receipt().write_text("\n".join(hashes) + "\n")

    def sibling(self):
        path = self.root / "checkout"
        (path / "target").mkdir(parents=True, exist_ok=True)
        (path / "Cargo.toml").write_text("trimmed sibling manifest\n")
        (path / "target/sentinel").write_text("Cargo build sentinel\n")
        self.settings["sibling"] = str(path)
        self.wrapper(path / "compile.sh", "wrong-compile")
        self.wrapper(path / "target/release/tgrep", "wrong-tgrep")
        return path


class InstallerTests(unittest.TestCase):
    def fixture(self, **kwargs):
        return Fixture(self, "installer", **kwargs)

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_wrapper_quotes_paths_and_preserves_configuration(self):
        for prefix in ("prefix with spaces'quoted", 'prefix$(touch "$TGREP_TEST_MARKER")'):
            with self.subTest(prefix=prefix):
                fixture = self.fixture(prefix=prefix)
                before = fixture.home_snapshot()
                self.assert_success(fixture.install())
                result = subprocess.run([str(fixture.bin / "rg"), "two words", "--", "a'b"],
                                        env=fixture.env, capture_output=True, text=True, timeout=5)
                self.assert_success(result)
                output = json.loads(Path(fixture.settings["output"]).read_text())
                self.assertEqual(output, {"args": ["two words", "--", "a'b"],
                                          "delegate": str(fixture.real_rg)})
                self.assertEqual(fixture.home_snapshot(), before)
                self.assertFalse((fixture.root / "injection-marker").exists())

    def test_invalid_probe_refuses_before_build_or_destination_writes(self):
        for mode in ("invalid", "failed"):
            with self.subTest(mode=mode):
                fixture = self.fixture()
                fixture.settings["candidates"] = [str(fixture.candidate("bad-rg", mode))]
                before = fixture.home_snapshot()
                result = fixture.install()
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(fixture.events("compile"))
                self.assertFalse(fixture.bin.exists())
                self.assertEqual(fixture.home_snapshot(), before)

    def test_unowned_collision_and_legacy_uninstall_preserve_files(self):
        fixture = self.fixture()
        fixture.bin.mkdir(parents=True)
        (fixture.bin / "rg").write_text("unowned rg\n")
        (fixture.bin / "tgrep").write_text("unowned tgrep\n")
        before = fixture.home_snapshot()
        for operation in (fixture.install, fixture.uninstall):
            result = operation()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("preserved", result.stderr)
            self.assertEqual((fixture.bin / "rg").read_text(), "unowned rg\n")
            self.assertEqual((fixture.bin / "tgrep").read_text(), "unowned tgrep\n")
            self.assertEqual(fixture.home_snapshot(), before)
        self.assertFalse(fixture.events("compile"))

    def test_receipt_owned_reinstall_and_uninstall_preserve_home(self):
        fixture = self.fixture()
        before = fixture.home_snapshot()
        self.assert_success(fixture.install())
        receipt = fixture.receipt().read_bytes()
        self.assert_success(fixture.install())
        self.assertEqual(fixture.receipt().read_bytes(), receipt)
        self.assert_success(fixture.uninstall())
        self.assertFalse((fixture.bin / "rg").exists())
        self.assertFalse(fixture.receipt().exists())
        self.assertEqual(fixture.home_snapshot(), before)

    def test_destination_inode_aliases_are_not_probed(self):
        fixture = self.fixture()
        self.assert_success(fixture.install())
        fixture.wrapper(fixture.bin / "rg", "probe-valid")
        fixture.write_receipt()
        hardlink = fixture.root / "hardlink-rg"
        os.link(fixture.bin / "rg", hardlink)
        symlink = fixture.root / "symlink-rg"
        symlink.symlink_to(fixture.bin / "rg")
        fixture.settings["candidates"] = [str(path) for path in
                                           (fixture.bin / "rg", hardlink, symlink, fixture.real_rg)]
        self.assert_success(fixture.install())
        self.assertEqual(len(fixture.events("probe-valid")), 2)
        self.assertTrue(all(event["invoked"] == str(fixture.real_rg)
                            for event in fixture.events("probe-valid")))

    def test_changed_receipt_owned_files_are_not_replaced(self):
        fixture = self.fixture()
        self.assert_success(fixture.install())
        (fixture.bin / "rg").write_text("external edit\n")
        builds = len(fixture.events("compile"))
        result = fixture.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(fixture.events("compile")), builds)
        self.assertEqual((fixture.bin / "rg").read_text(), "external edit\n")

    def test_destination_arriving_during_build_is_preserved(self):
        fixture = self.fixture()
        fixture.save(arrive_during_build=True)
        result = fixture.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((fixture.bin / "rg").read_text(), "external file appeared during build\n")
        self.assertFalse((fixture.bin / "tgrep").exists())

    def test_publication_failures_report_stage_and_retain_hashes(self):
        phases = {1: "while installing tgrep", 2: "tgrep was installed", 3: "ownership receipt publication failed"}
        for phase, diagnostic in phases.items():
            with self.subTest(phase=phase):
                fixture = self.fixture()
                fixture.save(fail_install=phase)
                before = fixture.home_snapshot()
                result = fixture.install()
                self.assertEqual(result.returncode, 40 + phase)
                self.assertIn(diagnostic, result.stderr)
                self.assertNotIn("Installation complete.", result.stdout)
                prefix = "Staged ownership receipt retained: "
                retained = next(line[len(prefix):] for line in result.stderr.splitlines()
                                if line.startswith(prefix))
                hashes = Path(retained).read_text().splitlines()
                self.assertEqual(len(hashes), 2)
                self.assertTrue(all(len(value) == 64 for value in hashes))
                self.assertEqual(hashes[0], hashlib.sha256(
                    (fixture.repo / "target" / fixture.env["TGREP_FAKE_HOST_TRIPLE"] /
                     "release/tgrep").read_bytes()).hexdigest())
                self.assertEqual(fixture.home_snapshot(), before)

    def test_committed_binaries_with_configuration_failure_are_reported(self):
        fixture = self.fixture()
        (fixture.config / "rg_path").unlink()
        fixture.config.rmdir()
        fixture.save(fail_mkdir=str(fixture.config))
        result = fixture.install()
        self.assertEqual(result.returncode, 44)
        self.assertIn("binaries and ownership receipt are installed; configuration failed", result.stderr)
        self.assertNotIn("Installation complete.", result.stdout)
        self.assertTrue(fixture.receipt().is_file())
        self.assertEqual((fixture.state / "sentinel").read_text(), "existing index\n")

    def test_partial_uninstall_retains_receipt_and_reports_manual_review(self):
        fixture = self.fixture()
        self.assert_success(fixture.install())
        receipt = fixture.receipt().read_bytes()
        before = fixture.home_snapshot()
        fixture.save(fail_remove="rg")
        result = fixture.uninstall()
        self.assertEqual(result.returncode, 1)
        self.assertFalse((fixture.bin / "tgrep").exists())
        self.assertTrue((fixture.bin / "rg").exists())
        self.assertTrue(fixture.receipt().is_file(), "partial uninstall removed ownership receipt")
        self.assertEqual(fixture.receipt().read_bytes(), receipt)
        self.assertIn("uninstall partially completed", result.stderr)
        self.assertIn("manual review", result.stderr)
        self.assertEqual(fixture.home_snapshot(), before)

    def test_physical_checkout_preserves_trimmed_sibling(self):
        fixture = self.fixture(checkout="checkout\n")
        sibling = fixture.sibling()
        alias = fixture.root / "installer-alias"
        alias.symlink_to(fixture.script)
        fixture.script = alias
        self.assert_success(fixture.install())
        self.assertTrue((sibling / "target/sentinel").exists())
        self.assertFalse(fixture.events("wrong-compile"))

    def test_custom_cargo_output_is_installed_instead_of_stale_default(self):
        fixture = self.fixture()
        custom = fixture.root / "cargo output with spaces\n"
        fixture.env["TGREP_FAKE_TARGET_DIRECTORY"] = str(custom)
        stale = fixture.repo / "target/release/tgrep"
        stale.parent.mkdir(parents=True)
        stale.write_bytes(b"stale artifact must never be installed\n")
        before = fixture.home_snapshot()
        self.assert_success(fixture.install())
        actual = custom / fixture.env["TGREP_FAKE_HOST_TRIPLE"] / "release/tgrep"
        self.assertEqual((fixture.bin / "tgrep").read_bytes(), actual.read_bytes())
        self.assertNotEqual((fixture.bin / "tgrep").read_bytes(), stale.read_bytes())
        self.assertEqual(fixture.home_snapshot(), before)

    def test_cross_target_is_rejected_before_build_or_publication(self):
        fixture = self.fixture()
        fixture.env["CARGO_BUILD_TARGET"] = "aarch64-unknown-linux-gnu"
        before = fixture.home_snapshot()
        result = fixture.install()
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("CARGO_BUILD_TARGET", result.stderr)
        self.assertFalse(fixture.events("compile"))
        self.assertFalse(fixture.bin.exists())
        self.assertEqual(fixture.home_snapshot(), before)


class CompileTests(unittest.TestCase):
    def test_clean_targets_physical_newline_checkout(self):
        fixture = Fixture(self, "compile", checkout="checkout\n")
        sibling = fixture.sibling()
        (fixture.repo / "target").mkdir()
        (fixture.repo / "target/sentinel").write_text("own Cargo artifact\n")
        alias = fixture.root / "compile-alias"
        alias.symlink_to(fixture.script)
        fixture.script = alias
        result = fixture.run("--clean", "--simd", "scalar", "--jobs", "1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((sibling / "target/sentinel").exists())
        self.assertFalse((fixture.repo / "target/sentinel").exists())
        self.assertTrue(all(event["cwd"] == str(fixture.repo) for event in fixture.events("cargo")))

    def test_explicit_simd_environment_reaches_fake_cargo(self):
        fixture = Fixture(self, "compile")
        fixture.env["TGREP_SIMD_FLAGS"] = "-msse4.2"
        result = fixture.run("--simd", "scalar", "--jobs", "1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("-msse4.2", [event.get("simd_flags") for event in fixture.events("cargo")])

    def test_custom_target_directory_and_explicit_native_host(self):
        fixture = Fixture(self, "compile")
        custom = fixture.root / "cargo target with spaces\n"
        fixture.env["TGREP_FAKE_TARGET_DIRECTORY"] = str(custom)
        result = fixture.run("--release", "--simd", "scalar", "--jobs", "1")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        expected = custom / fixture.env["TGREP_FAKE_HOST_TRIPLE"] / "release/tgrep"
        self.assertTrue(expected.is_file())
        self.assertIn(str(expected), result.stdout)
        build = next(event for event in fixture.events("cargo") if event["args"][0] == "build")
        self.assertEqual(build["args"][build["args"].index("--target") + 1],
                         fixture.env["TGREP_FAKE_HOST_TRIPLE"])
        self.assertFalse((fixture.repo / "target/release/tgrep").exists())

    def test_cross_target_is_rejected_before_clean_or_build(self):
        fixture = Fixture(self, "compile")
        fixture.env["CARGO_BUILD_TARGET"] = "aarch64-unknown-linux-gnu"
        sentinel = fixture.repo / "target/sentinel"
        sentinel.parent.mkdir(parents=True)
        sentinel.write_text("preserve build artifacts\n")
        result = fixture.run("--clean", "--simd", "scalar")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("CARGO_BUILD_TARGET", result.stderr)
        self.assertFalse(fixture.events("cargo"))
        self.assertEqual(sentinel.read_text(), "preserve build artifacts\n")


class RunnerTests(unittest.TestCase):
    def test_path_discovery_skips_tgrep_wrapper(self):
        fixture = Fixture(self, "runner")
        fixture.env.pop("RG_BIN")
        fixture.env.pop("TGREP_REAL_RG", None)
        wrapper = fixture.wrapper(fixture.root / "wrapper-tools/rg", "rg")
        wrapper.write_text(wrapper.read_text().replace("#!/bin/sh\n", "#!/bin/sh\n# TGREP_REAL_RG wrapper\n", 1))
        fixture.env["PATH"] = f"{wrapper.parent}:{fixture.env['PATH']}"
        result = fixture.run("1", "relative-state", "relative-corpus")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(any(event["invoked"] == str(wrapper) for event in fixture.events("rg")))
        self.assertTrue(any(event["invoked"] == str(fixture.tools / "rg") for event in fixture.events("rg")))

    def test_invalid_baseline_or_missing_python_fails_before_results(self):
        for prerequisite in ("wrapper", "identity", "python"):
            with self.subTest(prerequisite=prerequisite):
                fixture = Fixture(self, "runner")
                if prerequisite == "wrapper":
                    wrapper = fixture.tools / "rg"
                    wrapper.write_text(wrapper.read_text().replace("#!/bin/sh\n", "#!/bin/sh\n# TGREP_REAL_RG wrapper\n", 1))
                elif prerequisite == "identity":
                    fixture.env["RG_BIN"] = str(fixture.repo / "target/release/tgrep")
                else:
                    fixture.env["PYTHON_BIN"] = str(fixture.root / "missing-python")
                result = fixture.run("1", "relative-state", "relative-corpus")
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertFalse((fixture.script.parent / "benchmark_results.csv").exists())

    def test_custom_tgrep_and_xdg_state_defaults(self):
        fixture = Fixture(self, "runner")
        custom = fixture.wrapper(fixture.root / "custom output/tgrep", "tgrep")
        fixture.env["TGREP_BIN"] = str(custom)
        fixture.env.pop("TGREP_STATE_DIR", None)
        result = fixture.run("1")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(str(fixture.state), result.stdout)
        self.assertTrue(all(event["invoked"] == str(custom) for event in fixture.events("tgrep")))

    def test_physical_paths_keep_caller_relative_corpus(self):
        fixture = Fixture(self, "runner", checkout="checkout\n", benchmarks="benchmarks\n")
        sibling = fixture.sibling()
        expected_csv = fixture.script.parent / "benchmark_results.csv"
        alias = fixture.root / "runner-alias"
        alias.symlink_to(fixture.script)
        fixture.script = alias
        result = fixture.run("1", "relative-state", "relative-corpus")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(expected_csv.exists())
        self.assertTrue((sibling / "target/sentinel").exists())
        self.assertFalse(fixture.events("wrong-tgrep"))
        events = fixture.events("tgrep") + fixture.events("rg")
        self.assertTrue(all(event["cwd"] == str(fixture.caller) for event in events))
        self.assertTrue(any("relative-corpus" in event["args"] for event in events))

    def test_search_errors_mismatches_and_empty_results(self):
        for mode in ("none", "error", "mismatch", "cache_error"):
            with self.subTest(mode=mode):
                fixture = Fixture(self, "runner")
                fixture.save(search_mode=mode)
                result = fixture.run("1", "relative-state", "relative-corpus")
                if mode == "none":
                    import csv
                    self.assertEqual(result.returncode, 0, result.stderr)
                    with (fixture.script.parent / "benchmark_results.csv").open() as source:
                        self.assertTrue(all(row["files_matched"] == "0" for row in csv.DictReader(source)))
                else:
                    self.assertNotEqual(result.returncode, 0)


class PackagingTests(unittest.TestCase):
    def test_package_metadata_and_symlink_checkout_preserve_newlines(self):
        fixture = Fixture(self, "packaging", checkout="checkout\n")
        sibling = fixture.sibling()
        alias = fixture.root / "package-alias"
        alias.symlink_to(fixture.script)
        fixture.script = alias
        result = fixture.run()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue((sibling / "target/sentinel").exists())
        self.assertTrue(all(event["cwd"] == str(fixture.repo) for event in fixture.events("cargo")))
        control = Path(fixture.env["TGREP_FAKE_PACKAGE_CONTROL"]).read_text()
        self.assertIn("Version: 1.2.3", control)
        self.assertTrue((fixture.root / "review-output/tgrep_1.2.3_amd64.deb").is_file())
        self.assertFalse((sibling / "dist").exists())

    def test_sword_license_metadata_must_match_the_license_file(self):
        fixture = Fixture(self, "packaging")
        fixture.env["TGREP_FAKE_LICENSE"] = "LicenseRef-SRSL-1.1"
        result = fixture.run()
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("disagrees with LICENSE", result.stderr)

        (fixture.repo / "LICENSE").write_text(
            "SWORD RESTRICTED SOURCE LICENCE (SRSL-1.1)\nfixture only\n")
        archive = fixture.repo / "LICENSES/LicenseRef-SRSL-1.0.txt"
        archive.parent.mkdir(parents=True)
        archive.write_text("historical SRSL-1.0 fixture\n")
        result = fixture.run()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        packaged_archive = fixture.root / "review-output/tgrep_1.2.3_amd64.deb"
        self.assertTrue(packaged_archive.is_file())
        install_events = fixture.events("install")
        self.assertTrue(any(event["args"][-2].endswith("LICENSES/LicenseRef-SRSL-1.0.txt")
                            and event["args"][-1].endswith("/LICENSES/LicenseRef-SRSL-1.0.txt")
                            for event in install_events))

    def test_baseline_symlink_paths_write_only_the_owned_checkout(self):
        scripts = ("bench_grep_vs_rg.sh", "run_baselines.sh", "run_strong_baselines.sh")
        for script in scripts:
            with self.subTest(script=script):
                fixture = Fixture(self, "packaging", checkout="checkout\n", script="scripts/" + script)
                sibling = fixture.sibling()
                alias = fixture.root / "baseline-alias"
                alias.symlink_to(fixture.script)
                fixture.script = alias
                result = fixture.run()
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                if script == "bench_grep_vs_rg.sh":
                    self.assertIn(str(fixture.repo / "ARCHITECTURE.md"), result.stdout)
                    output_line = next(line for line in result.stdout.splitlines()
                                       if line.startswith("Output saved in: "))
                    self.assertEqual(Path(output_line.removeprefix("Output saved in: ")).parent,
                                     fixture.root / "tmp")
                else:
                    self.assertEqual(len(list(fixture.repo.glob("*.csv"))), 1)
                self.assertFalse(list(sibling.glob("*.csv")))
                self.assertTrue((sibling / "target/sentinel").exists())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--scratch", type=Path, required=True)
    parser.add_argument("group", choices=("installer", "compile", "runner", "packaging"))
    OPTIONS = parser.parse_args()
    OPTIONS.root = OPTIONS.root.resolve()
    OPTIONS.scratch = OPTIONS.scratch.resolve()
    groups = {"installer": InstallerTests, "compile": CompileTests, "runner": RunnerTests,
              "packaging": PackagingTests}
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(groups[OPTIONS.group])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    sys.exit(0 if result.wasSuccessful() else 1)
