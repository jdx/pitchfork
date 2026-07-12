#!/usr/bin/env bash
# Print text wrapped in ANSI color escape codes.
# Usage: ansi_output.sh <color_code> <text>
#   e.g. ansi_output.sh 32 green   -> prints "green" in green
#        ansi_output.sh 31 red      -> prints "red" in red
#        ansi_output.sh 34 blue     -> prints "blue" in blue
set -euo pipefail
color="${1:-32}"
text="${2:-hello}"
printf '\033[%sm%s\033[0m\n' "$color" "$text"
