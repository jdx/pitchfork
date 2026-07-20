#!/usr/bin/env bash

# Shared setup/teardown for all bats e2e tests.
#
# Design borrowed from the mise e2e framework:
# - Full environment isolation per test (unique HOME, state dir, config dir)
# - Fast watcher/poll intervals to keep tests snappy
# - Temp dirs preserved on failure for post-mortem debugging
# - Comprehensive wait helpers for async daemon behaviour

_common_setup() {
  # Resolve project root from test file location
  PROJECT_ROOT="$(cd "$BATS_TEST_DIRNAME/.." && pwd)"
  export PROJECT_ROOT

  # Load bats libraries
  load "$PROJECT_ROOT/test/test_helper/bats-support/load"
  load "$PROJECT_ROOT/test/test_helper/bats-assert/load"
  load "$PROJECT_ROOT/test/test_helper/bats-file/load"

  # Directory holding test daemon scripts (bash replacements for former bun scripts)
  export TEST_SCRIPTS_DIR="$PROJECT_ROOT/test/scripts"

  # Create isolated temp directory tree
  TEST_TEMP_DIR="$(temp_make)"
  export TEST_TEMP_DIR

  # Isolated HOME so no user config/state leaks in
  export HOME="$TEST_TEMP_DIR"

  # Isolated TMPDIR so scripts don't fall back to shared /tmp between runs
  export TMPDIR="$TEST_TEMP_DIR"

  # Isolated pitchfork state/logs/config directories.
  # Use a short path to stay under the 108-byte Unix socket path limit.
  PITCHFORK_STATE_DIR="$(mktemp -d /tmp/pf-test-XXXXXX)"
  export PITCHFORK_STATE_DIR
  export PITCHFORK_LOGS_DIR="$PITCHFORK_STATE_DIR/logs"
  export PITCHFORK_CONFIG_DIR="$TEST_TEMP_DIR/.config/pitchfork"
  mkdir -p "$PITCHFORK_STATE_DIR" "$PITCHFORK_LOGS_DIR" "$PITCHFORK_CONFIG_DIR"

  # Ensure pitchfork binary is on PATH
  export PATH="$PROJECT_ROOT/target/debug:$PATH"

  # Use sh as the daemon shell on all platforms. On Windows, the default
  # (cmd) cannot parse Unix-style commands used in test scripts. Git Bash's
  # sh.exe is available in the CI environment.
  export PITCHFORK_SHELL="sh -c"

  # Fast watcher/poll intervals for responsive tests (matches Rust e2e defaults)
  export PITCHFORK_WATCH_INTERVAL=100ms
  export PITCHFORK_WATCH_POLL_INTERVAL=100ms
  # Longer debounce: Windows ReadDirectoryChangesW may report multiple events
  # for a single file modification (truncate + write + close), spaced >1s apart.
  # A 3s debounce coalesces these into a single restart.
  export PITCHFORK_FILE_WATCH_DEBOUNCE=3s

  # Verbose logging for easier debugging
  export PITCHFORK_LOG=debug

  # Retry flaky tests when the runner asks for it. Windows CI sets
  # PITCHFORK_TEST_RETRIES because slow, highly variable process/bash startup
  # there makes the timing-sensitive assertions (ready-delay windows, elapsed
  # bounds) flaky. bats hardcodes BATS_TEST_RETRIES=0 per test file, so it must
  # be (re)set here in setup rather than via the environment, which gets
  # clobbered. Defaults to 0 (no retries) on Linux so real regressions surface.
  export BATS_TEST_RETRIES="${PITCHFORK_TEST_RETRIES:-0}"

  # Work inside the temp dir so pitchfork.toml is discovered there
  cd "$TEST_TEMP_DIR" || return 1

  # Pre-start the supervisor with output redirected to a file (not a pipe).
  # On Windows, when pitchfork auto-starts the supervisor via bats' `run`
  # (which uses pipes), the background supervisor inherits the pipe write
  # end and keeps it open after the CLI exits, causing bats to hang forever
  # waiting for pipe EOF. By starting the supervisor here with redirection
  # to /dev/null (a file, not a pipe), there's no pipe to inherit, and
  # subsequent pitchfork commands connect to the already-running supervisor
  # without spawning a new background process.
  pitchfork supervisor start --force >/dev/null 2>&1 || true
}

# Skip a test on Windows (Git Bash / MSYS2).
# Usage: skip_on_windows "reason"
skip_on_windows() {
  if [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]]; then
    skip "$1"
  fi
}

# Kill a process by PID, working on both Unix and Windows.
# On Windows Git Bash, `kill -9` may not terminate native Windows processes.
# Use taskkill //F //PID as a fallback.
kill_pid() {
  local pid="$1"
  kill -9 "$pid" 2>/dev/null || true
  if [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]]; then
    taskkill //F //PID "$pid" 2>/dev/null || true
  fi
}

