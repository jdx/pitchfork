#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

@test "a dependency started after the snapshot supplies its current ports" {
  local port http_script
  port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
  http_script="$(script_path http_server.py)"
  create_pitchfork_toml <<TOML
[daemons.gate]
run = "touch gate_started; while ! test -e release_gate; do sleep 0.1; done"
oneshot = true

[daemons.service]
run = "python3 -u $http_script 0 $port"
depends = ["gate"]
port = $port

[daemons.dependent]
run = "echo {{ daemons.service.port }} > dependency_port; sleep 30"
depends = ["service"]
ready_delay = 0
TOML

  pitchfork start dependent >dependent_start.log 2>&1 &
  local dependent_start=$!
  wait_for_file gate_started

  # `run` starts the dependency while the batch waits at an earlier level.
  run pitchfork run service --expected-port "$port" --port "$port" -- python3 -u "$http_script" 0 "$port"
  assert_success
  touch release_gate
  wait "$dependent_start"
  wait_for_file_content dependency_port "$port"

  pitchfork stop --all
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

@test "a dependent waits for a dependency that is still starting" {
  create_pitchfork_toml <<EOF
[daemons.db]
run = "echo db starting && sleep 5 && echo 'db READY' && sleep 30"
ready_output = "READY"

[daemons.api]
run = "echo api ready && sleep 30"
depends = ["db"]
ready_delay = 1
EOF

  # Get db under way on its own, then start api while db's start still waits
  # for its ready output. api must not start until db is ready.
  pitchfork start db >/dev/null 2>&1 &
  local db_start=$!
  wait_for_status db running 10

  run pitchfork start api
  assert_success

  # Checked before reaping db's start, which would let db get ready on its own
  # and the check could never fail. Had db been skipped as running, api would
  # be up after its 1s ready_delay with db still sleeping.
  run pitchfork logs db --raw
  assert_output --partial "db READY"

  wait "$db_start"

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

@test "stop orders daemons by their dependencies from another directory" {
  local proj="$TEST_TEMP_DIR/stopproj" other="$TEST_TEMP_DIR/elsewhere"
  mkdir -p "$proj" "$other"
  cd "$proj"
  create_pitchfork_toml <<EOF
[daemons.db]
run = "sleep 30"
ready_delay = 0

[daemons.app]
run = "sleep 30"
depends = ["db"]
ready_delay = 0
EOF
  run pitchfork start app
  assert_success

  # The config of stopproj is not visible from here.
  cd "$other"
  PITCHFORK_LOG=debug run pitchfork stop --all
  assert_success
  assert_output --partial 'shutdown order: [[DaemonId { namespace: "stopproj", name: "app" }], [DaemonId { namespace: "stopproj", name: "db" }]]'
}

@test "stop orders daemons through a disabled dependency that never started" {
  local proj other="$TEST_TEMP_DIR/elsewhere"
  proj="$(normalize_path "$TEST_TEMP_DIR/disabledproj")"
  mkdir -p "$proj" "$other"
  # Registered, so the supervisor sees api in config, which disabling checks.
  cat >"$PITCHFORK_CONFIG_DIR/config.toml" <<EOF
[namespaces.disabledproj]
dir = "$proj"
EOF
  cd "$proj"
  create_pitchfork_toml <<EOF
[daemons.db]
run = "sleep 30"
ready_delay = 0

[daemons.api]
run = "sleep 30"
depends = ["db"]
ready_delay = 0

[daemons.web]
run = "sleep 30"
depends = ["api"]
ready_delay = 0
EOF
  run pitchfork disable api
  assert_success
  # api is skipped, so only web records that it needs api, and api has no
  # record saying it needs db.
  run pitchfork start web
  assert_success
  wait_for_status db running
  wait_for_status web running

  cd "$other"
  PITCHFORK_LOG=debug run pitchfork stop --all
  assert_success
  assert_output --partial 'shutdown order: [[DaemonId { namespace: "disabledproj", name: "web" }], [DaemonId { namespace: "disabledproj", name: "db" }]]'
}

@test "stop orders daemons through a disabled dependency in another project" {
  local web_proj db_proj other="$TEST_TEMP_DIR/elsewhere"
  web_proj="$(normalize_path "$TEST_TEMP_DIR/webproj")"
  db_proj="$(normalize_path "$TEST_TEMP_DIR/dbproj")"
  mkdir -p "$web_proj" "$db_proj" "$other"
  cat >"$PITCHFORK_CONFIG_DIR/config.toml" <<EOF
[namespaces.webproj]
dir = "$web_proj"

[namespaces.dbproj]
dir = "$db_proj"
EOF
  cat >"$db_proj/pitchfork.toml" <<EOF
[daemons.db]
run = "sleep 30"
ready_delay = 0

[daemons.api]
run = "sleep 30"
depends = ["db"]
ready_delay = 0
EOF
  cat >"$web_proj/pitchfork.toml" <<EOF
[daemons.web]
run = "sleep 30"
depends = ["dbproj/api"]
ready_delay = 0
EOF
  cd "$web_proj"
  run pitchfork disable dbproj/api
  assert_success
  # dbproj/api is skipped, so web's record names it but nothing records that
  # it needs dbproj/db, and webproj's config does not define it.
  run pitchfork start web
  assert_success
  wait_for_status dbproj/db running
  wait_for_status web running

  cd "$other"
  PITCHFORK_LOG=debug run pitchfork stop --all
  assert_success
  assert_output --partial 'shutdown order: [[DaemonId { namespace: "webproj", name: "web" }], [DaemonId { namespace: "dbproj", name: "db" }]]'
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
