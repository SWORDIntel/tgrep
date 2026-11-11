#!/usr/bin/env bash
# Comprehensive tgrep vs ripgrep benchmark runner.
#
# Tests multiple pattern categories (rare, broad, word, case-insensitive,
# absent) with cold and warm cache runs. Outputs CSV data for graphing.
#
# Usage:
#   ./benchmarks/run_benchmarks.sh [trials] [state_dir] [corpus]
#
# Defaults:
#   trials=10
#   state_dir=TGREP_STATE_DIR, XDG_STATE_HOME/tgrep, or ~/.local/state/tgrep
#   corpus=/rpool/scratch/tgrep_corpus
# Set TGREP_BIN for a custom Cargo target directory or compile.sh output.

set -euo pipefail

TRIALS="${1:-10}"
STATE_DIR="${2:-${TGREP_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/tgrep}}"
CORPUS="${3:-/rpool/scratch/tgrep_corpus}"
if [[ ! "$TRIALS" =~ ^[1-9][0-9]*$ ]]; then
    echo "Error: trials must be a positive integer" >&2
    exit 2
fi
if ! IFS= read -r -d '' SCRIPT_PATH < <(readlink -fz -- "${BASH_SOURCE[0]}"); then
    echo 'Error: could not resolve benchmark script path.' >&2
    exit 2
fi
OUT_DIR="${SCRIPT_PATH%/*}"
[[ -n "$OUT_DIR" ]] || OUT_DIR=/
REPO_DIR="${OUT_DIR%/*}"
[[ -n "$REPO_DIR" ]] || REPO_DIR=/
TGREP_BIN="${TGREP_BIN:-$REPO_DIR/target/release/tgrep}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
    echo "Error: Python 3 is required for precise subprocess timing." >&2
    exit 2
fi

if [ ! -x "$TGREP_BIN" ]; then
    echo "Error: $TGREP_BIN not found. Build tgrep first or set TGREP_BIN." >&2
    exit 1
fi

# Exclude the installed tgrep wrapper from the independent ripgrep baseline.
if ! IFS= read -r -d '' RG_BIN < <("$PYTHON_BIN" - "$TGREP_BIN" "${RG_BIN:-${TGREP_REAL_RG:-}}" <<'PY'
import os
import subprocess
import sys

tgrep, explicit = sys.argv[1:]
name = explicit or "rg"
candidates = [name] if os.path.dirname(name) else [os.path.join(p, name) for p in os.get_exec_path()]
for candidate in candidates:
    try:
        if os.path.samefile(candidate, tgrep):
            continue
        with open(candidate, "rb") as source:
            prefix = source.read(4096)
        if prefix.startswith(b"#!") and b"TGREP_REAL_RG" in prefix:
            continue
        version = subprocess.run([candidate, "--version"], capture_output=True, timeout=5)
        first = version.stdout.splitlines()[0].split() if version.stdout else []
        if version.returncode == 0 and len(first) > 1 and first[0] == b"ripgrep" and first[1][:1].isdigit():
            sys.stdout.write(os.path.realpath(candidate) + "\0")
            break
    except (OSError, subprocess.TimeoutExpired):
        continue
else:
    sys.stderr.write("Error: no independent ripgrep found; set RG_BIN to the real executable.\n")
    sys.exit(2)
PY
); then
    exit 2
fi

CSV="$OUT_DIR/benchmark_results.csv"

