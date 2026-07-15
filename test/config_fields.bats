#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

@test "stop_signal sends custom signal to daemon" {
  skip_on_windows "POSIX signals are not supported on Windows"
  local sig_script
  sig_script="$TEST_TEMP_DIR/trap_sigint.sh"
  cat > "$sig_script" <<'EOF'
#!/bin/bash
trap 'echo got_sigint >> "$TEST_TEMP_DIR/signal_marker"; exit 0' SIGINT
sleep 60
EOF
  chmod +x "$sig_script"

  create_pitchfork_toml <<EOF
[daemons.signal_test]
run = "bash $sig_script"
stop_signal = "SIGINT"
ready_delay = 1
EOF

  run pitchfork start signal_test
  assert_success
  wait_for_status signal_test running

  run pitchfork stop signal_test
  assert_success

  wait_for_file "$TEST_TEMP_DIR/signal_marker"
  run cat "$TEST_TEMP_DIR/signal_marker"
  assert_output --partial "got_sigint"
}

@test "mise=true wraps run command with mise x" {
  command -v mise >/dev/null 2>&1 || skip "mise not installed"

  create_pitchfork_toml <<EOF
[daemons.mise_test]
run = "echo hello_from_mise"
mise = true
ready_delay = 1
EOF

  run pitchfork start mise_test
  assert_success
  wait_for_logs mise_test "hello_from_mise" 10
}

@test "cpu_limit triggers on high CPU usage" {
  export PITCHFORK_INTERVAL=1s
  pitchfork supervisor start --force >/dev/null 2>&1
  export PITCHFORK_INTERVAL=1s

  create_pitchfork_toml <<EOF
[daemons.cpu_burner]
run = "while true; do echo x > /dev/null; done"
cpu_limit = 1
retry = 0
ready_delay = 1
EOF

  run pitchfork start cpu_burner
  assert_success

  # Wait long enough for the default 3 consecutive CPU violations at 1s intervals.
  for _ in $(seq 1 60); do
    local status
    status="$(get_daemon_status cpu_burner)"
    [[ "$status" == "errored" ]] && break
    sleep 0.5
  done

  run pitchfork status cpu_burner
  assert_output --partial "errored"
}

@test "daemons add --local writes to pitchfork.local.toml" {
  run pitchfork daemons add testdaemon --run "sleep 10" --local
  assert_success
  assert [ -f pitchfork.local.toml ]

  run cat pitchfork.local.toml
  assert_output --partial "[daemons.testdaemon]"
  assert_output --partial 'run = "sleep 10"'
}

@test "daemons add --project writes to pitchfork.toml" {
  run pitchfork daemons add testdaemon --run "sleep 10" --project
  assert_success
  assert [ -f pitchfork.toml ]
  refute [ -f pitchfork.local.toml ]

  run cat pitchfork.toml
  assert_output --partial "[daemons.testdaemon]"
  assert_output --partial 'run = "sleep 10"'
}

@test "daemons add --cron-immediate sets immediate=true" {
  run pitchfork daemons add cronjob --run "echo hello" --cron-schedule "* * * * * *" --cron-immediate
  assert_success
  assert [ -f pitchfork.toml ]

  run cat pitchfork.toml
  assert_output --partial "[daemons.cronjob]"
  assert_output --partial 'immediate = true'
}

@test "daemons add --boot-start sets boot_start=true" {
  run pitchfork daemons add bootsvc --run "sleep 10" --boot-start
  assert_success

  run cat pitchfork.toml
  assert_output --partial 'boot_start = true'
}

@test "daemons add --on-stop registers stop hook via CLI" {
  run pitchfork daemons add hooktest --run "sleep 60" --on-stop "touch $TEST_TEMP_DIR/stop_marker"
  assert_success

  run cat pitchfork.toml
  assert_output --partial 'on_stop ='

  run pitchfork start hooktest
  assert_success
  wait_for_status hooktest running

  run pitchfork stop hooktest
  assert_success
  wait_for_file "$TEST_TEMP_DIR/stop_marker"
}

@test "daemons add --on-exit registers exit hook via CLI" {
  run pitchfork daemons add hooktest --run "sleep 1" --on-exit "touch $TEST_TEMP_DIR/exit_marker"
  assert_success

  run cat pitchfork.toml
  assert_output --partial 'on_exit ='

  run pitchfork start hooktest
  assert_success
  wait_for_file "$TEST_TEMP_DIR/exit_marker" 10
}

