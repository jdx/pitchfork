#!/usr/bin/env bash
# Fail after N seconds (default 0).
# Usage: fail.sh [seconds]
# Prints "Failed after N!" to stdout then exits 1.
set -euo pipefail
secs="${1:-0}"
sleep "$secs"
echo "Failed after ${secs}!"
exit 1