# Patterns: name|category|pattern|flags
# flags: empty for default, "-w" for word, "-i" for ignore-case
PATTERNS=(
    # Rare patterns (mixed case, indexed)
    "RareNeedle|rare|RareNeedle|"
    "KeystoneTrigram|rare|KeystoneTrigram|"
    "NonexistentXyz123|rare|NonexistentXyz123|"
    "QihseOptimization|rare|QihseOptimization|"
    "DsmilHashIndex|rare|DsmilHashIndex|"
    "TgrepSegmentWriter|rare|TgrepSegmentWriter|"
    "AtomicWriteFsync|rare|AtomicWriteFsync|"
    "WalCheckpointReplay|rare|WalCheckpointReplay|"
    # Broad patterns (mixed case, indexed)
    "Struct|broad|Struct|"
    "FnMain|broad|Fn Main|"
    "Return|broad|Return|"
    "UseStd|broad|Use Std|"
    # Word search patterns (-w flag, hash index)
    "WordStruct|word|Struct|-w"
    "WordMain|word|Main|-w"
    "WordReturn|word|Return|-w"
    "WordTerminal|word|Terminal|-w"
    "WordNonexistent|word|NonexistentXyz|-w"
    # Case-insensitive patterns (streaming fallback)
    "CITerminal|caseinsensitive|terminal|-i"
    "CIStruct|caseinsensitive|struct|-i"
)

echo "pattern,category,trial,condition,tgrep_ms,rg_ms,files_matched" > "$CSV"

run_trial() {
    local name="$1" cat="$2" pat="$3" flags="$4" trial="$5" cond="$6"

    # Cold denotes the application result cache, not filesystem or ZFS ARC cache.
    if [ "$cond" = "cold" ]; then
        TGREP_STATE_DIR="$STATE_DIR" "$TGREP_BIN" cache clear
    fi

    "$PYTHON_BIN" - "$name" "$cat" "$pat" "$flags" "$trial" "$cond" \
        "$TGREP_BIN" "$RG_BIN" "$STATE_DIR" "$CORPUS" "$CSV" <<'PY'
import csv
import os
import shlex
import subprocess
import sys
import time

name, category, pattern, flags, trial, condition, tgrep, rg, state, corpus, csv_path = sys.argv[1:]
arguments = shlex.split(flags)
commands = {
    "tgrep": [tgrep, *arguments, "-l", "--", pattern, corpus],
    "rg": [rg, *arguments, "-l", "-j", "4", "--", pattern, corpus],
}
results = {}
# Alternate order; the cache condition describes tgrep's application cache only.
for tool in (("tgrep", "rg") if int(trial) % 2 else ("rg", "tgrep")):
    environment = dict(os.environ)
    if tool == "tgrep":
        environment["TGREP_STATE_DIR"] = state
    try:
        started = time.perf_counter_ns()
        output = subprocess.run(commands[tool], stdout=subprocess.PIPE, env=environment)
        elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    except OSError as error:
        sys.exit(f"Error: {tool} could not run for {name}: {error}")
    if output.returncode not in (0, 1):
        sys.exit(f"Error: {tool} failed with status {output.returncode} for {name}")
    results[tool] = (output.returncode, sorted(output.stdout.splitlines()), elapsed_ms)
t_status, t_files, t_ms = results["tgrep"]
r_status, r_files, r_ms = results["rg"]
if t_status != r_status or t_files != r_files:
    sys.exit(f"Error: result mismatch for {name}")
with open(csv_path, "a", newline="") as target:
    csv.writer(target).writerow((name, category, trial, condition, f"{t_ms:.3f}", f"{r_ms:.3f}", len(t_files)))
print(f"  {name} [{category}] trial {trial} {condition}: tgrep={t_ms:.3f}ms rg={r_ms:.3f}ms files={len(t_files)}")
PY
}

echo "=== tgrep Comprehensive Benchmark ==="
echo "Trials: $TRIALS"
echo "State:  $STATE_DIR"
echo "Corpus: $CORPUS"
echo "ripgrep: $RG_BIN ($("$RG_BIN" --version | head -1))"
echo "Output: $CSV"
echo ""

for entry in "${PATTERNS[@]}"; do
    IFS='|' read -r name cat pat flags <<< "$entry"
    echo "--- $name ($cat) ---"

    # Cold run (first trial, no cache)
    run_trial "$name" "$cat" "$pat" "$flags" 1 "cold"

    # Warm runs (cache populated after first cold run)
    for t in $(seq 2 "$TRIALS"); do
        run_trial "$name" "$cat" "$pat" "$flags" "$t" "warm"
    done
    echo ""
done

echo "=== Benchmark complete ==="
echo "Results: $CSV"
