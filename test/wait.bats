#!/usr/bin/env bats

# E2E tests for the `pitchfork wait` command: waiting for one or more
# daemons to exit, propagating non-zero exit codes, and stopping waited
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

  run pitchfork wait wait_single
  assert_success

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

  run pitchfork wait wait_short wait_long
  assert_success
  # `wait` must not return before the slowest daemon has stopped: by the
  # time it returns, the slowest daemon must already report stopped, without
  # any further polling from this test.
  assert_equal "$(get_daemon_status wait_long)" "stopped"
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
  create_pitchfork_toml <<EOF
[daemons.wait_fail]
run = "sleep 1 && exit 7"
ready_delay = 0
EOF

  run pitchfork start wait_fail
  assert_success
  # The daemon becomes ready immediately, then exits 7 after 1s, so the
  # supervisor records Errored(7) (not Failed) and `wait` must propagate
  # exactly that code.
  wait_for_status wait_fail running

  run pitchfork wait wait_fail
  assert_failure 7
}

@test "wait propagates an early failure even when a later daemon stops cleanly" {
  create_pitchfork_toml <<EOF
[daemons.wait_fail_early]
run = "sleep 1 && exit 7"
ready_delay = 0

[daemons.wait_clean_late]
run = "sleep 4"
ready_delay = 0
EOF

  run pitchfork start wait_fail_early wait_clean_late
  assert_success
  wait_for_status wait_fail_early running
  wait_for_status wait_clean_late running

  # 'wait_fail_early' exits 7 after 1s; 'wait_clean_late' stops cleanly
  # after 4s. The later clean stop must not mask the earlier failure:
  # 'wait' has to exit with exactly 7.
  run pitchfork wait wait_fail_early wait_clean_late
  assert_failure 7

  wait_for_status wait_fail_early errored
  wait_for_status wait_clean_late stopped
}

@test "wait propagates the first failing daemon's exit code in argument order" {
  create_pitchfork_toml <<EOF
[daemons.wait_fail_first]
run = "sleep 2 && exit 3"
ready_delay = 0

[daemons.wait_fail_second]
run = "sleep 1 && exit 7"
ready_delay = 0
EOF

  run pitchfork start wait_fail_first wait_fail_second
  assert_success
  wait_for_status wait_fail_first running
  wait_for_status wait_fail_second running

  # 'wait_fail_second' is listed second but stops first with 7; only
  # argument-order selection returns the first-listed daemon's 3, so a
  # "first to finish" rule would return 7 here and fail this test.
  run pitchfork wait wait_fail_first wait_fail_second
  assert_failure 3

  wait_for_status wait_fail_first errored
  wait_for_status wait_fail_second errored
}

@test "wait includes already-finished daemons in exit-code evaluation" {
  create_pitchfork_toml <<EOF
[daemons.wait_preexited]
run = "sleep 1 && exit 7"
ready_delay = 0

[daemons.wait_then_clean]
run = "sleep 2"
ready_delay = 0
EOF

  run pitchfork start wait_preexited wait_then_clean
  assert_success
  wait_for_status wait_preexited errored
  wait_for_status wait_then_clean running

  # 'wait_preexited' already failed with 7 before this wait runs: it is
  # evaluated immediately and must not be swallowed by 'wait_then_clean'
  # stopping cleanly.
  run pitchfork wait wait_preexited wait_then_clean
  assert_failure 7

  wait_for_status wait_then_clean stopped

  # A single already-failed daemon fails the wait on its own.
  run pitchfork wait wait_preexited
  assert_failure 7
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