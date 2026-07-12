#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
  export PITCHFORK_INTERVAL=1s
  export PITCHFORK_CRON_CHECK_INTERVAL=1s
}

teardown() {
  _common_teardown
}

# ============================================================================
# Cron retrigger tests with failing tasks
# ============================================================================

# bats test_tags=slow
@test "cron finish retrigger with failing task runs at least twice" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_finish_fail]
run = 'bash "$fail_script" 0'
retry = 0

[daemons.cron_finish_fail.cron]
schedule = "* * * * * *"
retrigger = "finish"
immediate = true
EOF

  run pitchfork start cron_finish_fail
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_finish_fail --raw 2>/dev/null | grep -c "Failed after 0!" || true)
    [[ "$count" -ge 2 ]] && break
    sleep 2
  done

  [[ "$count" -ge 2 ]]
}

# bats test_tags=slow
@test "cron always retrigger with failing task runs at least twice" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_always_fail]
run = 'bash "$fail_script" 0'
retry = 0

[daemons.cron_always_fail.cron]
schedule = "* * * * * *"
retrigger = "always"
immediate = true
EOF

  run pitchfork start cron_always_fail
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_always_fail --raw 2>/dev/null | grep -c "Failed after 0!" || true)
    [[ "$count" -ge 2 ]] && break
    sleep 2
  done

  [[ "$count" -ge 2 ]]
}

# bats test_tags=slow
@test "cron success retrigger with failing task runs only once" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_success_fail]
run = 'bash "$fail_script" 0'
retry = 0

[daemons.cron_success_fail.cron]
schedule = "* * * * * *"
retrigger = "success"
immediate = true
EOF

  run pitchfork start cron_success_fail
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_success_fail --raw 2>/dev/null | grep -c "Failed after 0!" || true)
    [[ "$count" -ge 1 ]] && break
    sleep 2
  done

  [[ "$count" -eq 1 ]]
}

# bats test_tags=slow
@test "cron fail retrigger with failing task runs at least twice" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_fail_fail]
run = 'bash "$fail_script" 0'
retry = 0

[daemons.cron_fail_fail.cron]
schedule = "* * * * * *"
retrigger = "fail"
immediate = true
EOF

  run pitchfork start cron_fail_fail
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_fail_fail --raw 2>/dev/null | grep -c "Failed after 0!" || true)
    [[ "$count" -ge 2 ]] && break
    sleep 2
  done

  [[ "$count" -ge 2 ]]
}

# ============================================================================
# Cron retrigger tests with long-running tasks
# ============================================================================

# bats test_tags=slow
@test "cron finish retrigger with long-running task starts only once" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local slowly_output_script
  slowly_output_script="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_finish_long]
run = 'bash "$slowly_output_script" 2 999'
retry = 0

[daemons.cron_finish_long.cron]
schedule = "* * * * * *"
retrigger = "finish"
immediate = true
EOF

  run pitchfork start cron_finish_long
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_finish_long --raw 2>/dev/null | grep -c "Output 1/999" || true)
    [[ "$count" -ge 1 ]] && break
    sleep 2
  done

  [[ "$count" -eq 1 ]]
}

# bats test_tags=slow
@test "cron always retrigger with long-running task restarts at least twice" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local slowly_output_script
  slowly_output_script="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_always_long]
run = 'bash "$slowly_output_script" 2 999'
retry = 0

[daemons.cron_always_long.cron]
schedule = "* * * * * *"
retrigger = "always"
immediate = true
EOF

  run pitchfork start cron_always_long
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_always_long --raw 2>/dev/null | grep -c "Output 1/999" || true)
    [[ "$count" -ge 2 ]] && break
    sleep 2
  done

  [[ "$count" -ge 2 ]]
}

# bats test_tags=slow
@test "cron success retrigger with long-running task starts only once" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local slowly_output_script
  slowly_output_script="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_success_long]
run = 'bash "$slowly_output_script" 2 999'
retry = 0

[daemons.cron_success_long.cron]
schedule = "* * * * * *"
retrigger = "success"
immediate = true
EOF

  run pitchfork start cron_success_long
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_success_long --raw 2>/dev/null | grep -c "Output 1/999" || true)
    [[ "$count" -ge 1 ]] && break
    sleep 2
  done

  [[ "$count" -eq 1 ]]
}

