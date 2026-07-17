#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# ============================================================================
# Fail task tests
# ============================================================================

@test "instant fail task fails start command quickly" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.instant_fail]
run = 'bash $fail_script 0'
ready_delay = 3
EOF

  run pitchfork start instant_fail
  assert_failure

  wait_for_logs instant_fail "Failed after 0!" 5
  run pitchfork logs instant_fail --raw
  assert_output --partial "Failed after 0!"
}

@test "two second fail task fails before default ready check" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.two_sec_fail]
run = 'bash $fail_script 2'
retry = 0
ready_delay = 3
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start two_sec_fail
  elapsed=$(($(date +%s) - start_time))

  assert_failure
  [[ $elapsed -ge 2 ]]

  wait_for_logs two_sec_fail "Failed after 2!" 5
}

@test "four second fail task passes ready check before failing" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.four_sec_fail]
run = 'bash $fail_script 4'
retry = 0
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start four_sec_fail
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -ge 3 ]]

  pitchfork stop four_sec_fail
}

# ============================================================================
# CLI command tests
# ============================================================================

@test "list command shows running daemon" {
  create_pitchfork_toml <<EOF
[daemons.test_list]
run = "sleep 10"
EOF

  run pitchfork start test_list
  assert_success

  run pitchfork list
  assert_success
  assert_output --partial "test_list"

  pitchfork stop test_list
}

@test "list shows error messages for failed daemons" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.list_error_test]
run = 'bash $fail_script 0'
EOF

  run pitchfork start list_error_test
  assert_failure

  sleep 0.5

  run pitchfork list
  assert_success
  assert_output --partial "list_error_test"
  assert_output --partial "exit code"
}

@test "list shows available daemons" {
  create_pitchfork_toml <<EOF
[daemons.available_daemon]
run = "sleep 10"

[daemons.running_daemon]
run = "sleep 10"
EOF

  run pitchfork start running_daemon
  assert_success
  sleep 0.5

  run pitchfork list
  assert_success
  assert_output --partial "available_daemon"
  assert_output --partial "running_daemon"
  assert_output --partial "available"

  local running_line
  running_line=$(grep "running_daemon" <<< "$output")
  [[ "$running_line" != *available* ]]

  pitchfork stop running_daemon
}

@test "wait command waits for daemon to exit" {
  local slowly_output_script
  slowly_output_script="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.test_wait]
run = 'bash $slowly_output_script 1 3'
ready_delay = 0
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start test_wait
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -lt 2 ]]

  start_time=$(date +%s)
  pitchfork wait test_wait &
  local wait_pid=$!
  wait $wait_pid
  assert_success
  elapsed=$(($(date +%s) - start_time))

  [[ $elapsed -ge 2 ]]
  [[ $elapsed -lt 6 ]]
}

@test "status command returns running daemon info" {
  create_pitchfork_toml <<EOF
[daemons.test_status]
run = "sleep 10"
EOF

  run pitchfork start test_status
  assert_success
  sleep 1

  run pitchfork status test_status
  assert_success
  assert_output --partial "running"
  assert_output --partial "test_status"

  pitchfork stop test_status
}

# ============================================================================
# Retry tests
# ============================================================================

@test "retry zero fails immediately with one attempt" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.retry_zero]
run = 'bash $fail_script 0'
retry = 0
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start retry_zero
  elapsed=$(($(date +%s) - start_time))

  assert_failure
  [[ $elapsed -lt 3 ]]

  wait_for_logs retry_zero "Failed after 0!" 5
  run pitchfork logs retry_zero --raw
  local count
  count=$(grep -c "Failed after 0!" <<< "$output")
  [[ $count -eq 1 ]]
}

@test "retry three retries with exponential backoff" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.retry_three]
run = 'bash $fail_script 0'
ready_delay = 1
retry = 3
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start retry_three
  elapsed=$(($(date +%s) - start_time))

  assert_failure
  [[ $elapsed -ge 7 ]]

  wait_for_logs retry_three "Failed after 0!" 10
  run pitchfork logs retry_three --raw
  local count
  count=$(grep -c "Failed after 0!" <<< "$output")
  [[ $count -eq 4 ]]
}

@test "retry succeeds on third attempt" {
  local success_script
  success_script="$(script_path success_on_third.sh)"

  create_pitchfork_toml <<EOF
[daemons.retry_success]
run = 'bash $success_script'
ready_delay = 1
retry = 2

[daemons.retry_success.env]
TEST_SUCCESS_ON_THIRD_TIMESTAMP = "$BATS_TEST_NAME"
EOF

  run pitchfork start retry_success
  assert_success

  wait_for_logs retry_success "Success!" 10
  run pitchfork logs retry_success --raw
  local count
  count=$(grep -c "Attempt" <<< "$output")
  [[ $count -eq 3 ]]

  pitchfork stop retry_success
}