# Normalize a path for cross-platform comparison.
# On Windows Git Bash, paths can appear in multiple formats:
#   /tmp/...           (Git Bash virtual mount)
#   /c/Users/...       (MSYS2 sh.exe pwd output)
#   C:/Users/...       (Windows native)
#   C:\Users\...       (Windows backslash)
# Strip to just the lowercase drive-less form for comparison.
normalize_path() {
  local p="$1"
  if command -v cygpath >/dev/null 2>&1; then
    # cygpath -m handles all input formats and outputs C:/ mixed format
    cygpath -m "$p" 2>/dev/null || echo "$p"
  else
    echo "$p"
  fi
}

# Compare two paths ignoring format differences.
# Usage: assert_path_equal "expected" "actual"
assert_path_equal() {
  local expected="$1" actual="$2"
  # Strip numeric prefixes like "8080:" before normalization.
  # Drive-letter paths (C:/...) must be left intact so cygpath -m -l
  # does not double the drive prefix.
  local prefix=""
  if [[ "$expected" =~ ^([0-9]+):(.*)$ ]]; then
    prefix="${BASH_REMATCH[1]}:"
    expected="${BASH_REMATCH[2]}"
    actual="${actual#"$prefix"}"
  fi
  if command -v cygpath >/dev/null 2>&1; then
    # -m = mixed format (C:/), -l = prefer long names over 8.3 short names
    expected="$(cygpath -m -l "$expected" 2>/dev/null || echo "$expected")"
    actual="$(cygpath -m -l "$actual" 2>/dev/null || echo "$actual")"
  fi
  expected="$prefix$expected"
  actual="$prefix$actual"
  [[ "$expected" == "$actual" ]] || {
    echo "Path mismatch:" >&2
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    return 1
  }
}

# Convert a path to a Unix-style path suitable for shell commands.
#
# On Windows Git Bash, paths may be returned in Windows native format (e.g.
# C:/Users/...). The supervisor's shell (configured as "sh -c") cannot resolve
# those paths, so convert them to the Unix-style form (/c/Users/...) that
# MSYS2 sh understands. On non-Windows platforms the path is returned unchanged.
to_shell_path() {
  local p="$1"
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -u "$p" 2>/dev/null || echo "$p"
  else
    echo "$p"
  fi
}

_common_teardown() {
  # Stop the supervisor if running (swallow errors — it may not be running)
  # Use timeout to prevent hang if supervisor stop is stuck (e.g. daemon
  # cleanup on Windows where POSIX signals are unavailable).
  timeout 10 pitchfork supervisor stop 2>/dev/null || true

  # Preserve temp dirs on failure for post-mortem debugging
  if [[ -n "$BATS_TEST_COMPLETED" && "$BATS_TEST_COMPLETED" == "1" ]]; then
    rm -rf "$PITCHFORK_STATE_DIR"
    temp_del "$TEST_TEMP_DIR" 2>/dev/null || true
  else
    echo "# Test failed — preserving debug dirs:" >&3
    echo "#   TEST_TEMP_DIR=$TEST_TEMP_DIR" >&3
    echo "#   PITCHFORK_STATE_DIR=$PITCHFORK_STATE_DIR" >&3
    # Print supervisor log file for debugging watcher/hook issues
    local sup_log="$PITCHFORK_LOGS_DIR/pitchfork/pitchfork.log"
    if [[ -f "$sup_log" ]]; then
      echo "# --- pitchfork.log (last 80 lines) ---" >&3
      tail -80 "$sup_log" 2>/dev/null | sed 's/^/#   /' >&3 || true
    fi
  fi
}

# ---------------------------------------------------------------------------
# Config helpers
# ---------------------------------------------------------------------------

# Write a pitchfork.toml from stdin into the current working directory
create_pitchfork_toml() {
  cat > pitchfork.toml
}

# ---------------------------------------------------------------------------
# Process / status helpers
# ---------------------------------------------------------------------------

# Wait for a daemon to reach a given status (up to 30s, or custom timeout)
# Usage: wait_for_status <daemon> <expected_status> [timeout_secs]
wait_for_status() {
  local daemon="$1"
  local expected="$2"
  local timeout_secs="${3:-30}"
  for _ in $(seq 1 "$((timeout_secs * 5))"); do
    if pitchfork status "$daemon" 2>/dev/null | grep -q "Status:.*$expected"; then
      return 0
    fi
    sleep 0.2
  done
  echo "Timed out waiting for $daemon to reach status: $expected" >&2
  pitchfork status "$daemon" 2>&1 >&2 || true
  return 1
}

# Get daemon status string (e.g. "running", "stopped", "errored")
get_daemon_status() {
  local daemon="$1"
  pitchfork status "$daemon" 2>/dev/null | grep '^Status:' | sed 's/^Status: *//'
}

# Get daemon PID (returns empty string if not running)
get_daemon_pid() {
  local daemon="$1"
  local pid
  pid="$(pitchfork status "$daemon" 2>/dev/null | grep '^PID:' | sed 's/^PID: *//')"
  [[ "$pid" == "-" || -z "$pid" ]] && echo "" || echo "$pid"
}

