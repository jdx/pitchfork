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

1. Start with 1-2 paragraphs summarizing key changes
2. Organize into ### sections (Highlights, Bug Fixes, etc.)
3. Explain WHY changes matter to users
4. Include PR links and documentation links (https://pitchfork.jdx.dev/)
5. Include contributor usernames (@username)
6. Skip internal changes

Output ONLY the release notes, no preamble.
INSTRUCTIONS
)

# Use Claude Code to generate the release notes
# Sandboxed: only read-only tools allowed (no Bash, Edit, Write)
echo "Generating release notes with Claude..." >&2
echo "Version: $tag" >&2
echo "Previous version: ${prev_tag:-none}" >&2
echo "Changelog length: ${#changelog} chars" >&2

# Capture stderr separately to avoid polluting output
stderr_file=$(mktemp)
trap 'rm -f "$stderr_file"' EXIT

if ! output=$(
	printf '%s' "$prompt" | claude -p \
		--model claude-opus-4-20250514 \
		--output-format text \
		--allowedTools "Read,Grep,Glob" 2>"$stderr_file"
); then
	echo "Error: Claude CLI failed" >&2
	cat "$stderr_file" >&2
	exit 1
fi

# Validate we got non-empty output
if [[ -z $output ]]; then
	echo "Error: Claude returned empty output" >&2
	cat "$stderr_file" >&2
	exit 1
fi

echo "$output"
