#!/usr/bin/env bats

setup() {
  export PITCHFORK_INTERVAL=2s
  load test_helper/common_setup
  _common_setup
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
  skip_on_windows "sysinfo memory sampling is unreliable on Windows CI"
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

@test "start reports failure when the interval watcher retries during backoff" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  # Each attempt takes 5s to fail, short of the 10s ready delay, so no attempt
  # ever reports ready. The backoff before the fourth attempt is 4s, longer
  # than the 2s interval this file runs with, so the interval watcher's retry
  # check runs while the foreground start is still sleeping, and the attempt it
  # would start is still alive when the start wakes up. If the watcher is
  # allowed to take over, the start finds a process it does not own and exits 0
  # for a daemon that is about to fail.
  create_pitchfork_toml <<EOF
[daemons.always_fails]
run = "bash $fail_script 5"
retry = 3
ready_delay = 10
EOF

  run pitchfork start always_fails
  assert_failure

  run pitchfork status always_fails
  assert_output --partial "errored"

  # Four attempts: the original and the three retries, no extras from the
  # watcher.
  run pitchfork logs always_fails --raw
  local count
  count=$(grep -c "Failed after 5!" <<< "$output" || true)
  [[ $count -eq 4 ]]
}

@test "stop ends retries the interval watcher owns" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  # The daemon becomes ready and only then fails, so the start returns and the
  # interval watcher is the one carrying the retries. A stop has to end those
  # too: there is no foreground loop to tell, only an errored record the
  # watcher would pick back up.
  create_pitchfork_toml <<EOF
[daemons.watcher_retry]
run = "bash $fail_script 3"
retry = 5
ready_delay = 1
EOF

  run pitchfork start watcher_retry
  assert_success

  wait_for_logs watcher_retry "Failed after 3!" 15
  run pitchfork stop watcher_retry
  assert_success

  local before
  before=$(pitchfork logs watcher_retry --raw 2>/dev/null | grep -c "Failed after 3!" || true)

  # Several watcher ticks and a backoff later, nothing new can have started.
  sleep 10

  local after
  after=$(pitchfork logs watcher_retry --raw 2>/dev/null | grep -c "Failed after 3!" || true)
  [[ "$after" -eq "$before" ]]

  run pitchfork status watcher_retry
  refute_output --partial "running"
}
