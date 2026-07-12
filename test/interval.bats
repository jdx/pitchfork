#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
  export PITCHFORK_INTERVAL=2s
}

teardown() {
  _common_teardown
}

@test "interval watch long running task stays running" {
  create_pitchfork_toml <<EOF
[daemons.long_runner]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start long_runner
  assert_success

  sleep 6

  run pitchfork status long_runner
  assert_output --partial "running"

  pitchfork stop long_runner
}

@test "interval watch detects failed daemon" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.fail_after_ready]
run = "bash $fail_script 5"
EOF

  run pitchfork start fail_after_ready
  assert_success

  sleep 10

  run pitchfork status fail_after_ready
  assert_output --partial "errored"

  wait_for_logs fail_after_ready "Failed after 5!" 10

  pitchfork stop fail_after_ready
}

@test "interval watch retry on failure" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.retry_after_ready]
run = "bash $fail_script 5"
retry = 1
EOF

  run pitchfork start retry_after_ready
  assert_success

  sleep 16

  run pitchfork status retry_after_ready
  assert_output --partial "errored"

  run pitchfork logs retry_after_ready --raw
  local count
  count=$(grep -c "Failed after 5!" <<< "$output" || true)
  [[ $count -eq 2 ]]

  pitchfork stop retry_after_ready
}

@test "resource violation triggers retry" {
  local eat_memory_script
  eat_memory_script="$(script_path eat_memory.sh)"

  create_pitchfork_toml <<EOF
[daemons.mem_hog]
run = "bash $eat_memory_script 64"
memory_limit = "20MB"
retry = 1
ready_delay = 1
EOF

  run pitchfork start mem_hog
  assert_success

  local deadline
  deadline=$(($(date +%s) + 30))

  while true; do
    local status logs count
    status=$(get_daemon_status mem_hog)
    if [[ "$status" == *"errored"* ]]; then
      logs=$(read_logs mem_hog)
      count=$(grep -c "Starting memory allocation of 64MB" <<< "$logs" || true)
      if [[ $count -ge 2 ]]; then
        break
      fi
    fi
    if [[ $(date +%s) -ge $deadline ]]; then
      break
    fi
    sleep 2
  done

  run pitchfork status mem_hog
  assert_output --partial "errored"

  run pitchfork logs mem_hog --raw
  local count
  count=$(grep -c "Starting memory allocation of 64MB" <<< "$output" || true)
  [[ $count -eq 2 ]]

  pitchfork stop mem_hog
}
