#!/usr/bin/env bash
set -euo pipefail

# Generate concise changelog entry using Claude Code
# Usage: ./scripts/gen-release-summary.sh <tag> [prev_tag]

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
Write a brief changelog entry:

1. One short paragraph (2-3 sentences) summarizing the release
2. Categorized bullet points (### Features, ### Bug Fixes, etc.)
3. One line per change, no explanations
4. Skip minor/internal changes
5. Include PR links for significant changes
6. Include contributor usernames (@username)
7. Link to relevant documentation at https://pitchfork.jdx.dev/ where applicable

IMPORTANT: Use only ### for section headers. NEVER use "## [" as this pattern is reserved for version headers.

Output ONLY the brief changelog, no preamble.
INSTRUCTIONS
)

# Use Claude Code to generate the changelog entry
# Sandboxed: only read-only tools allowed (no Bash, Edit, Write)
output=$(
	printf '%s' "$prompt" | claude -p \
		--model claude-opus-4-20250514 \
		--output-format text \
		--allowedTools "Read,Grep,Glob"
)

# Validate output doesn't contain patterns that would corrupt changelog processing
if echo "$output" | grep -qE '^## \['; then
	echo "Error: LLM output contains '## [' pattern which would corrupt processing" >&2
	exit 1
fi

echo "$output"