# ============================================================================
# Ready check tests
# ============================================================================

@test "custom ready delay shortens startup wait" {
  create_pitchfork_toml <<EOF
[daemons.custom_delay]
run = "echo 'Starting' && sleep 10"
ready_delay = 1
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start custom_delay
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -lt 3 ]]

  pitchfork stop custom_delay
}

@test "ready output pattern matches and returns early" {
  create_pitchfork_toml <<EOF
[daemons.ready_pattern]
run = "echo 'Starting...' && sleep 1 && echo 'Server is READY' && sleep 10"
ready_output = "READY"
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start ready_pattern
  elapsed=$(($(date +%s) - start_time))

  assert_success
  # bats runs 16-way parallel on a shared CI runner, so stdout delivery and
  # ready-pattern matching can lag a few seconds under scheduling pressure.
  # 8s is still well under the daemon's trailing 10s sleep, so a regression
  # that skips early return (waits for daemon exit ~11s) still fails here.
  [[ $elapsed -lt 8 ]]

  wait_for_logs ready_pattern "READY" 5

  pitchfork stop ready_pattern
}

@test "ready output regex matches and returns early" {
  create_pitchfork_toml <<EOF
[daemons.ready_regex]
run = "echo 'Starting server on port 8080' && sleep 1 && echo 'Listening on http://localhost:8080' && sleep 10"
ready_output = 'Listening on http://.*:(\\d+)'
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start ready_regex
  elapsed=$(($(date +%s) - start_time))

  assert_success
  # Same parallel-bats scheduling-latency tolerance as the ready_pattern test
  # above; 8s (10s on Windows) stays well under the daemon's trailing 10s sleep.
  local max_elapsed=8
  if [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]]; then
    max_elapsed=10
  fi
  [[ $elapsed -lt $max_elapsed ]]

  wait_for_logs ready_regex "Listening on" 5

  pitchfork stop ready_regex
}

@test "ready output never matching blocks until daemon exits" {
  local slowly_output_script
  slowly_output_script="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.ready_no_match]
run = 'bash $slowly_output_script 1 3'
ready_output = "NEVER_APPEARS"
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start ready_no_match
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -ge 3 ]]

  wait_for_logs ready_no_match "Output 3/3" 5
}

@test "ready output beats ready delay" {
  create_pitchfork_toml <<EOF
[daemons.ready_both]
run = 'echo "READY NOW" && sleep 10'
ready_output = "READY"
ready_delay = 5
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start ready_both
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -lt 2 ]]

  pitchfork stop ready_both
}

# ============================================================================
# Integration tests
# ============================================================================

@test "multiple daemons can start and list together" {
  create_pitchfork_toml <<EOF
[daemons.daemon1]
run = "echo 'Daemon 1' && sleep 10"

[daemons.daemon2]
run = "echo 'Daemon 2' && sleep 10"

[daemons.daemon3]
run = "echo 'Daemon 3' && sleep 10"
EOF

  run pitchfork start --all
  assert_success
  sleep 4

  run pitchfork list
  assert_success
  assert_output --partial "daemon1"
  assert_output --partial "daemon2"
  assert_output --partial "daemon3"

  pitchfork stop --all
}
# ============================================================================
# HTTP / port / cmd ready checks
# ============================================================================

@test "ready http check waits for server to be ready" {
  kill_port 18081
  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.http_test]
run = 'python3 -u $http_script 1 18081'
ready_http = "http://localhost:18081/health"
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start http_test
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -ge 1 ]]
  [[ $elapsed -lt 10 ]]

  wait_for_logs http_test "Server listening" 5

  pitchfork stop http_test
}

@test "ready http check with custom status" {
  kill_port 18084
  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.http_status_test]
run = 'python3 -u $http_script 1 18084 401'
ready_http = { url = "http://localhost:18084/health", status = [401] }
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start http_status_test
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -ge 1 ]]
  [[ $elapsed -lt 10 ]]

  pitchfork stop http_status_test
}

@test "ready port check waits for port to be listening" {
  kill_port 18082
  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.port_test]
run = 'python3 -u $http_script 1 18082'
ready_port = 18082
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start port_test
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -ge 1 ]]
  [[ $elapsed -lt 10 ]]

  wait_for_logs port_test "Server listening" 5

  pitchfork stop port_test
}

