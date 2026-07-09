#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

strip_ansi() {
  sed -E 's/\x1b\[[0-9;]*m//g'
}

# ============================================================================
# Namespace isolation and qualified ID tests
# ============================================================================

@test "daemons from different directories are namespaced and logs are separate" {
  local proj_a="$TEST_TEMP_DIR/project-a"
  local proj_b="$TEST_TEMP_DIR/project-b"
  mkdir -p "$proj_a" "$proj_b"

  cd "$proj_a"
  create_pitchfork_toml <<'EOF'
[daemons.api]
run = "echo 'Hello from project-a' && sleep 10"
EOF
  run pitchfork start api
  assert_success

  cd "$proj_b"
  create_pitchfork_toml <<'EOF'
[daemons.api]
run = "echo 'Hello from project-b' && sleep 10"
EOF
  run pitchfork start api
  assert_success

  wait_for_logs "project-a/api" "Hello from project-a" 5
  wait_for_logs "project-b/api" "Hello from project-b" 5

  pitchfork stop "project-a/api" || true
  pitchfork stop "project-b/api" || true
}

@test "qualified IDs work with status and stop from another directory" {
  local proj="$TEST_TEMP_DIR/myproject"
  local other_dir="$TEST_TEMP_DIR/other"
  mkdir -p "$proj" "$other_dir"

  cd "$proj"
  create_pitchfork_toml <<'EOF'
[daemons.server]
run = "sleep 60"
EOF
  run pitchfork start server
  assert_success

  sleep 1

  cd "$other_dir"
  run pitchfork status "myproject/server"
  assert_success
  assert_output --partial "running"

  run pitchfork stop "myproject/server"
  assert_success

  sleep 1

  run pitchfork status "myproject/server"
  [[ "$output" == *"stopped"* || "$output" == *"exited"* ]]
}

@test "short IDs work when in the correct directory" {
  local proj="$TEST_TEMP_DIR/shorttest"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.myservice]
run = "sleep 30"
EOF
  run pitchfork start myservice
  assert_success

  sleep 1

  run pitchfork status myservice
  assert_success
  assert_output --partial "running"

  run pitchfork stop myservice
  assert_success
}

@test "list shows qualified namespaces when daemon names conflict" {
  local proj_x="$TEST_TEMP_DIR/proj-x"
  local proj_y="$TEST_TEMP_DIR/proj-y"
  mkdir -p "$proj_x" "$proj_y"

  cd "$proj_x"
  create_pitchfork_toml <<'EOF'
[daemons.web]
run = "sleep 60"
EOF
  run pitchfork start web
  assert_success

  cd "$proj_y"
  create_pitchfork_toml <<'EOF'
[daemons.web]
run = "sleep 60"
EOF
  run pitchfork start web
  assert_success

  sleep 1

  cd "$proj_x"
  run pitchfork list
  assert_success

  local list
  list=$(strip_ansi <<< "$output")
  [[ "$list" == *"proj-x/web"* ]]
  [[ "$list" == *"proj-y/web"* ]]

  pitchfork stop "proj-x/web" || true
  cd "$proj_y"
  pitchfork stop "proj-y/web" || true
}

@test "list always shows fully-qualified daemon names" {
  local proj="$TEST_TEMP_DIR/solo-project"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.unique-api]
run = "sleep 60"

[daemons.unique-worker]
run = "sleep 60"
EOF
  run pitchfork start unique-api
  assert_success

  sleep 1

  run pitchfork start unique-worker
  assert_success

  sleep 1

  run pitchfork list
  assert_success

  local list
  list=$(strip_ansi <<< "$output")
  [[ "$list" == *"solo-project/unique-api"* ]]
  [[ "$list" == *"solo-project/unique-worker"* ]]

  pitchfork stop "solo-project/unique-api" || true
  pitchfork stop "solo-project/unique-worker" || true
}

