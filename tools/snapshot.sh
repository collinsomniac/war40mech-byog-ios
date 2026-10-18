#!/usr/bin/env bash
# Desktop check under a watchdog: run the app at 800x600 (vsync) until it saves a snapshot, killing it if
# GPU memory or process memory pass their limits, so a bad build can't take the machine down.
#
#   tools/snapshot.sh <out.png> [seconds] [VAR=value ...]
#   e.g. tools/snapshot.sh shot.png 40 COGITATOR_START=Exploration COGITATOR_AUTOPLAY=1
#
# Limits: VRAM_LIMIT_MB (default 5000), RAM_LIMIT_MB (default 6000). Logs go to <out>.log.
set -u
out="$1"; secs="${2:-25}"; shift 2 || shift $#
here="$(cd "$(dirname "$0")/.." && pwd)"
vram_limit="${VRAM_LIMIT_MB:-5000}"; ram_limit="${RAM_LIMIT_MB:-6000}"
rm -f "$out"
export COGITATOR_PACK="${COGITATOR_PACK:-$here/content/pack}"
export COGITATOR_SNAPSHOT="$(cygpath -w "$out" 2>/dev/null || echo "$out")" COGITATOR_SNAPSHOT_SECS="$secs"
for kv in "$@"; do export "$kv"; done
(cd "$here/app" && exec target/release/cogitator.exe > "$out.log" 2>&1) &
deadline=$(( $(date +%s) + secs + 90 ))
peak_v=0; peak_r=0
while sleep 2; do
  pid=$(tasklist //FI "IMAGENAME eq cogitator.exe" //FO CSV //NH 2>/dev/null | grep -i cogitator | cut -d, -f2 | tr -d '"')
  [ -z "$pid" ] && break
  v=$(nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits 2>/dev/null | head -1)
  r=$(tasklist //FI "PID eq $pid" //FO CSV //NH | awk -F'","' '{print $5}' | tr -dc '0-9'); r=$(( ${r:-0} / 1024 ))
  [ "${v:-0}" -gt "$peak_v" ] && peak_v=$v; [ "$r" -gt "$peak_r" ] && peak_r=$r
  if [ "${v:-0}" -gt "$vram_limit" ] || [ "$r" -gt "$ram_limit" ] || [ "$(date +%s)" -gt "$deadline" ]; then
    taskkill //PID "$pid" //F > /dev/null 2>&1
    echo "KILLED: vram=${v}MiB ram=${r}MiB"
    break
  fi
done
echo "peak vram ${peak_v}MiB, peak ram ${peak_r}MiB"
sed 's/\x1b\[[0-9;]*m//g' "$out.log" | grep -E "state:|DeviceLost|panicked|Out of Memory|not yet ported" | cut -c1-160
[ -f "$out" ] && echo "snapshot: $out"
