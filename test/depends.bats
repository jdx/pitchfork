#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

@test "start with dependency auto-starts dependency" {
  create_pitchfork_toml <<EOF
[daemons.db]
run = "echo db ready && sleep 30"
ready_delay = 1

[daemons.api]
run = "echo api ready && sleep 30"
depends = ["db"]
ready_delay = 1
EOF

  run pitchfork start api
  assert_success

  run pitchfork list
  assert_success
  assert_output --partial "db"
  assert_output --partial "api"

  wait_for_logs db "db ready" 5
  wait_for_logs api "api ready" 5

  pitchfork stop --all
}

@test "dependency start order is respected" {
  create_pitchfork_toml <<EOF
[daemons.database]
run = "echo database started && sleep 30"
ready_delay = 1

[daemons.backend]
run = "echo backend started && sleep 30"
depends = ["database"]
ready_delay = 1

[daemons.api]
run = "echo api started && sleep 30"
depends = ["backend"]
ready_delay = 1
EOF

  run pitchfork start api
  assert_success

  run pitchfork list
  assert_success
  assert_output --partial "database"
  assert_output --partial "backend"
  assert_output --partial "api"

  wait_for_logs database "database started" 5
  wait_for_logs backend "backend started" 5
  wait_for_logs api "api started" 5

  pitchfork stop --all
}

@test "start --all respects dependencies" {
  create_pitchfork_toml <<EOF
[daemons.db]
run = "echo db started && sleep 30"
ready_delay = 1

[daemons.cache]
run = "echo cache started && sleep 30"
ready_delay = 1

[daemons.api]
run = "echo api started && sleep 30"
depends = ["db", "cache"]
ready_delay = 1

[daemons.worker]
run = "echo worker started && sleep 30"
depends = ["db"]
ready_delay = 1
EOF

  run pitchfork start --all
  assert_success

  run pitchfork list
  assert_success
  assert_output --partial "db"
  assert_output --partial "cache"
  assert_output --partial "api"
  assert_output --partial "worker"

  pitchfork stop --all
}

@test "already running dependency is skipped" {
  create_pitchfork_toml <<EOF
[daemons.db]
run = "echo db ready && sleep 30"
ready_delay = 1

[daemons.api]
run = "echo api ready && sleep 30"
depends = ["db"]
ready_delay = 1
EOF

  run pitchfork start db
  assert_success

  wait_for_status db running

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork start api
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -lt 3 ]]

  pitchfork stop --all
}

@test "circular dependency is rejected" {
  create_pitchfork_toml <<EOF
[daemons.a]
run = "echo a"
depends = ["c"]

[daemons.b]
run = "echo b"
depends = ["a"]

[daemons.c]
run = "echo c"
depends = ["b"]
EOF

  run pitchfork start a
  assert_failure
  [[ "${output,,}" == *"circular"* ]]
}

@test "missing dependency is rejected" {
  create_pitchfork_toml <<EOF
[daemons.api]
run = "echo api"
depends = ["nonexistent"]
EOF

  run pitchfork start api
  assert_failure
  assert_output --partial "nonexistent"
}

@test "force flag only restarts the requested daemon" {
  create_pitchfork_toml <<EOF
[daemons.db]
run = "echo db_started; sleep 60"
ready_delay = 1

[daemons.api]
run = "echo api_started; sleep 60"
depends = ["db"]
ready_delay = 1
EOF

  run pitchfork start api
  assert_success

  sleep 2

  run pitchfork list
  assert_output --partial "running"

  run read_logs db
  local count_before
  count_before=$(grep -c "db_started" <<< "$output" || true)

  run pitchfork start -f api
  assert_success

  sleep 1

  run read_logs db
  local count_after
  count_after=$(grep -c "db_started" <<< "$output" || true)
  [[ $count_before -eq $count_after ]]

  run read_logs api
  local api_count
  api_count=$(grep -c "api_started" <<< "$output" || true)
  [[ $api_count -eq 2 ]]

  pitchfork stop --all
}

@test "stop --all stops all daemons" {
  create_pitchfork_toml <<EOF
[daemons.db]
run = "echo db started && sleep 30"
ready_delay = 1

[daemons.cache]
run = "echo cache started && sleep 30"
ready_delay = 1

[daemons.api]
run = "echo api started && sleep 30"
depends = ["db", "cache"]
ready_delay = 1

[daemons.worker]
run = "echo worker started && sleep 30"
depends = ["db"]
ready_delay = 1
EOF

  run pitchfork start --all
  assert_success

  sleep 1

  run pitchfork stop --all
  assert_success

  sleep 1

  for daemon in db cache api worker; do
    run pitchfork status "$daemon"
    refute_output --partial "running"
  done
}

@test "stop --all handles partial running daemons" {
  create_pitchfork_toml <<EOF
[daemons.db]
run = "echo db started && sleep 30"
ready_delay = 1

[daemons.api]
run = "echo api started && sleep 30"
depends = ["db"]
ready_delay = 1

[daemons.worker]
run = "echo worker started && sleep 30"
ready_delay = 1
EOF

  run pitchfork start api
  assert_success

  sleep 1

  run pitchfork stop --all
  assert_success

  sleep 1

  for daemon in db api worker; do
    run pitchfork status "$daemon"
    refute_output --partial "running"
  done
}

@test "stop --all succeeds when no daemons are running" {
  create_pitchfork_toml <<EOF
[daemons.db]
run = "echo db started && sleep 30"
ready_delay = 1
EOF

  run pitchfork start db
  assert_success

  run pitchfork stop db
  assert_success

  sleep 1

  run pitchfork stop --all
  assert_success
}
