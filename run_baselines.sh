#!/bin/bash
# Raw grep baselines: 50 on NVMe, 20 on ZFS. Random patterns. Record for later research.
set -u

NVME="/home/john/Documents/KEYSTONE /home/john/Documents/QIHSE /home/john/Documents/Native-AI-Terminal"
ZFS="/fast/scratch/tgrep_corpus/KEYSTONE /fast/scratch/tgrep_corpus/QIHSE /fast/scratch/tgrep_corpus/Native-AI-Terminal"
RESULTS="/home/john/tgrep/baseline_results_$(date +%Y%m%d_%H%M%S).csv"

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

echo "=== NVMe baselines (50 runs, rpool) ==="
for i in $(seq 1 50); do
  pat="${patterns[$((RANDOM % NUM_PATS))]}"
  start=$(date +%s%N)
  m=$(grep -r --binary-files=without-match -c "$pat" $NVME 2>/dev/null | awk -F: '{s+=$NF} END {print s+0}')
  end=$(date +%s%N)
  ms=$(( (end - start) / 1000000 ))
  echo "nvme,$i,$pat,$ms,$m" >> "$RESULTS"
  printf "\r  NVMe %d/50: '%s' -> %dms, %s matches   " "$i" "$pat" "$ms" "$m"
done
echo ""
echo "  NVMe done."

echo ""
echo "=== ZFS baselines (20 runs, fast pool) ==="
# Drop caches for fairer ZFS reads
sync && echo 3 > /proc/sys/vm/drop_caches 2>/dev/null || true

for i in $(seq 1 20); do
  pat="${patterns[$((RANDOM % NUM_PATS))]}"
  start=$(date +%s%N)
  m=$(grep -r --binary-files=without-match -c "$pat" $ZFS 2>/dev/null | awk -F: '{s+=$NF} END {print s+0}')
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
