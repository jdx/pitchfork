#!/usr/bin/env bats

# E2E tests for the `pitchfork wait` command: waiting for one or more
# daemons to exit, propagating the last exit code, and stopping waited
# daemons on a signal with --kill.

setup() {
  load test_helper/common_setup
  bats_require_minimum_version 1.5.0
  _common_setup
}

teardown() {
  _common_teardown
}

@test "wait returns 0 after a single daemon exits cleanly" {
  create_pitchfork_toml <<EOF
[daemons.wait_single]
run = "sleep 2"
ready_delay = 0
EOF

  run pitchfork start wait_single
  assert_success
  wait_for_status wait_single running

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork wait wait_single
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -ge 2 ]]

  wait_for_status wait_single stopped
}

@test "wait returns after all daemons stop" {
  create_pitchfork_toml <<EOF
[daemons.wait_short]
run = "sleep 2"
ready_delay = 0

[daemons.wait_long]
run = "sleep 4"
ready_delay = 0
EOF

  run pitchfork start wait_short wait_long
  assert_success
  wait_for_status wait_short running
  wait_for_status wait_long running

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork wait wait_short wait_long
  elapsed=$(($(date +%s) - start_time))

  assert_success
  # `wait` must not return before the slowest daemon has stopped.
  [[ $elapsed -ge 4 ]]
  wait_for_status wait_short stopped
  wait_for_status wait_long stopped
}

@test "wait --group waits for all daemons in the group" {
  create_pitchfork_toml <<EOF
[daemons.wait_ga]
run = "sleep 3"
ready_delay = 0

[daemons.wait_gb]
run = "sleep 3"
ready_delay = 0

[groups.backend]
daemons = ["wait_ga", "wait_gb"]
EOF

  run pitchfork start wait_ga wait_gb
  assert_success
  wait_for_status wait_ga running
  wait_for_status wait_gb running

  run pitchfork wait --group backend
  assert_success

  wait_for_status wait_ga stopped
  wait_for_status wait_gb stopped
}

@test "wait propagates a non-zero daemon exit code" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.wait_fail]
run = 'bash $fail_script 2'
ready_delay = 0
EOF

  run pitchfork start wait_fail
  assert_success

  run pitchfork wait wait_fail
  assert_failure 1
}

@test "--kill stops waited daemons when a signal arrives" {
  skip_on_windows "POSIX signals are not supported on Windows"

  create_pitchfork_toml <<EOF
[daemons.wait_kill]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start wait_kill
  assert_success
  wait_for_status wait_kill running

  local daemon_pid
  daemon_pid="$(get_daemon_pid wait_kill)"
  [[ -n "$daemon_pid" ]]

  pitchfork wait --kill wait_kill >/dev/null 2>&1 &
  local wait_pid=$!
  sleep 1
  kill -INT "$wait_pid"

  # SIGINT: the wait command stops the daemon, then exits 128 + 2 = 130.
  # Reap directly instead of via `run`: `run` waits inside a command
  # substitution subshell, where bash cannot report a signal-killed job's
  # exit status (it surfaces 255 instead of 130).
  set +e
  wait "$wait_pid"
  local wait_status=$?
  set -e
  [[ $wait_status -eq 130 ]]

  wait_for_status wait_kill stopped
  run ! pid_alive "$daemon_pid"
}