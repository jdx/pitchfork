#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  bats_require_minimum_version 1.5.0
  _common_setup
}

teardown() {
  _common_teardown
}

@test "autostop is delayed when PITCHFORK_AUTOSTOP_DELAY is set" {
  skip_on_windows "shell PID liveness check is skipped on Windows, autostop timing unreliable"
  export PITCHFORK_AUTOSTOP_DELAY=5s
  export PITCHFORK_INTERVAL=2s

  # Restart supervisor to pick up the test-specific interval/delay settings
  pitchfork supervisor start --force >/dev/null 2>&1

  create_pitchfork_toml <<EOF
namespace = "project"

[daemons.delayed_stop]
run = "sleep 120"
auto = ["stop"]
ready_delay = 1
EOF

  local other_dir
  other_dir="$(mktemp -d /tmp/pf-autostop-other-XXXXXX)"

  run pitchfork cd --shell-pid $$
  assert_success

  run pitchfork start delayed_stop --shell-pid $$
  assert_success

  wait_for_status delayed_stop running

  cd "$other_dir"
  run pitchfork cd --shell-pid $$
  assert_success

  sleep 1
  run pitchfork status project/delayed_stop
  assert_success
  assert_output --partial "running"

  sleep 10

  run pitchfork list
  assert_success
  sleep 1

  run pitchfork status project/delayed_stop
  refute_output --partial "running"
}

@test "returning to project dir cancels pending autostop" {
  export PITCHFORK_AUTOSTOP_DELAY=5s
  export PITCHFORK_INTERVAL=2s

  create_pitchfork_toml <<EOF
namespace = "project"

[daemons.cancel_stop]
run = "sleep 120"
auto = ["stop"]
ready_delay = 1
EOF

  local other_dir
  other_dir="$(mktemp -d /tmp/pf-autostop-other-XXXXXX)"

  run pitchfork cd --shell-pid $$
  assert_success

  run pitchfork start cancel_stop --shell-pid $$
  assert_success

  wait_for_status cancel_stop running

  cd "$other_dir"
  run pitchfork cd --shell-pid $$
  assert_success

  sleep 2
  run pitchfork status project/cancel_stop
  assert_success
  assert_output --partial "running"

  cd "$TEST_TEMP_DIR"
  run pitchfork cd --shell-pid $$
  assert_success

  sleep 10

  run pitchfork list
  assert_success
  sleep 1

  run pitchfork status project/cancel_stop
  assert_success
  assert_output --partial "running"
}

@test "autostop happens immediately when delay is zero" {
  export PITCHFORK_AUTOSTOP_DELAY=0s
  export PITCHFORK_INTERVAL=2s

  # Restart supervisor to pick up the test-specific interval/delay settings
  pitchfork supervisor start --force >/dev/null 2>&1

  create_pitchfork_toml <<EOF
namespace = "project"

[daemons.immediate_stop]
run = "sleep 120"
auto = ["stop"]
ready_delay = 1
EOF

  local other_dir
  other_dir="$(mktemp -d /tmp/pf-autostop-other-XXXXXX)"

  run pitchfork cd --shell-pid $$
  assert_success

  run pitchfork start immediate_stop --shell-pid $$
  assert_success

  wait_for_status immediate_stop running

  cd "$other_dir"
  run pitchfork cd --shell-pid $$
  assert_success

  sleep 2

  run pitchfork list
  assert_success
  sleep 1

  run pitchfork status project/immediate_stop
  refute_output --partial "running"
}

@test "a daemon restarted after auto = stop is removed from config is not autostopped" {
  export PITCHFORK_AUTOSTOP_DELAY=0s
  export PITCHFORK_INTERVAL=2s

  # Restart supervisor to pick up the test-specific interval/delay settings
  pitchfork supervisor start --force >/dev/null 2>&1

  create_pitchfork_toml <<EOF2
namespace = "project"

[daemons.no_longer_stop]
run = "sleep 120"
auto = ["stop"]
ready_delay = 1
EOF2

  run pitchfork cd --shell-pid $$
  assert_success

  run pitchfork start no_longer_stop --shell-pid $$
  assert_success
  run pitchfork stop no_longer_stop
  assert_success

  # The daemon no longer asks to be stopped when its directory is left.
  create_pitchfork_toml <<EOF2
namespace = "project"

[daemons.no_longer_stop]
run = "sleep 120"
ready_delay = 1
EOF2

  run pitchfork start no_longer_stop --shell-pid $$
  assert_success
  wait_for_status no_longer_stop running

  local other_dir
  other_dir="$(mktemp -d "$BATS_TEST_TMPDIR/other-XXXXXX")"
  cd "$other_dir"
  run pitchfork cd --shell-pid $$
  assert_success

  sleep 3

  run pitchfork status project/no_longer_stop
  assert_output --partial "running"
}

@test "an ad-hoc run under the id of an auto = stop daemon is not autostopped" {
  export PITCHFORK_AUTOSTOP_DELAY=0s
  export PITCHFORK_INTERVAL=2s

  # Restart supervisor to pick up the test-specific interval/delay settings
  pitchfork supervisor start --force >/dev/null 2>&1

  create_pitchfork_toml <<EOF2
namespace = "project"

[daemons.adhoc_after_stop]
run = "sleep 120"
auto = ["stop"]
ready_delay = 1
EOF2

  run pitchfork cd --shell-pid $$
  assert_success

  run pitchfork start adhoc_after_stop --shell-pid $$
  assert_success
  run pitchfork stop adhoc_after_stop
  assert_success

  # An ad-hoc command is never autostopped, whatever the config daemon of the
  # same id asked for.
  run pitchfork run adhoc_after_stop --delay 1 -- sleep 121
  assert_success
  wait_for_status adhoc_after_stop running

  local other_dir
  other_dir="$(mktemp -d "$BATS_TEST_TMPDIR/other-XXXXXX")"
  cd "$other_dir"
  run pitchfork cd --shell-pid $$
  assert_success

  sleep 3

  run pitchfork status project/adhoc_after_stop
  assert_output --partial "running"
}
