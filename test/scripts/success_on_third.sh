#!/usr/bin/env bash
# Succeed on the third invocation, fail on the first two.
# Uses a counter file keyed by TEST_SUCCESS_ON_THIRD_TIMESTAMP env var
# to track invocations across separate process starts.
#
# Usage: success_on_third.sh
# Requires: TEST_SUCCESS_ON_THIRD_TIMESTAMP environment variable
set -euo pipefail

key="${TEST_SUCCESS_ON_THIRD_TIMESTAMP:-}"
if [[ -z "$key" ]]; then
  echo "Missing environment variable: TEST_SUCCESS_ON_THIRD_TIMESTAMP" >&2
  exit 2
fi

count_file="${TMPDIR:-/tmp}/retry_count_${key}"

count=0
if [[ -f "$count_file" ]]; then
  count="$(cat "$count_file")"
fi
count=$((count + 1))
echo "$count" > "$count_file"

echo "Attempt ${count} (key=${key})"

if [[ "$count" -lt 3 ]]; then
  exit 1
else
  echo "Success!"
  rm -f "$count_file"
  exit 0
fi
