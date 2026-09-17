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

@test "a dependent waits for an already in-flight oneshot" {
  create_pitchfork_toml <<TOML
[daemons.migrate]
run = "sleep 4 && echo migration done"
oneshot = true

[daemons.api]
run = "echo api started && $(default_shell_sleep_command)"
depends = ["migrate"]
ready_delay = 1
TOML

  # Get the oneshot under way on its own, then ask for a dependent. The
  # dependent must wait for the run already in flight rather than treating a
  # running oneshot as satisfied.
  pitchfork start migrate >/dev/null 2>&1 &
  local migrate_job=$!
  wait_for_status migrate running 10

  run pitchfork start api
  assert_success

  # Assert before reaping the background start: waiting on it first would let
  # migrate finish on its own and the assertion could never fail. Had the
  # in-flight run been skipped, api would be up after its 1s ready_delay with
  # migrate still sleeping.
  run pitchfork status migrate
  assert_output --partial "completed"

  wait "$migrate_job" || true

  run pitchfork logs api --raw
  assert_output --partial "api started"

  pitchfork stop --all
}

@test "a listening oneshot is not marked ready by the implicit port check" {
  local bind_script
  bind_script="$(script_path bind_then_exit.py)"

  # An expected port normally becomes an implicit TCP readiness check. This
  # task binds that port and holds it for several seconds before exiting 0, so
  # an implicit check would report it ready long before its work is done.
  create_pitchfork_toml <<TOML
[daemons.migrate]
run = 'python3 $bind_script 18233 4'
oneshot = true
port = { expect = [18233] }
TOML

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start migrate
  elapsed=$(($(date +%s) - start_time))
  assert_success

  # Readiness is the exit, not the socket: the port is listening almost
  # immediately, so an implicit check would return well under the 4s runtime.
  [[ $elapsed -ge 3 ]]

  run pitchfork status migrate
  assert_output --partial "completed"

  wait_for_logs migrate "task done" 5
}

@test "a stopped oneshot that exits 0 is not reported as ready" {
  # The trap has to live in the daemon's own process. A script run through a
  # second shell would leave the daemon process itself dying from the signal,
  # which is the ordinary case and not the one under test.
  #
  # The trap records that it ran by writing a file rather than by printing:
  # a line printed on the way out races the pipe teardown that follows the
  # process group's exit, and is not reliably in the log store afterwards.
  create_pitchfork_toml <<TOML
[daemons.slow]
run = 'trap "echo yes > trapped.txt; exit 0" TERM; echo task started; while true; do sleep 0.2; done'
oneshot = true
TOML

  pitchfork start slow >/dev/null 2>&1 &
  local start_job=$!
  wait_for_logs slow "task started" 10

  run pitchfork stop slow
  assert_success

  # stop waits for the whole process group, so the trap has already run and
  # the daemon exited 0 rather than dying from the signal.
  assert_file_exists "$TEST_TEMP_DIR/trapped.txt"

  # It was interrupted rather than finished, so the waiting start must report
  # failure instead of telling dependents to proceed.
  local start_status=0
  wait "$start_job" || start_status=$?
  [[ $start_status -ne 0 ]]

  run pitchfork status slow
  assert_output --partial "stopped"
  refute_output --partial "completed"
}

@test "waiting on an in-flight oneshot survives its retry backoff" {
  local retry_script
  retry_script="$(script_path fail_then_succeed.sh)"
  local key
  key="$(date +%s%N)"

  # The task runs for a few seconds before failing, so the dependent below is
  # already waiting on the running daemon when the attempt fails and the
  # backoff begins. That gap is persisted as errored but is not the result.
  create_pitchfork_toml <<TOML
[daemons.migrate]
run = 'bash $retry_script $key 3'
oneshot = true
retry = 2

[daemons.api]
run = "echo api started && $(default_shell_sleep_command)"
depends = ["migrate"]
ready_delay = 1
TOML

  pitchfork start migrate >/dev/null 2>&1 &
  local migrate_job=$!
  wait_for_logs migrate "attempt 1 started" 10

  run pitchfork start api
  assert_success

  run pitchfork status migrate
  assert_output --partial "completed"

  wait "$migrate_job" || true

  run pitchfork logs migrate --raw
  assert_output --partial "failing attempt 1"
  assert_output --partial "succeeded on attempt 2"

  run pitchfork logs api --raw
  assert_output --partial "api started"

  pitchfork stop --all
}