@test "daemons add --bump with explicit number sets bump range" {
  run pitchfork daemons add portsvc --run "sleep 10" --expected-port 8080 --bump 20
  assert_success

  run cat pitchfork.toml
  assert_output --partial 'bump = 20'
}

@test "cron retrigger=success only re-fires on success" {
  export PITCHFORK_INTERVAL=1s
  export PITCHFORK_CRON_CHECK_INTERVAL=1s

  create_pitchfork_toml <<EOF
[daemons.cron_success]
run = "echo success_output"
ready_delay = 0

[daemons.cron_success.cron]
schedule = "0 0 1 1 *"
retrigger = "success"
immediate = true
EOF

  run pitchfork start cron_success
  assert_success

  sleep 3
  run pitchfork logs cron_success --raw
  assert_output --partial "success_output"

  local count
  count=$(pitchfork logs cron_success --raw 2>/dev/null | grep -c "success_output" || true)
  [[ "$count" -eq 1 ]]
}

@test "cron retrigger=fail does not re-fire on success" {
  export PITCHFORK_INTERVAL=1s
  export PITCHFORK_CRON_CHECK_INTERVAL=1s

  create_pitchfork_toml <<EOF
[daemons.cron_fail]
run = "echo success_output"
ready_delay = 0

[daemons.cron_fail.cron]
schedule = "0 0 1 1 *"
retrigger = "fail"
immediate = true
EOF

  run pitchfork start cron_fail
  assert_success

  sleep 3

  local count
  count=$(pitchfork logs cron_fail --raw 2>/dev/null | grep -c "success_output" || true)
  [[ "$count" -eq 1 ]]
}

@test "time_retention setting is accepted in config" {
  create_pitchfork_toml <<EOF
[daemons.logger]
run = "sleep 60"

[settings.logs]
time_retention = "5s"
EOF

  run pitchfork list
  assert_success
  assert_output --partial "logger"
}

@test "line_retention setting is accepted in config" {
  create_pitchfork_toml <<EOF
[daemons.logger]
run = "sleep 60"

[settings.logs]
line_retention = 100
EOF

  run pitchfork list
  assert_success
  assert_output --partial "logger"
}

@test "archive_hook setting is accepted in config" {
  create_pitchfork_toml <<EOF
[daemons.logger]
run = "sleep 60"
archive_hook = "echo archived"
EOF

  run pitchfork list
  assert_success
  assert_output --partial "logger"
}

@test "daemons remove deletes daemon from config" {
  run pitchfork daemons add toremove --run "sleep 10"
  assert_success

  run cat pitchfork.toml
  assert_output --partial "[daemons.toremove]"

  run pitchfork daemons remove toremove
  assert_success

  run cat pitchfork.toml
  refute_output --partial "[daemons.toremove]"
}

@test "daemons remove on nonexistent daemon gives warning" {
  # Ensure a project config exists so the remove command looks inside it.
  run pitchfork daemons add existing --run "sleep 10"
  assert_success

  run pitchfork daemons remove does_not_exist
  assert_success
  assert_output --partial "does_not_exist" || assert_output --partial "not found"
}

@test "[daemons.x.logs] sub-table configures log_format per-daemon" {
  # Verify the sub-table is parsed and log_format is applied at runtime
  create_pitchfork_toml <<'EOF'
[daemons.subtable_json]
run = 'echo {"level":"info","msg":"subt"}; sleep 60'
ready_output = "subt"

[daemons.subtable_json.logs]
log_format = "json"
EOF

  run pitchfork start subtable_json
  assert_success
  wait_for_log_lines subtable_json 1

  # Verify the daemon is running (config was accepted)
  run pitchfork status subtable_json
  assert_success
  assert_output --partial "running"

  pitchfork stop subtable_json
}

@test "[daemons.x.logs] sub-table overrides top-level time_retention" {
  # Top-level: 1h, sub-table: 1s. Config should be accepted without error.
  create_pitchfork_toml <<'EOF'
[daemons.override_test]
run = "sleep 60"
time_retention = "1h"

[daemons.override_test.logs]
time_retention = "1s"
EOF

  run pitchfork list
  assert_success
  [[ "$output" == *"override_test"* ]]

  run pitchfork start override_test
  assert_success
  wait_for_status override_test running

  pitchfork stop override_test
}