# ---------------------------------------------------------------------------
# Log helpers
# ---------------------------------------------------------------------------

# Read raw logs for a daemon (no ANSI, no pager)
read_logs() {
  local daemon="$1"
  pitchfork logs "$daemon" --raw 2>/dev/null
}

# Wait for a daemon's logs to contain a substring (up to N seconds, default 10)
# Usage: wait_for_logs <daemon> <needle> [timeout_secs]
wait_for_logs() {
  local daemon="$1"
  local needle="$2"
  local timeout_secs="${3:-10}"
  for _ in $(seq 1 "$((timeout_secs * 5))"); do
    if pitchfork logs "$daemon" --raw 2>/dev/null | grep -q "$needle"; then
      return 0
    fi
    sleep 0.2
  done
  echo "Timed out waiting for logs of '$daemon' to contain: $needle" >&2
  pitchfork logs "$daemon" --raw 2>&1 | tail -20 >&2 || true
  return 1
}

# Wait for a daemon to have at least N log lines (up to 10s)
# Usage: wait_for_log_lines <daemon> <min_lines>
wait_for_log_lines() {
  local daemon="$1"
  local min_lines="$2"
  for _ in $(seq 1 50); do
    local count
    count="$(pitchfork logs "$daemon" --raw 2>/dev/null | wc -l | tr -d ' ')"
    if [[ "$count" -ge "$min_lines" ]]; then
      return 0
    fi
    sleep 0.2
  done
  echo "Timed out waiting for $daemon to have $min_lines log lines" >&2
  return 1
}

# ---------------------------------------------------------------------------
# File helpers
# ---------------------------------------------------------------------------

# Wait up to 5s for a file to exist
# Usage: wait_for_file <path>
wait_for_file() {
  local file="$1"
  for _ in $(seq 1 50); do
    if [[ -e "$file" ]]; then
      return 0
    fi
    sleep 0.1
  done
  echo "Timed out waiting for file: $file" >&2
  return 1
}

# Wait up to 5s for a file to contain exact content (trimmed)
# Usage: wait_for_file_content <path> <expected_content>
wait_for_file_content() {
  local file="$1"
  local expected="$2"
  for _ in $(seq 1 50); do
    if [[ -e "$file" ]]; then
      local content
      content="$(cat "$file" 2>/dev/null)"
      if [[ "$(echo "$content" | tr -d '[:space:]')" == "$(echo "$expected" | tr -d '[:space:]')" ]]; then
        return 0
      fi
    fi
    sleep 0.1
  done
  echo "Timed out waiting for $file to contain: $expected" >&2
  [[ -e "$file" ]] && echo "Actual content: $(cat "$file")" >&2
  return 1
}

# ---------------------------------------------------------------------------
# PTY / terminal helpers
# ---------------------------------------------------------------------------

# Run a command with a PTY so is_terminal() returns true.
# Usage: run_with_pty timeout 3 pitchfork logs ticker --tail
run_with_pty() {
  if [[ "$(uname)" == "Darwin" ]]; then
    script -q /dev/null "$@"
  elif [[ "$(uname)" == MINGW* || "$(uname)" == MSYS* ]]; then
    # Windows: use winpty if available, otherwise run without PTY
    if command -v winpty >/dev/null 2>&1; then
      winpty "$@"
    else
      "$@"
    fi
  else
    script -qec "$*" /dev/null
  fi
}

# ---------------------------------------------------------------------------
# Port helpers
# ---------------------------------------------------------------------------

# Kill any process listening on the specified port
kill_port() {
  local port="$1"
  if command -v lsof >/dev/null 2>&1; then
    local pids
    pids="$(lsof -ti ":$port" 2>/dev/null)" || true
    for pid in $pids; do
      kill -9 "$pid" 2>/dev/null || true
    done
  elif command -v fuser >/dev/null 2>&1; then
    fuser -k "${port}/tcp" 2>/dev/null || true
  # Use a kill_port fallback that works without rg (not installed on Windows CI)
  elif command -v netstat >/dev/null 2>&1; then
    # Windows: use netstat + taskkill
    local pids
    pids="$(netstat -ano 2>/dev/null | grep ":${port}\s.*LISTENING" | awk '{print $NF}' | sort -u)" || true
    for pid in $pids; do
      taskkill //F //PID "$pid" 2>/dev/null || true
    done
  fi
  sleep 0.1
}

# Bind to a port to simulate it being in use (background, 5s lifetime)
# Usage: occupy_port <port>
occupy_port() {
  local port="$1"
  nohup python3 -c "
import socket, time
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('0.0.0.0', $port))
s.listen(1)
time.sleep(5)
" >/dev/null 2>&1 &
  echo $!
}

# ---------------------------------------------------------------------------
# Path / script helpers
# ---------------------------------------------------------------------------

# Get absolute path to a test script
# Usage: script_path fail.sh
script_path() {
  echo "$TEST_SCRIPTS_DIR/$1"
}
