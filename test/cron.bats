#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
  export PITCHFORK_INTERVAL=1s
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
    count=$(pitchfork logs cron_finish_fail --raw 2>/dev/null | grep -c "Failed after 0!")
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
    count=$(pitchfork logs cron_always_fail --raw 2>/dev/null | grep -c "Failed after 0!")
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
    count=$(pitchfork logs cron_success_fail --raw 2>/dev/null | grep -c "Failed after 0!")
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
    count=$(pitchfork logs cron_fail_fail --raw 2>/dev/null | grep -c "Failed after 0!")
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
    count=$(pitchfork logs cron_finish_long --raw 2>/dev/null | grep -c "Output 1/999")
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
    count=$(pitchfork logs cron_always_long --raw 2>/dev/null | grep -c "Output 1/999")
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
    count=$(pitchfork logs cron_success_long --raw 2>/dev/null | grep -c "Output 1/999")
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
    count=$(pitchfork logs cron_fail_long --raw 2>/dev/null | grep -c "Output 1/999")
    [[ "$count" -ge 1 ]] && break
    sleep 2
  done

  [[ "$count" -eq 1 ]]
}
