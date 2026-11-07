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
#   state_dir=/tmp/tgrep_qwi_mmap
#   corpus=/rpool/scratch/tgrep_corpus

set -euo pipefail

TRIALS="${1:-10}"
STATE_DIR="${2:-/tmp/tgrep_qwi_mmap}"
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
TGREP_BIN="$REPO_DIR/target/release/tgrep"
RG_BIN="${RG_BIN:-/usr/bin/rg}"

CSV="$OUT_DIR/benchmark_results.csv"
TMPF=$(mktemp)
trap 'rm -f "$TMPF"' EXIT

# Verify tgrep is built
if [ ! -x "$TGREP_BIN" ]; then
    echo "Error: $TGREP_BIN not found. Run 'cargo build --release' first." >&2
    exit 1
fi

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
    local tgrep_ms rg_ms files

    # Cold denotes the application result cache, not filesystem or ZFS ARC cache.
    if [ "$cond" = "cold" ]; then
        TGREP_STATE_DIR="$STATE_DIR" "$TGREP_BIN" cache clear
    fi

    local t_out r_out t_status r_status
    if t_out=$(TGREP_STATE_DIR="$STATE_DIR" /usr/bin/time -q -o "$TMPF" -f "%e" \
        "$TGREP_BIN" $flags -l "$pat" "$CORPUS"); then
        t_status=0
    else
        t_status=$?
    fi
    if [ "$t_status" -gt 1 ]; then
        echo "Error: tgrep failed with status $t_status for $name" >&2
        return 2
    fi
    tgrep_ms=$(awk '{printf "%.0f", $1 * 1000}' "$TMPF")

    if r_out=$(/usr/bin/time -q -o "$TMPF" -f "%e" \
        "$RG_BIN" $flags -l "$pat" "$CORPUS" -j 4); then
        r_status=0
    else
        r_status=$?
    fi
    if [ "$r_status" -gt 1 ]; then
        echo "Error: rg failed with status $r_status for $name" >&2
        return 2
    fi
    rg_ms=$(awk '{printf "%.0f", $1 * 1000}' "$TMPF")
    local t_sorted r_sorted
    if ! t_sorted=$(printf '%s' "$t_out" | LC_ALL=C sort) ||
        ! r_sorted=$(printf '%s' "$r_out" | LC_ALL=C sort); then
        echo "Error: could not compare results for $name" >&2
        return 2
    fi
    if [ "$t_status" -ne "$r_status" ] || [ "$t_sorted" != "$r_sorted" ]; then
        echo "Error: result mismatch for $name" >&2
        return 2
    fi
    files=0
    if [ -n "$t_out" ]; then
        files=$(printf '%s\n' "$t_out" | wc -l)
    fi

    echo "$name,$cat,$trial,$cond,$tgrep_ms,$rg_ms,$files" >> "$CSV"
    printf "  %s [%s] trial %d %s: tgrep=%sms rg=%sms files=%s\n" \
        "$name" "$cat" "$trial" "$cond" "$tgrep_ms" "$rg_ms" "$files"
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
