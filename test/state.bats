#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# Read state.toml directly
read_state() { cat "$PITCHFORK_STATE_DIR/state.toml"; }

# Portable file mtime (seconds since epoch)
file_mtime() {
  if stat --version &>/dev/null 2>&1; then
    stat -c %Y "$1"
  else
    stat -f %m "$1"
  fi
}

# Wait for a substring to appear in the persisted state file
wait_for_state() {
  local needle="$1"
  for _ in $(seq 1 50); do
    if read_state | grep -q "$needle"; then
      return 0
    fi
    sleep 0.1
  done
  echo "Timed out waiting for state to contain: $needle" >&2
  return 1
}

@test "state file survives truncation and recovers" {
  create_pitchfork_toml <<EOF
[daemons.state_test]
run = "sleep 10"
EOF

  run pitchfork start state_test
  assert_success
  wait_for_status state_test running

  assert_file_exists "$PITCHFORK_STATE_DIR/state.toml"

  pitchfork supervisor stop

  # Truncate state file to invalid TOML
  echo "invalid toml [[" > "$PITCHFORK_STATE_DIR/state.toml"

  run pitchfork list
  # Must not crash; a clean failure or graceful recovery is acceptable
  [[ "$status" -eq 0 || "$status" -eq 1 ]]
  [[ "$output" != *"panic"* ]]
  [[ "$output" != *"SIGSEGV"* ]]
}

@test "state file is not rewritten when content is unchanged" {
  create_pitchfork_toml <<EOF
[daemons.state_test]
run = "sleep 10"
EOF

  run pitchfork start state_test
  assert_success
  wait_for_status state_test running

  # Let the background flush task settle
  sleep 2

  local mtime_before
  mtime_before=$(file_mtime "$PITCHFORK_STATE_DIR/state.toml")

  run pitchfork list
  assert_success
  run pitchfork list
  assert_success
  run pitchfork list
  assert_success

  local mtime_after
  mtime_after=$(file_mtime "$PITCHFORK_STATE_DIR/state.toml")

  [[ "$mtime_before" -eq "$mtime_after" ]]

  pitchfork stop state_test
}

@test "concurrent state writes do not corrupt the file" {
  create_pitchfork_toml <<EOF
[daemons.daemon1]
run = "sleep 10"

[daemons.daemon2]
run = "sleep 10"
EOF

  # Ensure the supervisor is running to avoid two starts racing each other
  pitchfork supervisor start

  # Start both daemons concurrently
  pitchfork start daemon1 &
  local pid1=$!
  pitchfork start daemon2 &
  local pid2=$!

  wait "$pid1"
  wait "$pid2"

  wait_for_status daemon1 running
  wait_for_status daemon2 running

  # Both daemons should appear in the state file and in list output
  run pitchfork list
  assert_success
  assert_output --partial "daemon1"
  assert_output --partial "daemon2"

  run read_state
  assert_output --partial "daemon1"
  assert_output --partial "daemon2"

  pitchfork stop daemon1
  pitchfork stop daemon2
}

@test "active_port is cleared when daemon exits" {
  kill_port 18080

  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.port_daemon]
run = 'python3 -u $http_script 1 18080'
port = 18080
ready_port = 18080
EOF

  run pitchfork start port_daemon
  assert_success
  wait_for_status port_daemon running

  # active_port detection is async; wait for it to land in state
  wait_for_state "active_port = 18080"

  run read_state
  assert_output --partial "active_port = 18080"

  run pitchfork stop port_daemon
  assert_success
  wait_for_status port_daemon stopped

  run read_state
  refute_output --partial "active_port = 18080"
}

@test "clean removes stopped daemons from state" {
  create_pitchfork_toml <<EOF
[daemons.quick_exit]
run = 'bash $(script_path fail.sh) 0'
retry = 0

[daemons.stays_running]
run = "sleep 10"
EOF

  pitchfork start quick_exit || true
  run pitchfork start stays_running
  assert_success
  wait_for_status stays_running running

  # Wait for quick_exit to finish and be recorded in state
  for _ in $(seq 1 50); do
    local daemon_status
    daemon_status=$(get_daemon_status quick_exit)
    if [[ "$daemon_status" != "running" && -n "$daemon_status" ]]; then
      break
    fi
    sleep 0.2
  done

  wait_for_state "quick_exit"

  run read_state
  assert_output --partial "quick_exit"

  run pitchfork list
  assert_output --partial "stays_running"

  run pitchfork clean
  assert_success

  # The stopped daemon should be removed from the persisted state file
  run read_state
  refute_output --partial "quick_exit"

  # The running daemon should still be listed
  run pitchfork list
  assert_output --partial "stays_running"

  pitchfork stop stays_running
}

@test "shell directory registration and removal" {
  # Ensure supervisor is running
  pitchfork supervisor start

  run pitchfork cd --shell-pid $$
  assert_success

  run read_state
  assert_output --partial "[shell_dirs]"
  assert_output --partial "$(pwd)"

  # Leave the directory and verify the old directory is removed from state
  cd /tmp
  run pitchfork cd --shell-pid $$
  assert_success

  run read_state
  refute_output --partial "$TEST_TEMP_DIR"
}

@test "disable and enable daemon persist in state" {
  create_pitchfork_toml <<EOF
[daemons.toggle_test]
run = "sleep 10"
EOF

  run pitchfork start toggle_test
  assert_success
  wait_for_status toggle_test running
  wait_for_state "toggle_test"

  run pitchfork disable toggle_test
  assert_success
  wait_for_state "disabled"

  run pitchfork list
  assert_output --partial "toggle_test"
  assert_output --partial "disabled"

  pitchfork supervisor stop
  sleep 1
  pitchfork supervisor start

  run pitchfork list
  assert_output --partial "toggle_test"
  assert_output --partial "disabled"

  run pitchfork enable toggle_test
  assert_success

  run pitchfork list
  assert_output --partial "toggle_test"
  refute_output --partial "disabled"

  pitchfork stop toggle_test
}
