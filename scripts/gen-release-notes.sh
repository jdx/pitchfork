#!/usr/bin/env bash
set -euo pipefail

# Generate rich release notes for GitHub releases using Claude Code
# Usage: ./scripts/gen-release-notes.sh <tag> [prev_tag]

tag="${1:-}"
prev_tag="${2:-}"

if [[ -z $tag ]]; then
	echo "Usage: $0 <tag> [prev_tag]" >&2
	exit 1
fi

# If no prev_tag provided, find the previous tag
if [[ -z $prev_tag ]]; then
	# Use grep -F for literal matching (dots in semver are not regex wildcards)
	prev_tag=$(git tag --sort=-v:refname | grep -F -A1 "${tag}" | tail -1)
	if [[ $prev_tag == "$tag" ]]; then
		# No previous tag found, use first commit (head -1 handles multiple root commits)
		prev_tag=$(git rev-list --max-parents=0 HEAD | head -1)
	fi
fi

# Get conventional commits between tags
changelog=$(git log --pretty=format:"- %s (%h)" "${prev_tag}..${tag}" 2>/dev/null || git log --pretty=format:"- %s (%h)" "${tag}" 2>/dev/null || echo "")

if [[ -z $changelog ]]; then
	echo "Error: No commits found for release" >&2
	exit 1
fi

# Build prompt safely using printf to avoid command substitution on backticks in changelog
prompt=$(
	printf '%s\n' "You are writing release notes for pitchfork version ${tag}."
	printf '\n'
	printf '%s\n' "Pitchfork is a daemon supervisor CLI for developers. It manages background processes with features like auto-start/stop, cron scheduling, retry logic, and HTTP ready checks."
	printf '\n'
	printf '%s\n' "Here are the commits in this release:"
	printf '%s\n' "$changelog"
	printf '\n'
	cat <<'INSTRUCTIONS'
Write user-friendly release notes:

1. Start with a single "# <pithy title>" heading - a short, catchy title summarizing this release (will become the GitHub release title). For smaller or less impactful releases, keep the title understated and modest.

TONE CALIBRATION:
- Match the tone and length to the actual significance of the changes
- If the release is mostly small bug fixes or minor tweaks, be upfront about that—a sentence or two of summary is fine, don't write multiple paragraphs inflating the importance
- Reserve enthusiastic, detailed write-ups for releases with genuinely significant features or changes
- It's okay to say "This is a smaller release focused on bug fixes" when that's the case

2. Follow with a summary proportional to the significance of the changes
3. Organize into ### sections (Highlights, Bug Fixes, etc.)
4. Explain WHY changes matter to users
5. Include PR links and documentation links (https://pitchfork.jdx.dev/)
6. Include contributor usernames (@username). Do not thank @jdx since that is who is writing these notes.
7. Skip internal changes

Write your output to the file: /tmp/release-notes-output.md
Do not output anything else — just write to the file.
INSTRUCTIONS
)

# Use Claude Code to generate the release notes
# Claude writes to a temp file via the Write tool to avoid formatting artifacts from stdout
echo "Generating release notes with Claude..." >&2
echo "Version: $tag" >&2
echo "Previous version: ${prev_tag:-none}" >&2
echo "Changelog length: ${#changelog} chars" >&2

output_file="/tmp/release-notes-output.md"
rm -f "$output_file"

stderr_file=$(mktemp)
trap 'rm -f "$stderr_file" "$output_file"' EXIT

if ! printf '%s' "$prompt" | claude -p \
	--model claude-opus-4-6 \
	--permission-mode bypassPermissions \
	--allowedTools "Read,Grep,Glob,Write($output_file)" 2>"$stderr_file"; then
	echo "Error: Claude CLI failed" >&2
	cat "$stderr_file" >&2
	exit 1
fi

# Validate the output file was created and is non-empty
if [[ ! -s $output_file ]]; then
	echo "Error: Claude did not write release notes to $output_file" >&2
	cat "$stderr_file" >&2
	exit 1
fi

output=$(cat "$output_file")

# Extract title from "# ..." heading and separate from body
title=""
body="$output"
if echo "$output" | grep -q "^# "; then
	title=$(echo "$output" | grep "^# " | head -1 | sed 's/^# //')
	body=$(echo "$output" | sed "1,/^# /d")
fi

# Validate we got non-empty output
if [[ -z $body ]]; then
	echo "Error: Claude returned empty output" >&2
	cat "$stderr_file" >&2
	exit 1
fi

# Output format: title on first line, separator, then body
echo "$title"
echo "---"
echo "$body"
