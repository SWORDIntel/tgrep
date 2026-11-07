#!/bin/bash
# Strong corpus baselines: grep + rg on NVMe (50 runs) and ZFS (20 runs)
# Corpus: $TGREP_NVME_CORPUS (NVMe) / $TGREP_ZFS_CORPUS (ZFS)
# 1.75 GB, 26,731 files, real source + synthetic fixtures with known patterns

set -euo pipefail

if ! IFS= read -r -d '' SCRIPT_PATH < <(readlink -fz -- "${BASH_SOURCE[0]}"); then
  echo 'Error: could not resolve baseline script path.' >&2
  exit 2
fi
SCRIPT_DIR="${SCRIPT_PATH%/*}"
REPO_DIR="${SCRIPT_DIR%/*}"
[[ -n "$REPO_DIR" ]] || REPO_DIR=/

NVME="${TGREP_NVME_CORPUS:-/rpool/scratch/tgrep_corpus}"
ZFS="${TGREP_ZFS_CORPUS:-/fast/scratch/tgrep_corpus}"
RESULTS=$(mktemp "$REPO_DIR/strong_baselines_$(date +%Y%m%d_%H%M%S).XXXXXX.csv")

echo "pool,tool,run,pattern,time_ms,matches" > "$RESULTS"

# Mix of known patterns (for correctness) and random patterns (for breadth)
patterns=(
  "rareneedle_tgrep_test_42"
  "ZEBRA_ALPHANUMERIC_X9"
  "café_naïve_résumé"
  "trigram_bait"
  "rarenee dle_tgrep_test_41"
  "struct"
  "int main"
  "NULL"
  "keystone"
  "qihse"
  "error"
  "TODO"
  "pthread"
  "uint64_t"
  "malloc"
  "search"
  "hash"
  "index"
  "config"
  "mutex"
  "encrypt"
  "anchor"
  "posting"
  "segment"
  "manifest"
  "interpolation"
  "typedef"
  "memcpy"
  "atomic"
  "spinlock"
  "test result: ok"
  "cargo test"
  "error\["
  "^error"
  "unnecessary parentheses"
  "FAILED"
  "select"
  "insert"
  "delete"
  "update"
  "void"
  "const"
  "static"
  "inline"
  "extern"
  "enum"
  "union"
  "bool"
  "sizeof"
  "offsetof"
)

NUM_PATS=${#patterns[@]}
RANDOM="${TGREP_BENCH_SEED:-1}"

run_pool() {
  local pool_name="$1"
  local corpus="$2"
  local count="$3"
  local tool="$4"
  local -n query_sequence="$5"

  for i in $(seq 1 $count); do
    pat="${query_sequence[$((i - 1))]}"
    start=$(date +%s%N)
    if [ "$tool" == "grep" ]; then
      if m=$(grep -r --binary-files=without-match -c -- "$pat" "$corpus" | awk -F: '{s+=$NF} END {print s+0}'); then :
      else status=$?; [[ "$status" -eq 1 ]] || exit "$status"; fi
    else
      if m=$(rg --case-sensitive --threads 4 --no-heading -c -- "$pat" "$corpus" | awk -F: '{s+=$NF} END {print s+0}'); then :
      else status=$?; [[ "$status" -eq 1 ]] || exit "$status"; fi
    fi
    end=$(date +%s%N)
    ms=$(( (end - start) / 1000000 ))
    echo "$pool_name,$tool,$i,$pat,$ms,$m" >> "$RESULTS"
    printf "\r  %s %s %d/%d: '%s' -> %dms, %s matches   " "$pool_name" "$tool" "$i" "$count" "$pat" "$ms" "$m"
  done
  echo ""
}

nvme_patterns=()
zfs_patterns=()
for i in $(seq 1 50); do nvme_patterns+=("${patterns[$((RANDOM % NUM_PATS))]}"); done
for i in $(seq 1 20); do zfs_patterns+=("${patterns[$((RANDOM % NUM_PATS))]}"); done

echo "=== NVMe grep (50 runs) ==="
run_pool "nvme" "$NVME" 50 "grep" nvme_patterns

echo ""
echo "=== NVMe rg (50 runs) ==="
run_pool "nvme" "$NVME" 50 "rg" nvme_patterns

echo ""
echo "=== ZFS grep (20 runs) ==="
run_pool "zfs" "$ZFS" 20 "grep" zfs_patterns

echo ""
echo "=== ZFS rg (20 runs) ==="
run_pool "zfs" "$ZFS" 20 "rg" zfs_patterns

echo ""
echo "=== Complete. Results: $RESULTS ==="
echo ""
echo "--- Summary ---"
for pool in nvme zfs; do
  for tool in grep rg; do
    awk -F, -v p="$pool" -v t="$tool" '$1==p && $2==t {sum+=$5; n++; if($5>max)max=$5; if(min==0||$5<min)min=$5} END{printf "  %s/%s: runs=%d  mean=%.0fms  min=%dms  max=%dms\n",p,t,n,sum/n,min,max}' "$RESULTS"
  done
done
echo ""
echo "All raw data:"
cat "$RESULTS"
