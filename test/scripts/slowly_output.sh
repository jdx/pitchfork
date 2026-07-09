#!/usr/bin/env bash
# Output lines at regular intervals, then exit 0.
# Usage: slowly_output.sh [interval_secs] [total_outputs]
# Prints "Output i/total" every interval_secs seconds.
set -euo pipefail
interval="${1:-1}"
total="${2:-5}"
for ((i = 1; i <= total; i++)); do
  echo "Output ${i}/${total}"
  sleep "$interval"
done
exit 0