@test "ready port timeout fails daemon" {
  kill_port 18083

  create_pitchfork_toml <<EOF
[daemons.port_timeout_test]
run = "sleep 30"
ready_port = { port = 18083, timeout = "3s" }
retry = 0
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start port_timeout_test
  elapsed=$(($(date +%s) - start_time))

  assert_failure
  [[ $elapsed -ge 2 ]]
  # Windows needs more time for taskkill /F /T + output drain after the
  # ready-check timeout fires.
  local max_elapsed=10
  if [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]]; then
    max_elapsed=30
  fi
  [[ $elapsed -lt $max_elapsed ]]

  wait_for_status port_timeout_test errored
}

@test "ready output timeout fails daemon" {
  create_pitchfork_toml <<EOF
[daemons.output_timeout_test]
run = "while true; do echo 'still starting'; sleep 1; done"
ready_output = { pattern = "READY", timeout = "3s" }
retry = 0
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start output_timeout_test
  elapsed=$(($(date +%s) - start_time))

  assert_failure
  [[ $elapsed -ge 2 ]]
  [[ $elapsed -lt 10 ]]

  wait_for_status output_timeout_test errored
}

@test "ready cmd check waits for command to succeed" {
  local marker
  marker="$TEST_TEMP_DIR/ready_marker"

  create_pitchfork_toml <<EOF
[daemons.cmd_test]
run = "echo Starting; sleep 1; touch $marker; echo Ready; sleep 60"
ready_cmd = 'test -f $marker'
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start cmd_test
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -ge 1 ]]
  [[ $elapsed -lt 10 ]]

  assert_file_exists "$marker"

  wait_for_logs cmd_test "Ready" 5
  run pitchfork logs cmd_test --raw
  assert_output --partial "Starting"
  assert_output --partial "Ready"

  pitchfork stop cmd_test
}

@test "ready cmd timeout fails daemon and blocks dependent" {
  create_pitchfork_toml <<EOF
[daemons.never_ready]
run = "sleep 30"
ready_cmd = { run = "false", timeout = "3s" }
retry = 0

[daemons.dependent]
run = "sleep 30"
depends = ["never_ready"]
ready_delay = 1
retry = 0
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start never_ready
  elapsed=$(($(date +%s) - start_time))

  assert_failure
  [[ $elapsed -ge 2 ]]
  [[ $elapsed -lt 10 ]]

  wait_for_status never_ready errored

  start_time=$(date +%s)
  run pitchfork start dependent
  elapsed=$(($(date +%s) - start_time))

  assert_failure
  [[ $elapsed -lt 10 ]]

  run pitchfork status dependent
  refute_output --partial "running"
}

@test "ready cmd probe survives concurrent output lines" {
  create_pitchfork_toml <<EOF
[daemons.cmd_output_race]
run = "while true; do echo tick; sleep 0.05; done"
ready_cmd = "sleep 1; true"
EOF

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start cmd_output_race
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -ge 1 ]]
  [[ $elapsed -lt 15 ]]

  pitchfork stop cmd_output_race
}

# ============================================================================
# Dir and env tests
# ============================================================================

@test "daemon dir relative sets working directory" {
  mkdir -p mysubdir
  local marker
  marker="$TEST_TEMP_DIR/dir_test_marker"

  create_pitchfork_toml <<EOF
[daemons.dir_test]
run = "pwd > $marker && sleep 60"
dir = "mysubdir"
ready_delay = 1
EOF

  run pitchfork start dir_test
  assert_success

  sleep 0.5

  run cat "$marker"
  assert_path_equal "$(cd mysubdir && pwd)" "$output"

  pitchfork stop dir_test
}

@test "daemon dir absolute sets working directory" {
  local abs_dir marker
  abs_dir="$TEST_TEMP_DIR/absolute_dir"
  abs_dir="$(normalize_path "$abs_dir")"
  mkdir -p "$abs_dir"
  marker="$TEST_TEMP_DIR/dir_abs_test_marker"

  create_pitchfork_toml <<EOF
[daemons.dir_abs_test]
run = "pwd > $marker && sleep 60"
dir = "$abs_dir"
ready_delay = 1
EOF

  run pitchfork start dir_abs_test
  assert_success

  sleep 0.5

  run cat "$marker"
  assert_path_equal "$abs_dir" "$output"

  pitchfork stop dir_abs_test
}

