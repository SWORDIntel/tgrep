#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-SRSL-1.1
"""Structured events for the few fixture boundaries that need JSON."""

import json
import os
from pathlib import Path
import sys


settings = json.loads(Path(os.environ["TGREP_SHELL_FIXTURE"]).read_text())
tool, invoked, *arguments = sys.argv[1:]
event = {"tool": tool, "args": arguments, "cwd": os.getcwd()}
if tool == "cargo":
    event["simd_flags"] = os.environ.get("TGREP_SIMD_FLAGS")
    if arguments[0] == "metadata":
        print(json.dumps({"packages": [{"name": "tgrep", "version": "1.2.3",
                                       "license": os.environ.get("TGREP_FAKE_LICENSE",
                                                                 "AGPL-3.0-only")}],
                          "target_directory": str(Path.cwd() / "target")}))
    elif "--release" in arguments:
        target = Path.cwd() / "target/release/tgrep"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("synthetic release artifact, never executed\n")
    elif arguments[0] == "clean":
        (Path.cwd() / "target/sentinel").unlink(missing_ok=True)
    else:
        target = Path.cwd() / "target/debug/tgrep"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("synthetic debug artifact, never executed\n")
elif tool == "tgrep":
    Path(settings["output"]).write_text(json.dumps({
        "args": arguments, "delegate": os.environ.get("TGREP_REAL_RG")
    }))
else:
    raise RuntimeError(f"unexpected structured fixture tool: {tool}")
with Path(settings["log"]).open("a") as output:
    output.write(json.dumps(event) + "\n")
