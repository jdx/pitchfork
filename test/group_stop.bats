#!/usr/bin/env bats

# E2E tests for whole-process-group stops and the paths that depend on their
# timing: stop() waiting for every group member to exit (escalating to SIGKILL
# for members that ignore the stop signal), starts serializing behind an
# in-flight slow stop instead of racing the widened Stopping window, and
# parallel multi-daemon start/stop attributing IPC responses to the right
# daemon task.

setup() {
  load test_helper/common_setup
  bats_require_minimum_version 1.5.0
  _common_setup
}

teardown() {
  _common_teardown
}

@test "stop returns only after TERM-ignoring child processes are gone" {
  skip_on_windows "POSIX signals and process groups are not supported on Windows"

  local child_pid_file="$TEST_TEMP_DIR/child.pid"
  local child_script="$TEST_TEMP_DIR/stubborn_child.sh"
  local daemon_script="$TEST_TEMP_DIR/parent.sh"
  cat > "$child_script" <<EOF
#!/bin/sh
trap '' TERM
echo \$\$ > "$child_pid_file"
sleep 60
EOF
  cat > "$daemon_script" <<EOF
#!/bin/sh
sh "$child_script" &
wait
EOF

  create_pitchfork_toml <<EOF
[daemons.group_stop_test]
run = "sh $daemon_script"
stop_signal = { signal = "SIGTERM", timeout = "2s" }
ready_delay = 1
EOF

  run pitchfork start group_stop_test
  assert_success
  wait_for_status group_stop_test running
  wait_for_file "$child_pid_file"
  local child_pid
  child_pid="$(cat "$child_pid_file")"
  pid_alive "$child_pid"

  # The child ignores SIGTERM, so stop must escalate to SIGKILL after the 2s
  # stop timeout and wait for the entire process group — by the time stop
  # returns, the child may only linger as an unreaped zombie, never as a
  # running process.
  run pitchfork stop group_stop_test
  assert_success

  local gone=1
  for _ in $(seq 1 10); do
    if ! pid_alive "$child_pid"; then
      gone=0
      break
    fi
    # ps state Z means the process is dead and merely awaiting reaping by init
    if [[ "$(ps -o stat= -p "$child_pid" 2>/dev/null)" == Z* ]]; then
      gone=0
      break
    fi
    sleep 0.1
  done
  [[ "$gone" -eq 0 ]]

  wait_for_status group_stop_test stopped
}

@test "start during an in-flight slow stop waits it out instead of racing it" {
  skip_on_windows "POSIX signals are not supported on Windows"

  local marker_file="$TEST_TEMP_DIR/instances.log"
  local daemon_script="$TEST_TEMP_DIR/slow_stop.sh"
  cat > "$daemon_script" <<EOF
#!/bin/sh
echo "started:\$\$" >> "$marker_file"
trap 'sleep 2; exit 0' TERM
while true; do sleep 0.1; done
EOF

  create_pitchfork_toml <<EOF
[daemons.slow_stop_test]
run = "sh $daemon_script"
ready_delay = 1
EOF

  run pitchfork start slow_stop_test
  assert_success
  wait_for_status slow_stop_test running
  local old_pid
  old_pid="$(get_daemon_pid slow_stop_test)"

  # Kick off a stop that takes ~2s (the daemon delays its TERM exit), then
  # immediately request a start. The start must serialize behind the
  # in-flight stop — waiting out the Stopping window rather than observing it
  # as "not running" and spawning a duplicate instance.
  pitchfork stop slow_stop_test &
  local stop_job=$!
  sleep 0.3
  run pitchfork start slow_stop_test
  assert_success
  wait "$stop_job"

  wait_for_status slow_stop_test running
  local new_pid
  new_pid="$(get_daemon_pid slow_stop_test)"
  [[ -n "$new_pid" ]]
  [[ "$new_pid" != "$old_pid" ]]
  # `run !` rather than a bare `! pid_alive`: negated commands are excluded
  # from errexit, so a bare form could never fail the test.
  run ! pid_alive "$old_pid"

  # Exactly two instances ever ran: the original and the one started after
  # the stop completed. A third line would mean the start raced the stop.
  local count
  count="$(grep -c '^started:' "$marker_file")"
  [[ "$count" -eq 2 ]]
}

@test "parallel multi-daemon start attributes readiness to the right daemon" {
  # Staggered readiness makes the per-daemon IPC responses arrive interleaved,
  # which is exactly the case that requires each parallel start task to read
  # from its own connection rather than picking up a sibling's response.
  create_pitchfork_toml <<EOF
[daemons.par_a]
run = "sleep 2 && echo a_ready && sleep 60"
ready_output = "a_ready"

[daemons.par_b]
run = "sleep 1 && echo b_ready && sleep 60"
ready_output = "b_ready"

[daemons.par_c]
run = "echo c_ready && sleep 60"
ready_output = "c_ready"
EOF

  run pitchfork start par_a par_b par_c
  assert_success

  wait_for_status par_a running
  wait_for_status par_b running
  wait_for_status par_c running

  run pitchfork stop par_a par_b par_c
  assert_success

  wait_for_status par_a stopped
  wait_for_status par_b stopped
  wait_for_status par_c stopped
}
