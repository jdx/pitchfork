#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# ============================================================================
# Error handling tests
# ============================================================================

@test "start nonexistent daemon gives clear error" {
  create_pitchfork_toml <<EOF
[daemons.existing_daemon]
run = "sleep 10"
EOF

  pitchfork supervisor start

  run pitchfork start does_not_exist
  assert_failure
  assert_output --partial "does_not_exist"
  assert_output --partial "not found"
}

@test "stop nonexistent daemon gives clear error" {
  create_pitchfork_toml <<EOF
[daemons.existing_daemon]
run = "sleep 10"
EOF

  pitchfork supervisor start

  run pitchfork stop does_not_exist
  assert_failure
  assert_output --partial "does_not_exist"
  assert_output --partial "not found"
}

@test "start with conflicting flags is rejected" {
  run pitchfork start --all --local some_daemon
  assert_failure
  assert_output --partial "--all"
  assert_output --partial "--local"
}

@test "start with no arguments is rejected" {
  run pitchfork start
  assert_failure
  assert_output --partial "At least one"
}

@test "logs on daemon with no logs returns empty without error" {
  create_pitchfork_toml <<EOF
[daemons.silent_daemon]
run = "sleep 10"
EOF

  # Suppress debug output so we can assert on empty stdout.
  PITCHFORK_LOG=warn run pitchfork logs silent_daemon --raw
  assert_success
  assert_output ""
}

@test "invalid pitchfork.toml syntax gives diagnostic error" {
  # Start the supervisor with a valid config first; once it is running,
  # swapping in an invalid config will surface the parse error in the CLI.
  pitchfork supervisor start
  create_pitchfork_toml <<EOF
[daemons.placeholder]
run = "sleep 1"
EOF

  echo "invalid toml [[" > pitchfork.toml

  PITCHFORK_LOG=warn run pitchfork list
  assert_failure
  assert_output --regexp 'parse|toml|invalid|TOML'
}

@test "invalid daemon name in config is rejected" {
  # Start the supervisor with a valid config first; once it is running,
  # swapping in an invalid daemon name will surface the validation error.
  pitchfork supervisor start
  create_pitchfork_toml <<EOF
[daemons.placeholder]
run = "sleep 1"
EOF

  cat > pitchfork.toml <<'EOF'
[daemons.bad--name]
run = "sleep 10"
EOF

  PITCHFORK_LOG=warn run pitchfork list
  assert_failure
  assert_output --partial "bad--name"
}

@test "supervisor stop exits cleanly with SIGTERM" {
  run pitchfork supervisor start
  assert_success

  run pitchfork supervisor status
  assert_success

  run pitchfork supervisor stop
  assert_success
}

@test "CLI works without supervisor running (auto-starts)" {
  create_pitchfork_toml <<EOF
[daemons.auto_start_test]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start auto_start_test
  assert_success

  wait_for_status auto_start_test running
}

@test "pitchfork list works with empty config" {
  PITCHFORK_LOG=warn run pitchfork list
  assert_success
  assert_output ""
}
