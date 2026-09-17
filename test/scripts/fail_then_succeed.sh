#!/usr/bin/env bash
# Run for a while, then fail on the first invocation and succeed afterwards.
#
# Unlike success_on_third.sh this one takes time before failing, so a second
# request has a window in which to start waiting on the running daemon before
# the attempt fails and the retry backoff begins.
#
# Usage: fail_then_succeed.sh <key> <seconds>
# Requires: a key unique to the test run, used to track invocations across
# separate process starts.
set -uo pipefail

key="$1"
secs="${2:-0}"
count_file="${TMPDIR:-/tmp}/fail_then_succeed_${key}"

count=0
if [[ -f "$count_file" ]]; then
  count="$(cat "$count_file")"
fi
count=$((count + 1))
echo "$count" > "$count_file"

echo "attempt ${count} started"
sleep "$secs"

if [[ "$count" -lt 2 ]]; then
  echo "failing attempt ${count}"
  exit 1
fi

echo "succeeded on attempt ${count}"
rm -f "$count_file"
exit 0
