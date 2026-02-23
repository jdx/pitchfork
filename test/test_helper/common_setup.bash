#!/usr/bin/env bash

_common_setup() {
  # Resolve project root from test file location
  PROJECT_ROOT="$(cd "$BATS_TEST_DIRNAME/.." && pwd)"
  export PROJECT_ROOT

  # Load bats libraries
  load "$PROJECT_ROOT/test/test_helper/bats-support/load"
  load "$PROJECT_ROOT/test/test_helper/bats-assert/load"
  load "$PROJECT_ROOT/test/test_helper/bats-file/load"

  # Create isolated temp directory and cd into it
  TEST_TEMP_DIR="$(temp_make)"
  export TEST_TEMP_DIR
  cd "$TEST_TEMP_DIR" || return 1

  # Ensure pitchfork binary is on PATH
  export PATH="$PROJECT_ROOT/target/debug:$PATH"

  # Isolate HOME so no user config leaks in
  export HOME="$TEST_TEMP_DIR"

  # Use a short state dir path to avoid exceeding Unix socket path limits (108 bytes)
  PITCHFORK_STATE_DIR="$(mktemp -d /tmp/pf-test-XXXXXX)"
  export PITCHFORK_STATE_DIR
  export PITCHFORK_LOGS_DIR="$PITCHFORK_STATE_DIR/logs"
  mkdir -p "$PITCHFORK_STATE_DIR" "$PITCHFORK_LOGS_DIR"
}

_common_teardown() {
  # Stop the supervisor if running
  pitchfork supervisor stop 2>/dev/null || true
  rm -rf "$PITCHFORK_STATE_DIR"
  temp_del "$TEST_TEMP_DIR"
}

# --- Helper functions ---

# Create a pitchfork.toml with daemon definitions
create_pitchfork_toml() {
  cat > pitchfork.toml
}

# Run a command with a PTY so is_terminal() returns true.
# This is critical for testing pager-related behavior — without a PTY,
# stdout is a pipe and the pager is bypassed regardless of flags.
# Usage: run_with_pty timeout 3 pitchfork logs ticker --tail
run_with_pty() {
  if [[ "$(uname)" == "Darwin" ]]; then
    script -q /dev/null "$@"
  else
    script -qec "$*" /dev/null
  fi
}

# Wait for a daemon to reach a given status (up to 10s)
wait_for_status() {
  local daemon="$1"
  local expected="$2"
  for _ in $(seq 1 50); do
    if pitchfork status "$daemon" 2>/dev/null | grep -q "$expected"; then
      return 0
    fi
    sleep 0.2
  done
  echo "Timed out waiting for $daemon to reach status: $expected" >&2
  return 1
}

# Wait for a log file to have at least N lines (up to 10s)
wait_for_log_lines() {
  local daemon="$1"
  local min_lines="$2"
  local log_dir="$PITCHFORK_LOGS_DIR/$daemon"
  local log_file="$log_dir/$daemon.log"
  for _ in $(seq 1 50); do
    if [[ -f "$log_file" ]]; then
      local count
      count=$(wc -l < "$log_file" 2>/dev/null || echo 0)
      if [[ "$count" -ge "$min_lines" ]]; then
        return 0
      fi
    fi
    sleep 0.2
  done
  echo "Timed out waiting for $daemon to have $min_lines log lines" >&2
  return 1
}
