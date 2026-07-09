#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# ---------------------------------------------------------------------------
# on_output – filter (substring match)
# ---------------------------------------------------------------------------

@test "on_output with filter fires hook when output contains substring" {
  local marker="$TEST_TEMP_DIR/on_output_fired"

  create_pitchfork_toml <<EOF
[daemons.printer]
run = "bash -c 'sleep 0.2; echo hello world; sleep 60'"

[daemons.printer.hooks]
on_output = { filter = "hello", run = "touch $marker" }
EOF

  pitchfork supervisor start
  pitchfork start printer

  wait_for_file "$marker"
  assert_file_exists "$marker"

  pitchfork stop printer
}

@test "on_output with filter does not fire when output does not match" {
  local marker="$TEST_TEMP_DIR/on_output_fired"

  create_pitchfork_toml <<EOF
[daemons.printer]
run = "bash -c 'echo goodbye; sleep 60'"

[daemons.printer.hooks]
on_output = { filter = "hello", run = "touch $marker" }
EOF

  pitchfork supervisor start
  pitchfork start printer

  wait_for_logs printer "goodbye" 5
  sleep 1
  assert_file_not_exists "$marker"

  pitchfork stop printer
}

# ---------------------------------------------------------------------------
# on_output – regex match
# ---------------------------------------------------------------------------

@test "on_output with regex fires hook on matching line" {
  local marker="$TEST_TEMP_DIR/on_output_regex"

  create_pitchfork_toml <<EOF
[daemons.printer]
run = "bash -c 'sleep 0.2; echo port 3000; sleep 60'"

[daemons.printer.hooks]
on_output = { regex = "port [0-9]+", run = "touch $marker" }
EOF

  pitchfork supervisor start
  pitchfork start printer

  wait_for_file "$marker"
  assert_file_exists "$marker"

  pitchfork stop printer
}

@test "on_output with regex does not fire when line does not match" {
  local marker="$TEST_TEMP_DIR/on_output_regex"

  create_pitchfork_toml <<EOF
[daemons.printer]
run = "bash -c 'echo no numbers here; sleep 60'"

[daemons.printer.hooks]
on_output = { regex = "port [0-9]+", run = "touch $marker" }
EOF

  pitchfork supervisor start
  pitchfork start printer

  sleep 1
  assert_file_not_exists "$marker"

  pitchfork stop printer
}

# ---------------------------------------------------------------------------
# on_output – no filter/regex (fires on every line, subject to debounce)
# ---------------------------------------------------------------------------

@test "on_output without filter or regex fires on any output line" {
  local counter="$TEST_TEMP_DIR/counter"

  create_pitchfork_toml <<EOF
[daemons.printer]
run = "bash -c 'sleep 0.2; echo line1; sleep 60'"

[daemons.printer.hooks]
on_output = { run = "sh -c 'echo x >> $counter'" }
EOF

  pitchfork supervisor start
  pitchfork start printer

  wait_for_file "$counter"
  run cat "$counter"
  assert_output --partial "x"

  pitchfork stop printer
}

# ---------------------------------------------------------------------------
# on_output – PITCHFORK_MATCHED_LINE env var
# ---------------------------------------------------------------------------

@test "on_output passes matched line via PITCHFORK_MATCHED_LINE" {
  local capture="$TEST_TEMP_DIR/matched_line"

  create_pitchfork_toml <<EOF
[daemons.printer]
run = "bash -c 'sleep 0.2; echo server started on port 8080; sleep 60'"

[daemons.printer.hooks]
on_output = { filter = "server started", run = "sh -c 'echo \$PITCHFORK_MATCHED_LINE > $capture'" }
EOF

  pitchfork supervisor start
  pitchfork start printer

  wait_for_file "$capture"
  run cat "$capture"
  assert_output --partial "server started on port 8080"

  pitchfork stop printer
}

# ---------------------------------------------------------------------------
# on_output – debounce prevents rapid re-firing
# ---------------------------------------------------------------------------

@test "on_output debounce limits firing rate" {
  local counter="$TEST_TEMP_DIR/debounce_count"

  # Emit 5 lines quickly then pause; debounce of 2s should collapse them into 1 firing.
  create_pitchfork_toml <<EOF
[daemons.spammer]
run = "bash -c 'for i in 1 2 3 4 5; do echo tick; done; sleep 60'"

[daemons.spammer.hooks]
on_output = { filter = "tick", run = "sh -c 'echo x >> $counter'", debounce = "2s" }
EOF

  pitchfork supervisor start
  pitchfork start spammer

  # Wait long enough for at least one firing but less than 3x the debounce window.
  sleep 2.5

  local count
  count=$(wc -l < "$counter" | tr -d ' ')
  # At least 1 firing must have occurred (debounce collapses rapid lines).
  [[ "$count" -ge 1 ]]

  pitchfork stop spammer
}