@test "logs command works with qualified IDs from another directory" {
  local proj="$TEST_TEMP_DIR/logtest"
  local other_dir="$TEST_TEMP_DIR/other"
  mkdir -p "$proj" "$other_dir"

  cd "$proj"
  create_pitchfork_toml <<'EOF'
[daemons.logger]
run = "echo 'test log message' && sleep 30"
EOF
  run pitchfork start logger
  assert_success

  sleep 2

  cd "$other_dir"
  run pitchfork logs "logtest/logger" -n 10
  assert_success
  assert_output --partial "test log message"

  pitchfork stop "logtest/logger" || true
}

@test "path encoding roundtrip uses filesystem-safe qualified IDs" {
  local proj="$TEST_TEMP_DIR/my-cool-project"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.my-service]
run = "echo 'encoding test' && sleep 30"
EOF
  run pitchfork start my-service
  assert_success

  sleep 2

  wait_for_logs "my-cool-project/my-service" "encoding test" 5

  local daemon_id
  daemon_id="$(sqlite3 "$PITCHFORK_LOGS_DIR/logs.db" "SELECT DISTINCT daemon_id FROM log_entries WHERE daemon_id LIKE '%my-service';" 2>/dev/null)"
  [[ "$daemon_id" == "my-cool-project/my-service" ]]

  run pitchfork status "my-cool-project/my-service"
  assert_success
  assert_output --partial "my-cool-project/my-service"

  pitchfork stop "my-cool-project/my-service" || true
}

@test "explicit namespace override succeeds when directory name is invalid" {
  local outer="$TEST_TEMP_DIR/foo-bar"
  local inner="$outer/foo--bar"
  mkdir -p "$inner"

  cd "$outer"
  create_pitchfork_toml <<'EOF'
[daemons.outer-daemon]
run = "echo outer && sleep 60"
EOF

  cd "$inner"
  create_pitchfork_toml <<'EOF'
namespace = "inner-override"

[daemons.inner-daemon]
run = "echo inner && sleep 60"
EOF

  run pitchfork list
  assert_success

  local list
  list=$(strip_ansi <<< "$output")
  [[ "$list" == *"outer-daemon"* ]]
  [[ "$list" == *"inner-daemon"* ]]
  [[ "$list" == *"inner-override"* ]]
  [[ "$list" != *"foo--bar"* ]]
}

# ============================================================================
# Slug resolution tests
# ============================================================================

@test "slug resolves daemon in the same directory" {
  local proj="$TEST_TEMP_DIR/slug-same-dir"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.long-service-name]
run = "sleep 60"
EOF
  run pitchfork proxy add svc --daemon long-service-name
  assert_success

  run pitchfork start long-service-name
  assert_success

  sleep 1

  run pitchfork status svc
  assert_success
  assert_output --partial "running"

  run pitchfork stop svc
  assert_success
}

@test "slug resolves daemon from another directory" {
  local proj="$TEST_TEMP_DIR/ns-slug-test"
  local other_dir="$TEST_TEMP_DIR/other"
  mkdir -p "$proj" "$other_dir"

  cd "$proj"
  create_pitchfork_toml <<'EOF'
[daemons.database]
run = "sleep 60"
EOF
  run pitchfork proxy add db --daemon database
  assert_success

  run pitchfork start database
  assert_success

  sleep 1

  cd "$other_dir"
  run pitchfork status db
  assert_success
  assert_output --partial "running"

  run pitchfork stop db
  assert_success
}

@test "list shows daemon registered with a slug" {
  local proj="$TEST_TEMP_DIR/slug-list"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.my-api-service]
run = "sleep 60"
EOF
  run pitchfork proxy add api --daemon my-api-service
  assert_success

  run pitchfork start my-api-service
  assert_success

  sleep 1

  run pitchfork list
  assert_success

  local list
  list=$(strip_ansi <<< "$output")
  [[ "$list" == *"my-api-service"* ]]

  run pitchfork stop my-api-service
  assert_success
}
