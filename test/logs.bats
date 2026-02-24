#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup

  create_pitchfork_toml <<'EOF'
[daemons.ticker]
run = "bash -c 'while true; do echo tick $(date +%s); sleep 1; done'"
EOF
}

teardown() {
  _common_teardown
}

@test "logs --tail streams without pager" {
  pitchfork start ticker
  wait_for_log_lines ticker 3

  # Run with a PTY so is_terminal() returns true — without --tail bypassing
  # the pager, less would open, block for input, and timeout would kill it
  # producing "(END)" or empty output instead of log lines.
  output="$(run_with_pty timeout 3 pitchfork logs ticker --tail 2>&1)" || true
  [[ "$output" == *"tick"* ]]
  [[ "$output" != *"(END)"* ]]
}

@test "logs -n shows last N lines without pager" {
  pitchfork start ticker
  wait_for_log_lines ticker 5

  run pitchfork logs ticker -n 3 --raw
  assert_success
  assert_output --partial "tick"
  # Should have at most 3 lines
  local line_count
  line_count="$(echo "$output" | wc -l | tr -d ' ')"
  [[ "$line_count" -le 3 ]]
}

@test "logs --follow is an alias for --tail" {
  pitchfork start ticker
  wait_for_log_lines ticker 3

  # Same PTY test as --tail to verify --follow maps to the same code path
  output="$(run_with_pty timeout 3 pitchfork logs ticker --follow 2>&1)" || true
  [[ "$output" == *"tick"* ]]
  [[ "$output" != *"(END)"* ]]
}