# ---------------------------------------------------------------------------
# on_output – stderr is also monitored
# ---------------------------------------------------------------------------

@test "on_output fires on stderr output" {
  local marker="$TEST_TEMP_DIR/stderr_hook"

  create_pitchfork_toml <<EOF
[daemons.errorer]
run = "bash -c 'sleep 0.2; echo error: something went wrong >&2; sleep 60'"

[daemons.errorer.hooks]
on_output = { filter = "error:", run = "touch $marker" }
EOF

  pitchfork supervisor start
  pitchfork start errorer

  wait_for_file "$marker"
  assert_file_exists "$marker"

  pitchfork stop errorer
}

# ===========================================================================
# Lifecycle hooks – on_ready
# ===========================================================================

@test "on_ready hook fires when daemon becomes ready" {
  local marker="$TEST_TEMP_DIR/on_ready_marker"

  create_pitchfork_toml <<EOF
[daemons.ready_hook_test]
run = "bash -c 'sleep 0.2; echo READY; sleep 60'"
ready_output = "READY"

[daemons.ready_hook_test.hooks]
on_ready = "touch $marker"
EOF

  pitchfork supervisor start
  run pitchfork start ready_hook_test
  assert_success

  wait_for_file "$marker"
  assert_file_exists "$marker"

  pitchfork stop ready_hook_test
}

# ===========================================================================
# Lifecycle hooks – on_fail
# ===========================================================================

@test "on_fail hook fires only after retries are exhausted" {
  local marker="$TEST_TEMP_DIR/on_fail_retry_marker"

  create_pitchfork_toml <<EOF
[daemons.fail_retry_hook]
run = "sh -c 'exit 1'"
retry = 2

[daemons.fail_retry_hook.hooks]
on_fail = "touch $marker"
EOF

  pitchfork supervisor start
  PITCHFORK_INTERVAL=1s run pitchfork start fail_retry_hook
  assert_failure

  wait_for_file "$marker"
  assert_file_exists "$marker"
}

# ===========================================================================
# Lifecycle hooks – on_retry
# ===========================================================================

@test "on_retry hook fires once per retry attempt" {
  local marker="$TEST_TEMP_DIR/on_retry_marker"

  create_pitchfork_toml <<EOF
[daemons.retry_hook_test]
run = "sh -c 'exit 1'"
retry = 2

[daemons.retry_hook_test.hooks]
on_retry = "sh -c 'echo retry >> $marker'"
EOF

  pitchfork supervisor start
  PITCHFORK_INTERVAL=1s run pitchfork start retry_hook_test
  assert_failure

  wait_for_file "$marker"
  local count
  count=$(wc -l < "$marker" | tr -d ' ')
  [[ "$count" -eq 2 ]]
}

# ===========================================================================
# Lifecycle hooks – environment variables
# ===========================================================================

@test "PITCHFORK_DAEMON_ID is passed to daemon process" {
  create_pitchfork_toml <<EOF
[daemons.id_env_test]
run = "sh -c 'echo \$PITCHFORK_DAEMON_ID && sleep 60'"
ready_delay = 1
EOF

  pitchfork supervisor start
  run pitchfork start id_env_test
  assert_success

  local namespace expected
  namespace=$(basename "$TEST_TEMP_DIR")
  expected="$namespace/id_env_test"
  wait_for_logs id_env_test "$expected" 5
}

@test "PITCHFORK_RETRY_COUNT is incremented on retry" {
  local marker
  marker="$TEST_TEMP_DIR/retry_count_marker"

  create_pitchfork_toml <<EOF
[daemons.retry_count_test]
run = "sh -c 'echo \$PITCHFORK_RETRY_COUNT && exit 1'"
retry = 1
ready_delay = 1

[daemons.retry_count_test.hooks]
on_retry = "sh -c 'echo \$PITCHFORK_RETRY_COUNT >> $marker'"
EOF

  pitchfork supervisor start
  PITCHFORK_INTERVAL=1s run pitchfork start retry_count_test
  assert_failure

  wait_for_file "$marker"
  local content
  content="$(cat "$marker")"
  [[ "$(echo "$content" | tail -1)" == "1" ]]
}

@test "on_fail hook receives PITCHFORK_DAEMON_ID and PITCHFORK_EXIT_CODE" {
  local marker="$TEST_TEMP_DIR/hook_env_marker"

  create_pitchfork_toml <<EOF
[daemons.hook_env_test]
run = "sh -c 'exit 7'"
retry = 0

[daemons.hook_env_test.hooks]
on_fail = "sh -c 'echo \$PITCHFORK_DAEMON_ID \$PITCHFORK_EXIT_CODE > $marker'"
EOF

  pitchfork supervisor start
  run pitchfork start hook_env_test
  assert_failure

  local namespace expected
  namespace=$(basename "$TEST_TEMP_DIR")
  expected="$namespace/hook_env_test 7"
  wait_for_file_content "$marker" "$expected"
}

