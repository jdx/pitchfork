#!/usr/bin/env bash
set -euo pipefail

# Generate editorialized release notes using Claude Code
# Usage: ./scripts/gen-release-notes.sh <tag> [prev_tag]

tag="${1:-}"
prev_tag="${2:-}"

if [[ -z $tag ]]; then
	echo "Usage: $0 <tag> [prev_tag]" >&2
	exit 1
fi

# If no prev_tag provided, find the previous tag
if [[ -z $prev_tag ]]; then
	prev_tag=$(git tag --sort=-v:refname | grep -A1 "^${tag}$" | tail -1)
	if [[ $prev_tag == "$tag" ]]; then
		# No previous tag found, use first commit
		prev_tag=$(git rev-list --max-parents=0 HEAD)
	fi
fi

# Get conventional commits between tags
changelog=$(git log --pretty=format:"- %s (%h)" "${prev_tag}..${tag}" 2>/dev/null || git log --pretty=format:"- %s (%h)" "${tag}" 2>/dev/null || echo "")

if [[ -z $changelog ]]; then
	echo "Error: No commits found for release" >&2
	exit 1
fi

# Use Claude Code to editorialize the release notes
# Sandboxed: only read-only tools allowed (no Bash, Edit, Write)
output=$(
	claude -p \
		--model claude-opus-4-20250514 \
		--output-format text \
		--allowedTools "Read,Grep,Glob" \
		<<EOF
You are writing release notes for pitchfork version ${tag}.

Pitchfork is a daemon supervisor CLI for developers. It manages background processes with features like auto-start/stop, cron scheduling, retry logic, and HTTP ready checks.

Here are the commits in this release:
${changelog}

Write user-friendly release notes. The format should be:

1. Start with 1-2 paragraphs summarizing the most important changes
2. Organize into sections using ### headers (e.g., "### Highlights", "### Bug Fixes") - only include sections that have content
3. Write in clear, user-focused language (not developer commit messages)
4. Explain WHY changes matter to users, not just what changed
5. Group related changes together logically
6. Skip minor/internal changes that don't affect users
7. Include contributor attribution where appropriate (@username)

IMPORTANT: Use only ### for section headers. NEVER use "## [" as this pattern is reserved for version headers.

Keep the tone professional but approachable. Focus on what users care about.

Output ONLY the editorialized release notes, no preamble.
EOF
)

# Validate output doesn't contain patterns that would corrupt changelog processing
if echo "$output" | grep -qE '^## \['; then
	echo "Error: LLM output contains '## [' pattern which would corrupt processing" >&2
	exit 1
fi

echo "$output"
