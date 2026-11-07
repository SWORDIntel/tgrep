#!/bin/bash
# Raw grep baselines: 50 on NVMe, 20 on ZFS. Random patterns. Record for later research.
set -euo pipefail

if ! IFS= read -r -d '' SCRIPT_PATH < <(readlink -fz -- "${BASH_SOURCE[0]}"); then
  echo 'Error: could not resolve baseline script path.' >&2
  exit 2
fi
SCRIPT_DIR="${SCRIPT_PATH%/*}"
REPO_DIR="${SCRIPT_DIR%/*}"
[[ -n "$REPO_DIR" ]] || REPO_DIR=/

if [[ -n "${TGREP_NVME_CORPUS:-}" ]]; then
  NVME=("$TGREP_NVME_CORPUS")
else
  NVME=("$HOME/Documents/KEYSTONE" "$HOME/Documents/QIHSE" "$HOME/Documents/Native-AI-Terminal")
fi
if [[ -n "${TGREP_ZFS_CORPUS:-}" ]]; then
  ZFS=("$TGREP_ZFS_CORPUS")
else
  ZFS=(/fast/scratch/tgrep_corpus/KEYSTONE /fast/scratch/tgrep_corpus/QIHSE /fast/scratch/tgrep_corpus/Native-AI-Terminal)
fi
RESULTS=$(mktemp "$REPO_DIR/baseline_results_$(date +%Y%m%d_%H%M%S).XXXXXX.csv")

echo "pool,run,pattern,time_ms,matches" > "$RESULTS"

patterns=(
  "struct" "int main" "NULL" "errno" "malloc" "pthread" "uint64_t" "static"
  "return 0" "typedef" "include" "define" "size_t" "memcpy" "free(" "const"
  "void" "assert" "printf" "strcmp" "qihse" "keystone" "error" "warning"
  "TODO" "FIXME" "config" "mutex" "lock" "atomic"
  "inline" "extern" "enum" "union" "bool" "sizeof" "offsetof"
  "spinlock" "wait" "signal" "barrier" "nonce" "encrypt" "decrypt"
  "hash" "index" "search" "query" "insert" "delete" "update" "select"
)

NUM_PATS=${#patterns[@]}
RANDOM="${TGREP_BENCH_SEED:-1}"

echo "=== NVMe baselines (50 runs, rpool) ==="
for i in $(seq 1 50); do
  pat="${patterns[$((RANDOM % NUM_PATS))]}"
  start=$(date +%s%N)
  if m=$(grep -r --binary-files=without-match -c -- "$pat" "${NVME[@]}" | awk -F: '{s+=$NF} END {print s+0}'); then :
  else status=$?; [[ "$status" -eq 1 ]] || exit "$status"; fi
  end=$(date +%s%N)
  ms=$(( (end - start) / 1000000 ))
  echo "nvme,$i,$pat,$ms,$m" >> "$RESULTS"
  printf "\r  NVMe %d/50: '%s' -> %dms, %s matches   " "$i" "$pat" "$ms" "$m"
done
echo ""
echo "  NVMe done."

echo ""
echo "=== ZFS baselines (20 runs, fast pool) ==="
# Filesystem and ZFS ARC cache state are uncontrolled; do not change global kernel cache state.

for i in $(seq 1 20); do
  pat="${patterns[$((RANDOM % NUM_PATS))]}"
  start=$(date +%s%N)
  if m=$(grep -r --binary-files=without-match -c -- "$pat" "${ZFS[@]}" | awk -F: '{s+=$NF} END {print s+0}'); then :
  else status=$?; [[ "$status" -eq 1 ]] || exit "$status"; fi
  end=$(date +%s%N)
  ms=$(( (end - start) / 1000000 ))
  echo "zfs,$i,$pat,$ms,$m" >> "$RESULTS"
  printf "\r  ZFS %d/20: '%s' -> %dms, %s matches   " "$i" "$pat" "$ms" "$m"
done
echo ""
echo "  ZFS done."

echo ""
echo "=== Complete. Results: $RESULTS ==="
echo ""
echo "--- Summary ---"
echo "NVMe:"
awk -F, '$1=="nvme"{sum+=$4; n++; if($4>max)max=$4; if(min==0||$4<min)min=$4} END{printf "  runs=%d  mean=%.0fms  min=%dms  max=%dms\n",n,sum/n,min,max}' "$RESULTS"
echo "ZFS:"
awk -F, '$1=="zfs"{sum+=$4; n++; if($4>max)max=$4; if(min==0||$4<min)min=$4} END{printf "  runs=%d  mean=%.0fms  min=%dms  max=%dms\n",n,sum/n,min,max}' "$RESULTS"
echo ""
echo "All raw data:"
cat "$RESULTS"