@test "daemon env vars are passed to process" {
  local marker
  marker="$TEST_TEMP_DIR/env_test_marker"

  create_pitchfork_toml <<EOF
[daemons.env_test]
run = "echo \$MY_TEST_VAR > $marker && sleep 60"
ready_delay = 1

[daemons.env_test.env]
MY_TEST_VAR = "hello_from_pitchfork"
EOF

  run pitchfork start env_test
  assert_success

  sleep 0.5

  run cat "$marker"
  assert_output "hello_from_pitchfork"

  pitchfork stop env_test
}

@test "daemon multiple env vars are passed to process" {
  local marker
  marker="$TEST_TEMP_DIR/multi_env_test_marker"

  create_pitchfork_toml <<EOF
[daemons.multi_env_test]
run = "echo \$VAR_A:\$VAR_B:\$VAR_C > $marker && sleep 60"
ready_delay = 1

[daemons.multi_env_test.env]
VAR_A = "alpha"
VAR_B = "beta"
VAR_C = "gamma"
EOF

  run pitchfork start multi_env_test
  assert_success

  sleep 0.5

  run cat "$marker"
  assert_output "alpha:beta:gamma"

  pitchfork stop multi_env_test
}

@test "daemon dir and env work together" {
  mkdir -p combined_test_dir
  local marker
  marker="$TEST_TEMP_DIR/combined_test_marker"

  create_pitchfork_toml <<EOF
[daemons.combined_test]
run = "echo \$MY_PORT:\$(pwd) > $marker && sleep 60"
dir = "combined_test_dir"
ready_delay = 1

[daemons.combined_test.env]
MY_PORT = "8080"
EOF

  run pitchfork start combined_test
  assert_success

  sleep 0.5

  local expected_dir
  expected_dir="$(cd combined_test_dir && pwd)"

  run cat "$marker"
  assert_path_equal "8080:$expected_dir" "$output"

  pitchfork stop combined_test
}

# ============================================================================
# Stop command tests
# ============================================================================

@test "stop transitions daemon to stopped" {
  create_pitchfork_toml <<EOF
[daemons.stop_test]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start stop_test
  assert_success

  wait_for_status stop_test running

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork stop stop_test
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -lt 5 ]]

  wait_for_status stop_test stopped
}

@test "stop kills child processes" {
  create_pitchfork_toml <<EOF
[daemons.stop_children_test]
run = "sleep 60 & sleep 60 & wait"
ready_delay = 1
EOF

  run pitchfork start stop_children_test
  assert_success

  wait_for_status stop_children_test running

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork stop stop_children_test
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -lt 10 ]]

  wait_for_status stop_children_test stopped
}

@test "stop already stopped daemon is handled gracefully" {
  create_pitchfork_toml <<EOF
[daemons.already_stopped_test]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start already_stopped_test
  assert_success

  run pitchfork stop already_stopped_test
  assert_success

  wait_for_status already_stopped_test stopped

  run pitchfork stop already_stopped_test
  wait_for_status already_stopped_test stopped
}

# ============================================================================
# Ad-hoc daemon tests
# ============================================================================

@test "ad-hoc daemon can be restarted" {
  create_pitchfork_toml <<EOF
EOF

  run pitchfork run adhoc_test --delay 1 -- sleep 60
  assert_success

  wait_for_status adhoc_test running

  local original_pid
  original_pid=$(get_daemon_pid adhoc_test)
  [[ -n "$original_pid" ]]

  run pitchfork restart adhoc_test
  assert_success

  sleep 2

  wait_for_status adhoc_test running

  local new_pid
  new_pid=$(get_daemon_pid adhoc_test)
  [[ "$new_pid" != "$original_pid" ]]

  pitchfork stop adhoc_test
}

@test "restart all includes ad-hoc daemons" {
  create_pitchfork_toml <<EOF
[daemons.config_daemon]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start config_daemon
  assert_success

  run pitchfork run adhoc_daemon --delay 1 -- sleep 60
  assert_success

  wait_for_status config_daemon running
  wait_for_status adhoc_daemon running

  local config_pid adhoc_pid
  config_pid=$(get_daemon_pid config_daemon)
  adhoc_pid=$(get_daemon_pid adhoc_daemon)
  [[ -n "$config_pid" ]]
  [[ -n "$adhoc_pid" ]]

  run pitchfork restart --all
  assert_success

  sleep 2

  wait_for_status config_daemon running
  wait_for_status adhoc_daemon running

  local new_config_pid new_adhoc_pid
  new_config_pid=$(get_daemon_pid config_daemon)
  new_adhoc_pid=$(get_daemon_pid adhoc_daemon)
  [[ "$new_config_pid" != "$config_pid" ]]
  [[ "$new_adhoc_pid" != "$adhoc_pid" ]]

  pitchfork stop --all
}