# bats test_tags=slow
@test "cron fail retrigger with long-running task starts only once" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local slowly_output_script
  slowly_output_script="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_fail_long]
run = 'bash "$slowly_output_script" 2 999'
retry = 0

[daemons.cron_fail_long.cron]
schedule = "* * * * * *"
retrigger = "fail"
immediate = true
EOF

  run pitchfork start cron_fail_long
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_fail_long --raw 2>/dev/null | grep -c "Output 1/999" || true)
    [[ "$count" -ge 1 ]] && break
    sleep 2
  done

  [[ "$count" -eq 1 ]]
}

# ============================================================================
# Fast cron immediate/retry tests
# ============================================================================

@test "cron immediate=false does not fire on first watcher tick" {
  create_pitchfork_toml <<EOF
[daemons.cron_no_immediate]
run = "echo fired"

[daemons.cron_no_immediate.cron]
schedule = "0 0 1 1 *"
retrigger = "always"
immediate = false
EOF

  run pitchfork start cron_no_immediate
  assert_success

  # The manual start runs the daemon once; with immediate=false the cron watcher
  # should not trigger any additional runs for this far-future schedule.
  sleep 3

  local count
  count=$(pitchfork logs cron_no_immediate --raw 2>/dev/null | grep -c "fired" || true)
  [[ "$count" -eq 1 ]]
}

@test "cron immediate=true fires on start" {
  create_pitchfork_toml <<EOF
[daemons.cron_immediate]
run = "echo immediate_fired"

[daemons.cron_immediate.cron]
schedule = "*/5 * * * * *"
retrigger = "always"
immediate = true
EOF

  run pitchfork start cron_immediate
  assert_success

  # immediate=true should trigger the cron watcher on its first check because a
  # scheduled time falls within the 10-second look-back window.
  wait_for_logs cron_immediate "immediate_fired" 5

  local count
  count=$(pitchfork logs cron_immediate --raw 2>/dev/null | grep -c "immediate_fired" || true)
  [[ "$count" -ge 2 ]]
}

@test "cron finish retrigger does not restart a running daemon" {
  create_pitchfork_toml <<EOF
[daemons.cron_finish_running]
run = "echo started && sleep 10"

[daemons.cron_finish_running.cron]
schedule = "* * * * * *"
retrigger = "finish"
immediate = true
EOF

  run pitchfork start cron_finish_running
  assert_success

  # With retrigger=finish, the daemon should not be restarted while it is still
  # running, even though the schedule fires every second.
  sleep 3

  local count
  count=$(pitchfork logs cron_finish_running --raw 2>/dev/null | grep -c "started" || true)
  [[ "$count" -eq 1 ]]
}

@test "retry=true retries indefinitely" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.retry_infinite]
run = 'bash "$fail_script" 0'
retry = true
EOF

  # Infinite retry causes the start client to block forever, so run it in the
  # background and observe several retry attempts.
  pitchfork start retry_infinite &
  local start_pid=$!
  sleep 5
  kill "$start_pid" 2>/dev/null || true
  wait "$start_pid" 2>/dev/null || true

  local count
  count=$(pitchfork logs retry_infinite --raw 2>/dev/null | grep -c "Failed after 0!" || true)
  [[ "$count" -ge 3 ]]

  # The daemon should still be retrying, not in a final stopped state.
  local status
  status=$(get_daemon_status retry_infinite || true)
  [[ "$status" != "stopped" ]]
}

@test "retry with ready_output re-checks on each attempt" {
  local success_script
  success_script="$(script_path success_on_third.sh)"
  export TEST_SUCCESS_ON_THIRD_TIMESTAMP="$BATS_TEST_NAME"

  create_pitchfork_toml <<EOF
[daemons.retry_ready_output]
run = 'bash "$success_script"'
ready_output = "READY"
retry = 2
EOF

  run pitchfork start retry_ready_output
  assert_success

  wait_for_logs retry_ready_output "Success!" 15
  wait_for_logs retry_ready_output "Attempt 3" 15

  # The daemon command exits after success, so it ends in stopped status.
  # The important verification is that start succeeded and the third attempt
  # produced the ready/success output despite the non-matching ready_output
  # pattern being rechecked on each attempt.
  wait_for_status retry_ready_output stopped
}