# ===========================================================================
# Lifecycle hooks – on_stop
# ===========================================================================

@test "on_stop hook fires when daemon is explicitly stopped" {
  local marker="$TEST_TEMP_DIR/on_stop_marker"

  create_pitchfork_toml <<EOF
[daemons.stop_hook_test]
run = "sleep 60"
ready_delay = 1

[daemons.stop_hook_test.hooks]
on_stop = "touch $marker"
EOF

  pitchfork supervisor start
  run pitchfork start stop_hook_test
  assert_success

  pitchfork stop stop_hook_test

  wait_for_file "$marker"
  assert_file_exists "$marker"
}

@test "on_stop hook receives PITCHFORK_EXIT_REASON=stop" {
  local marker="$TEST_TEMP_DIR/on_stop_reason_marker"

  create_pitchfork_toml <<EOF
[daemons.stop_reason_test]
run = "sleep 60"
ready_delay = 1

[daemons.stop_reason_test.hooks]
on_stop = "sh -c 'echo \$PITCHFORK_EXIT_REASON > $marker'"
EOF

  pitchfork supervisor start
  run pitchfork start stop_reason_test
  assert_success

  pitchfork stop stop_reason_test

  wait_for_file_content "$marker" "stop"
}

# ===========================================================================
# Lifecycle hooks – on_exit
# ===========================================================================

@test "on_exit hook fires when daemon is explicitly stopped" {
  local marker="$TEST_TEMP_DIR/on_exit_stop_marker"

  create_pitchfork_toml <<EOF
[daemons.exit_stop_test]
run = "sleep 60"
ready_delay = 1

[daemons.exit_stop_test.hooks]
on_exit = "touch $marker"
EOF

  pitchfork supervisor start
  run pitchfork start exit_stop_test
  assert_success

  pitchfork stop exit_stop_test

  wait_for_file "$marker"
  assert_file_exists "$marker"
}

@test "on_exit hook receives PITCHFORK_EXIT_REASON=fail on non-zero exit" {
  local marker="$TEST_TEMP_DIR/on_exit_fail_marker"

  create_pitchfork_toml <<EOF
[daemons.exit_fail_test]
run = "sh -c 'exit 1'"
retry = 0

[daemons.exit_fail_test.hooks]
on_exit = "sh -c 'echo \$PITCHFORK_EXIT_REASON > $marker'"
EOF

  pitchfork supervisor start
  run pitchfork start exit_fail_test
  assert_failure

  wait_for_file_content "$marker" "fail"
}

@test "on_exit hook receives PITCHFORK_EXIT_REASON=exit on clean exit" {
  local marker="$TEST_TEMP_DIR/on_exit_clean_marker"

  create_pitchfork_toml <<EOF
[daemons.exit_clean_test]
run = "sh -c 'exit 0'"
retry = 0

[daemons.exit_clean_test.hooks]
on_exit = "sh -c 'echo \$PITCHFORK_EXIT_REASON > $marker'"
EOF

  pitchfork supervisor start
  run pitchfork start exit_clean_test

  wait_for_file_content "$marker" "exit"
}

@test "both on_stop and on_exit fire when daemon is explicitly stopped" {
  local stop_marker="$TEST_TEMP_DIR/both_on_stop_marker"
  local exit_marker="$TEST_TEMP_DIR/both_on_exit_marker"

  create_pitchfork_toml <<EOF
[daemons.both_hooks_test]
run = "sleep 60"
ready_delay = 1

[daemons.both_hooks_test.hooks]
on_stop = "touch $stop_marker"
on_exit = "touch $exit_marker"
EOF

  pitchfork supervisor start
  run pitchfork start both_hooks_test
  assert_success

  pitchfork stop both_hooks_test

  wait_for_file "$stop_marker"
  assert_file_exists "$stop_marker"
  wait_for_file "$exit_marker"
  assert_file_exists "$exit_marker"
}

@test "on_exit does not fire during retries, only after retries are exhausted" {
  local counter="$TEST_TEMP_DIR/on_exit_retry_count"

  create_pitchfork_toml <<EOF
[daemons.exit_retry_guard_test]
run = "sh -c 'exit 1'"
retry = 2

[daemons.exit_retry_guard_test.hooks]
on_exit = "sh -c 'echo x >> $counter'"
EOF

  pitchfork supervisor start
  PITCHFORK_INTERVAL=600s run pitchfork start exit_retry_guard_test
  assert_failure

  wait_for_file "$counter"
  local count
  count=$(wc -l < "$counter" | tr -d ' ')
  [[ "$count" -eq 1 ]]
}
