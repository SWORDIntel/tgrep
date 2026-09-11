#!/bin/bash
# Benchmark: grep vs rg using history-derived patterns from ARCHITECTURE.md
# Corpus: KEYSTONE + QIHSE + Native-AI-Terminal source trees
# Design doc: ARCHITECTURE.md (lines 278-298)

set -u

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

CORPUS="${TGREP_CORPUS:-$HOME/Documents/KEYSTONE $HOME/Documents/QIHSE $HOME/Documents/Native-AI-Terminal}"
TRIALS=5
OUTDIR="/tmp/tgrep_bench_$$"
mkdir -p "$OUTDIR"

echo "=== tgrep benchmark: grep vs rg ==="
echo "Design: $REPO_DIR/ARCHITECTURE.md"
echo "Corpus: $CORPUS"
echo "Trials: $TRIALS per pattern"
echo ""

# Source bytes in corpus
SRC_BYTES=$(find $CORPUS -type f -readable 2>/dev/null | xargs cat 2>/dev/null | wc -c)
FILE_COUNT=$(find $CORPUS -type f -readable 2>/dev/null | wc -l)
echo "Source files: $FILE_COUNT"
echo "Source bytes: $SRC_BYTES"
echo ""

# Patterns from ARCHITECTURE.md lines 282-291
# Using parallel arrays to avoid delimiter issues
ids=(01 02 03 04 05 06 07 08 09 10)
gpats=("^error" "error\[" "error:" "unnecessary parentheses" "test result: ok" "FAILED\|test result: FAILED" "cargo test" "nat-context" "cpu" "CFG")
rpats=("^error" "error\[" "error:" "unnecessary parentheses" "test result: ok" "FAILED|test result: FAILED" "cargo test" "nat-context" "cpu" "CFG")
gopts=("" "" "-B2" "-A5" "-c" "" "" "" "" "")
ropts=("-s" "-s" "-s -B2" "-s -A5" "-s -c" "-s" "-s" "-s" "-s" "-s")
descs=("Anchored compiler errors" "Escaped punctuation" "Context lines (-B2)" "Warning + following (-A5)" "Match count (-c)" "Alternation (test failures)" "Literal phrase" "Hyphenated identifier" "Common three-byte term" "Uppercase identifier")

NUM=${#ids[@]}

printf "%-4s %-35s %14s %14s %8s %8s %9s\n" "ID" "Pattern" "grep_med(ms)" "rg_med(ms)" "grep_n" "rg_n" "speedup"
printf "%-4s %-35s %14s %14s %8s %8s %9s\n" "--" "-------" "------------" "-----------" "------" "-----" "-------"

total_grep=0
total_rg=0

for i in $(seq 0 $((NUM - 1))); do
  id="${ids[$i]}"
  gpat="${gpats[$i]}"
  rpat="${rpats[$i]}"
  gopt="${gopts[$i]}"
  ropt="${ropts[$i]}"
  desc="${descs[$i]}"

  grep_times=()
  rg_times=()

  for t in $(seq 1 $TRIALS); do
    # grep: BRE by default, \| is BRE alternation, case-sensitive
    start=$(date +%s%N)
    grep -r --binary-files=without-match $gopt -e "$gpat" $CORPUS > "$OUTDIR/${id}_grep_${t}.txt" 2>/dev/null
    end=$(date +%s%N)
    elapsed=$(( (end - start) / 1000000 ))
    grep_times+=($elapsed)

    # rg: translate \| to |, use -s for case sensitivity
    start=$(date +%s%N)
    rg -s --no-heading $ropt -e "$rpat" $CORPUS > "$OUTDIR/${id}_rg_${t}.txt" 2>/dev/null
    end=$(date +%s%N)
    elapsed=$(( (end - start) / 1000000 ))
    rg_times+=($elapsed)
  done

  # Count matches from first trial
  grep_matches=$(wc -l < "$OUTDIR/${id}_grep_1.txt" 2>/dev/null || echo 0)
  rg_matches=$(wc -l < "$OUTDIR/${id}_rg_1.txt" 2>/dev/null || echo 0)

  # Median
  g_sorted=($(printf '%s\n' "${grep_times[@]}" | sort -n))
  r_sorted=($(printf '%s\n' "${rg_times[@]}" | sort -n))
  mid=$(( (TRIALS - 1) / 2 ))
  g_med=${g_sorted[$mid]}
  r_med=${r_sorted[$mid]}

  # Speedup
  if [ "$r_med" -gt 0 ]; then
    speedup=$(awk "BEGIN { printf \"%.2fx\", $g_med / $r_med }")
  else
    speedup="N/A"
  fi

  total_grep=$((total_grep + g_med))
  total_rg=$((total_rg + r_med))

  # Save per-trial times
  echo "grep: ${grep_times[@]}" > "$OUTDIR/${id}_times.txt"
  echo "rg:   ${rg_times[@]}" >> "$OUTDIR/${id}_times.txt"

  printf "%-4s %-35s %14s %14s %8s %8s %9s\n" "$id" "$desc" "$g_med" "$r_med" "$grep_matches" "$rg_matches" "$speedup"
done

echo ""
echo "=== Per-trial timings (ms) ==="
echo ""
for i in $(seq 0 $((NUM - 1))); do
  id="${ids[$i]}"
  desc="${descs[$i]}"
  echo "--- $id: $desc ---"
  cat "$OUTDIR/${id}_times.txt" 2>/dev/null | sed 's/^/  /'
done

echo ""
echo "=== Result equality check (line counts, trial 1) ==="
for i in $(seq 0 $((NUM - 1))); do
  id="${ids[$i]}"
  desc="${descs[$i]}"
  g=$(wc -l < "$OUTDIR/${id}_grep_1.txt" 2>/dev/null || echo 0)
  r=$(wc -l < "$OUTDIR/${id}_rg_1.txt" 2>/dev/null || echo 0)
  if [ "$g" -eq "$r" ]; then
    status="MATCH ($g lines)"
  else
    status="DIFF (grep=$g rg=$r)"
  fi
  printf "  %-4s %-35s %s\n" "$id" "$desc" "$status"
done

echo ""
echo "=== Summary ==="
echo "Total median grep: ${total_grep}ms"
echo "Total median rg:   ${total_rg}ms"
if [ "$total_rg" -gt 0 ]; then
  echo "Overall speedup: $(awk "BEGIN { printf \"%.2fx\", $total_grep / $total_rg }")"
fi

echo ""
echo "Output saved in: $OUTDIR"
echo ""
echo "Tool versions:"
grep --version | head -1
rg --version | head -1
