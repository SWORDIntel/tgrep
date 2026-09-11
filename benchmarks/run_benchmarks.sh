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
TGREP_BIN="$(cd "$(dirname "$0")/.." && pwd)/target/release/tgrep"

OUT_DIR="$(cd "$(dirname "$0")" && pwd)"
CSV="$OUT_DIR/benchmark_results.csv"
TMPF="/tmp/tgrep_bench_$$.txt"

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

    # Clear cache for cold runs
    if [ "$cond" = "cold" ]; then
        TGREP_STATE_DIR="$STATE_DIR" "$TGREP_BIN" cache clear 2>/dev/null || true
    fi

    # Run tgrep (exit 1 = no match, which is valid)
    local t_out t_elapsed
    t_out=$(TGREP_STATE_DIR="$STATE_DIR" /usr/bin/time -f "%e" \
        "$TGREP_BIN" $flags -l "$pat" "$CORPUS" 2>"$TMPF" || true)
    t_elapsed=$(cat "$TMPF")
    tgrep_ms=$(awk '{printf "%.0f", $1 * 1000}' <<< "$t_elapsed")
    files=$(echo "$t_out" | wc -l)

    # Run rg (always fresh, no cache)
    local r_out r_elapsed
    r_out=$(/usr/bin/time -f "%e" rg $flags -l "$pat" "$CORPUS" -j 4 2>"$TMPF" || true)
    r_elapsed=$(cat "$TMPF")
    rg_ms=$(awk '{printf "%.0f", $1 * 1000}' <<< "$r_elapsed")

    echo "$name,$cat,$trial,$cond,$tgrep_ms,$rg_ms,$files" >> "$CSV"
    printf "  %s [%s] trial %d %s: tgrep=%sms rg=%sms files=%s\n" \
        "$name" "$cat" "$trial" "$cond" "$tgrep_ms" "$rg_ms" "$files"
}

echo "=== tgrep Comprehensive Benchmark ==="
echo "Trials: $TRIALS"
echo "State:  $STATE_DIR"
echo "Corpus: $CORPUS"
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
