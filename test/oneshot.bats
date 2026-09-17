#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

@test "oneshot daemon reports completed after a clean exit" {
  create_pitchfork_toml <<EOF
[daemons.migrate]
run = "echo migration done"
oneshot = true
EOF

  run pitchfork start migrate
  assert_success

  # start returns only once the completed state is persisted.
  run pitchfork status migrate
  assert_success
  assert_output --partial "completed"

  wait_for_logs migrate "migration done" 5
}

@test "oneshot completed status appears in list and json output" {
  create_pitchfork_toml <<EOF
[daemons.migrate]
run = "echo migration done"
oneshot = true
EOF

  run pitchfork start migrate
  assert_success

  run pitchfork list
  assert_success
  assert_output --partial "completed"

  run pitchfork list --json
  assert_success
  assert_output --partial '"status": "completed"'

  # The filter accepts the new value.
  run pitchfork list --status completed
  assert_success
  assert_output --partial "migrate"
}

@test "dependents wait for a oneshot to complete" {
  create_pitchfork_toml <<EOF
[daemons.migrate]
run = "sleep 2 && echo migration done"
oneshot = true

[daemons.api]
run = "echo api started && $(default_shell_sleep_command)"
depends = ["migrate"]
ready_delay = 1
EOF

  run pitchfork start api
  assert_success

  # The migration finished before the API was started, so its line is in the
  # log store by the time the API has produced any output at all.
  wait_for_logs migrate "migration done" 5
  wait_for_logs api "api started" 5

  run pitchfork status migrate
  assert_output --partial "completed"

  run pitchfork status api
  assert_output --partial "running"

  pitchfork stop --all
}

@test "a failing oneshot blocks its dependents" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.migrate]
run = 'bash $fail_script 0'
oneshot = true

[daemons.api]
run = "echo api started && $(default_shell_sleep_command)"
depends = ["migrate"]
ready_delay = 1
EOF

  run pitchfork start api
  assert_failure

  wait_for_logs migrate "Failed after 0!" 5
  run pitchfork status migrate
  assert_output --partial "errored"

  # The dependent must not have been started.
  run pitchfork logs api --raw
  refute_output --partial "api started"
}

@test "a failing oneshot is retried" {
  local success_script
  success_script="$(script_path success_on_third.sh)"
  export TEST_SUCCESS_ON_THIRD_TIMESTAMP="$(date +%s%N)"

  create_pitchfork_toml <<EOF
[daemons.migrate]
run = 'bash $success_script'
oneshot = true
retry = 3

[daemons.migrate.env]
TEST_SUCCESS_ON_THIRD_TIMESTAMP = "$TEST_SUCCESS_ON_THIRD_TIMESTAMP"
EOF

  run pitchfork start migrate
  assert_success

  wait_for_logs migrate "Success!" 15
  run pitchfork logs migrate --raw
  assert_output --partial "Attempt 1"
  assert_output --partial "Attempt 3"

  run pitchfork status migrate
  assert_output --partial "completed"
}

@test "restart re-runs a completed oneshot" {
  create_pitchfork_toml <<EOF
[daemons.migrate]
run = "echo migration ran"
oneshot = true
EOF

  run pitchfork start migrate
  assert_success
  wait_for_logs migrate "migration ran" 5

  run pitchfork restart migrate
  assert_success

  run pitchfork status migrate
  assert_output --partial "completed"

  # Two runs, two log lines.
  wait_for_log_lines migrate 2
  run pitchfork logs migrate --raw
  local count
  count=$(grep -c "migration ran" <<< "$output")
  [[ $count -eq 2 ]]
}

@test "start re-runs a completed oneshot" {
  create_pitchfork_toml <<EOF
[daemons.migrate]
run = "echo migration ran"
oneshot = true
EOF

  run pitchfork start migrate
  assert_success
  wait_for_logs migrate "migration ran" 5

  run pitchfork start migrate
  assert_success

  wait_for_log_lines migrate 2
  run pitchfork logs migrate --raw
  local count
  count=$(grep -c "migration ran" <<< "$output")
  [[ $count -eq 2 ]]
}

@test "stopping a running oneshot records it as stopped, not completed" {
  create_pitchfork_toml <<EOF
[daemons.slow]
run = "echo task started && $(default_shell_sleep_command)"
oneshot = true
EOF

  # A oneshot that does not exit leaves start waiting, so start it in the
  # background and stop it once it is up.
  pitchfork start slow >/dev/null 2>&1 &
  local start_job=$!

  wait_for_logs slow "task started" 10

  run pitchfork stop slow
  assert_success
  wait "$start_job" || true

  run pitchfork status slow
  assert_output --partial "stopped"
  refute_output --partial "completed"
}

@test "oneshot rejects readiness and health checks at config load" {
  create_pitchfork_toml <<EOF
[daemons.migrate]
run = "echo hi"
oneshot = true
ready_port = 8080
EOF

  run pitchfork start migrate
  assert_failure
  assert_output --partial "oneshot"
  assert_output --partial "ready_port"
}
