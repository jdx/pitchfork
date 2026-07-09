#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# ============================================================================
# Tail / follow tests
# ============================================================================

@test "logs --tail streams newly appended output without pager" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.tail_test]
run = "bash $slowly_output 1 10"
ready_delay = 0
EOF

  pitchfork start tail_test
  wait_for_logs tail_test "Output 1/10" 10

  local output
  output="$(run_with_pty timeout 4 pitchfork logs tail_test --tail 2>&1)" || true

  [[ "$output" != *"(END)"* ]]
  [[ "$output" == *"Output "* ]]

  pitchfork stop tail_test
}

@test "logs --follow is an alias for --tail" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.follow_test]
run = "bash $slowly_output 1 10"
ready_delay = 0
EOF

  pitchfork start follow_test
  wait_for_logs follow_test "Output 1/10" 10

  local output
  output="$(run_with_pty timeout 4 pitchfork logs follow_test --follow 2>&1)" || true

  [[ "$output" != *"(END)"* ]]
  [[ "$output" == *"Output "* ]]

  pitchfork stop follow_test
}

# ============================================================================
# Clear logs
# ============================================================================

@test "logs --clear removes logs for all daemons" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.clear_test_1]
run = "bash $slowly_output 1 3"
ready_delay = 0

[daemons.clear_test_2]
run = "bash $slowly_output 1 3"
ready_delay = 0
EOF

  pitchfork start clear_test_1
  pitchfork start clear_test_2
  sleep 4

  [[ -n "$(read_logs clear_test_1)" ]]
  [[ -n "$(read_logs clear_test_2)" ]]

  run pitchfork logs --clear
  assert_success

  [[ -z "$(read_logs clear_test_1)" ]]
  [[ -z "$(read_logs clear_test_2)" ]]

  pitchfork stop clear_test_1
  pitchfork stop clear_test_2
}

# ============================================================================
# Time filter tests (--since / --until)
# ============================================================================

@test "logs --since with relative time shows recent logs" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.since_test]
run = "bash $slowly_output 1 5"
ready_delay = 0
EOF

  pitchfork start since_test
  sleep 6

  run pitchfork logs since_test --since 3s --raw
  assert_success
  assert_output --partial "Output"

  pitchfork stop since_test
}

@test "logs --since with time only succeeds" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.time_only_test]
run = "bash $slowly_output 1 3"
ready_delay = 0
EOF

  pitchfork start time_only_test
  sleep 4

  local time_str
  time_str="$(date +%H:%M)"

  run pitchfork logs time_only_test --since "$time_str" --raw
  assert_success

  pitchfork stop time_only_test
}

@test "logs --since and --until return logs in range" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.range_test]
run = "bash $slowly_output 1 5"
ready_delay = 0
EOF

  local start_time mid_time
  start_time="$(date +"%Y-%m-%d %H:%M:%S")"

  pitchfork start range_test
  sleep 3
  mid_time="$(date +"%Y-%m-%d %H:%M:%S")"
  sleep 3

  run pitchfork logs range_test --since "$start_time" --until "$mid_time" --raw
  assert_success

  local count
  count="$(grep -c "Output" <<< "$output" || true)"
  [[ "$count" -ge 1 ]]

  pitchfork stop range_test
}

@test "logs --since respects -n line limit" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.since_n_test]
run = "bash $slowly_output 1 10"
ready_delay = 0
EOF

  pitchfork start since_n_test
  sleep 11

  run pitchfork logs since_n_test --since 10s -n 3 --raw
  assert_success

  local count
  count="$(grep -c "Output" <<< "$output" || true)"
  [[ "$count" -le 3 ]]

  pitchfork stop since_n_test
}

# ============================================================================
# Raw output tests
# ============================================================================

@test "logs --raw omits ANSI escape codes" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.raw_test]
run = "bash $slowly_output 1 3"
ready_delay = 0
EOF

  pitchfork start raw_test
  sleep 4

  run pitchfork logs raw_test --raw
  assert_success
  [[ -n "$output" ]]
  [[ "$output" != *$'\x1b['* ]]

  pitchfork stop raw_test
}

@test "logs --raw works with --since" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.raw_time_test]
run = "bash $slowly_output 1 5"
ready_delay = 0
EOF

  pitchfork start raw_time_test
  sleep 6

  run pitchfork logs raw_time_test --raw --since 5s
  assert_success
  [[ -n "$output" ]]

  pitchfork stop raw_time_test
}

# ============================================================================
# Line limit tests
# ============================================================================

@test "logs -n limits output to last N lines" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.n_limit_test]
run = "bash $slowly_output 1 10"
ready_delay = 0
EOF

  pitchfork start n_limit_test
  sleep 11

  run pitchfork logs n_limit_test -n 5 --raw
  assert_success

  local count
  count="$(grep -c "Output" <<< "$output" || true)"
  [[ "$count" -le 5 ]]

  pitchfork stop n_limit_test
}

@test "logs without -n outputs directly to stdout in non-interactive mode" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.pager_test]
run = "bash $slowly_output 1 3"
ready_delay = 0
EOF

  pitchfork start pager_test
  sleep 4

  run pitchfork logs pager_test
  assert_success
  assert_output --partial "Output"

  pitchfork stop pager_test
}

# ============================================================================
# Multiple daemon tests
# ============================================================================

@test "logs for multiple daemons includes each daemon" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.multi_log_1]
run = "bash $slowly_output 1 3"
ready_delay = 0

[daemons.multi_log_2]
run = "bash $slowly_output 1 3"
ready_delay = 0
EOF

  pitchfork start multi_log_1
  pitchfork start multi_log_2
  sleep 4

  run pitchfork logs multi_log_1 multi_log_2
  assert_success
  [[ "$output" == *"multi_log_1"* ]]
  [[ "$output" == *"multi_log_2"* ]]

  pitchfork stop multi_log_1
  pitchfork stop multi_log_2
}

# ============================================================================
# No-pager tests
# ============================================================================

@test "logs --no-pager outputs logs directly" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.no_pager_test]
run = "bash $slowly_output 1 3"
ready_delay = 0
EOF

  pitchfork start no_pager_test
  sleep 4

  run pitchfork logs no_pager_test --no-pager
  assert_success
  assert_output --partial "Output"

  pitchfork stop no_pager_test
}

# ============================================================================
# SSE tests
# ============================================================================

# TODO: The Rust e2e test `test_web_logs_sse_skips_existing_content_on_connect`
# started a web supervisor and consumed the SSE `/logs/project%2Fsse_connect/stream`
# endpoint. Converting it reliably to bash requires backgrounding the supervisor,
# parsing a dynamic port, and timing SSE chunks with curl. Skipping for now.
