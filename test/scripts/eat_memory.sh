#!/usr/bin/env bash
# Allocate and hold a specified amount of memory (default 64MB).
# Used for testing memory_limit / resource violation behavior.
#
# Usage: eat_memory.sh [MB]
#
# Prints "Starting memory allocation of NMB" BEFORE allocating so the log line
# is captured even if the process is killed mid-allocation.
set -euo pipefail
target_mb="${1:-64}"

echo "Starting memory allocation of ${target_mb}MB"

# Prefer an in-process allocation so the daemon's own RSS exceeds the limit.
# This is more reliable than writing to /dev/shm, which may be charged to page
# cache rather than the process.
if command -v python3 >/dev/null 2>&1; then
  exec python3 -u -c "
import time
# Touch every page so RSS grows immediately.
data = bytearray(${target_mb} * 1024 * 1024)
print('Allocated ${target_mb}MB of memory')
while True:
    time.sleep(1)
"
fi

# Fallback: allocate 1MB chunks in a bash array. This is slower and less
# reliable, but works when python3 is not available.
if [[ -d /dev/shm ]]; then
  chunk_size=$((1024 * 1024)) # 1MB
  chunk_file="/dev/shm/eat_memory_$$"

  for ((i = 0; i < target_mb; i++)); do
    dd if=/dev/zero of="${chunk_file}_${i}" bs="$chunk_size" count=1 2>/dev/null
  done
  echo "Allocated ${target_mb}MB of memory"
  # Hold the memory and stay alive
  while true; do
    sleep 1
  done
else
  declare -a buffers
  for ((i = 0; i < target_mb; i++)); do
    buffers+=("$(printf 'x%.0s' {1..1048576})")
  done
  echo "Allocated ${target_mb}MB of memory"
  while true; do
    sleep 1
  done
fi
