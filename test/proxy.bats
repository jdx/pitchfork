#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# ============================================================================
# Slug resolution tests
# ============================================================================

@test "slug start/stop resolves daemon by slug" {
  local proj="$TEST_TEMP_DIR/slugtest"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.api-server]
run = "sleep 60"
EOF

  run pitchfork proxy add api --daemon api-server
  assert_success

  run pitchfork start api
  assert_success

  sleep 1

  run pitchfork status api
  assert_success
  assert_output --partial "running"

  run pitchfork stop api
  assert_success

  sleep 1

  run pitchfork status api
  [[ "$output" == *"stopped"* || "$output" == *"exited"* ]]
}

@test "slug resolves from a different directory" {
  local proj="$TEST_TEMP_DIR/cross-slug"
  local other_dir="$TEST_TEMP_DIR/other"
  mkdir -p "$proj" "$other_dir"

  cd "$proj"
  create_pitchfork_toml <<'EOF'
[daemons.backend]
run = "sleep 60"
EOF

  run pitchfork proxy add be --daemon backend
  assert_success

  run pitchfork start backend
  assert_success

  sleep 1

  cd "$other_dir"
  run pitchfork status be
  assert_success
  assert_output --partial "running"

  run pitchfork stop be
  assert_success
}

@test "slug takes priority over daemon name in another namespace" {
  local proj_a="$TEST_TEMP_DIR/proj-a"
  local proj_b="$TEST_TEMP_DIR/proj-b"
  mkdir -p "$proj_a" "$proj_b"

  cd "$proj_a"
  create_pitchfork_toml <<'EOF'
[daemons.web]
run = "sleep 60"
EOF
  run pitchfork proxy add frontend --daemon web
  assert_success

  cd "$proj_b"
  create_pitchfork_toml <<'EOF'
[daemons.frontend]
run = "sleep 60"
EOF

  cd "$proj_a"
  run pitchfork start web
  assert_success
  sleep 0.5

  cd "$proj_b"
  run pitchfork start frontend
  assert_success
  sleep 1

  cd "$proj_a"
  run pitchfork status frontend
  assert_success
  assert_output --partial "running"
  [[ "$output" != *"proj-b"* ]]
  [[ "$output" == *"proj-a"* || "$output" == *"web"* ]]

  cd "$proj_a"
  run pitchfork stop web || true
  cd "$proj_b"
  run pitchfork stop frontend || true
}

@test "slug takes priority over same-named daemon in another namespace" {
  local proj_c="$TEST_TEMP_DIR/proj-c"
  local proj_d="$TEST_TEMP_DIR/proj-d"
  mkdir -p "$proj_c" "$proj_d"

  cd "$proj_c"
  create_pitchfork_toml <<'EOF'
[daemons.frontend]
run = "sleep 60"
EOF
  run pitchfork proxy add frontend-slug --daemon frontend
  assert_success

  cd "$proj_d"
  create_pitchfork_toml <<'EOF'
[daemons.frontend]
run = "sleep 60"
EOF

  cd "$proj_c"
  run pitchfork start frontend
  assert_success
  sleep 0.5

  cd "$proj_d"
  run pitchfork start frontend
  assert_success
  sleep 1

  cd "$proj_d"
  run pitchfork status frontend-slug
  assert_success
  assert_output --partial "running"
  assert_output --partial "proj-c"
  [[ "$output" != *"proj-d"* ]]

  cd "$proj_c"
  run pitchfork stop frontend || true
  cd "$proj_d"
  run pitchfork stop frontend || true
}

@test "logs command resolves daemon by slug" {
  local proj="$TEST_TEMP_DIR/slug-logs"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.myservice]
run = "echo 'slug log test' && sleep 30"
EOF

  run pitchfork proxy add svc --daemon myservice
  assert_success

  run pitchfork start svc
  assert_success

  sleep 2

  run pitchfork logs svc -n 10
  assert_success
  assert_output --partial "slug log test"

  run pitchfork stop myservice || true
}

@test "restart command resolves daemon by slug" {
  local proj="$TEST_TEMP_DIR/slug-restart"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.worker]
run = "sleep 60"
EOF

  run pitchfork proxy add w --daemon worker
  assert_success

  run pitchfork start w
  assert_success

  sleep 1

  run pitchfork restart w
  assert_success

  sleep 1

  run pitchfork status w
  assert_success
  assert_output --partial "running"

  run pitchfork stop worker || true
}

# ============================================================================
# Proxy URL display tests
# ============================================================================

_free_port() {
  python3 -c "import socket; s=socket.socket(); s.bind(('127.0.0.1', 0)); print(s.getsockname()[1]); s.close()"
}

@test "list shows proxy URL when proxy is enabled" {
  local proj="$TEST_TEMP_DIR/proxy-list"
  mkdir -p "$proj"
  cd "$proj"

  local port
  port=$(_free_port)

  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.api]
run = "python3 -u $http_script 0 $port"
port = $port
EOF

  run pitchfork proxy add api
  assert_success

  run pitchfork start api
  assert_success
  sleep 2

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7777 pitchfork list
  assert_success
  assert_output --partial "localhost:7777"

  run pitchfork stop api || true
  kill_port "$port"
}

@test "status shows proxy URL when proxy is enabled" {
  local proj="$TEST_TEMP_DIR/proxy-status"
  mkdir -p "$proj"
  cd "$proj"

  local port
  port=$(_free_port)

  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.server]
run = "python3 -u $http_script 0 $port"
port = $port
EOF

  run pitchfork proxy add server
  assert_success

  run pitchfork start server
  assert_success
  sleep 2

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7777 pitchfork status server
  assert_success
  assert_output --partial "Proxy:"
  assert_output --partial "localhost:7777"

  run pitchfork stop server || true
  kill_port "$port"
}

@test "start shows proxy URL when proxy is enabled" {
  local proj="$TEST_TEMP_DIR/proxy-start"
  mkdir -p "$proj"
  cd "$proj"

  local port
  port=$(_free_port)

  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.app]
run = "python3 -u $http_script 0 $port"
port = $port
EOF

  run pitchfork proxy add app
  assert_success

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7777 pitchfork start app
  assert_success
  [[ "$output" == *"Proxy:"* || "$output" == *"localhost:7777"* ]]

  run pitchfork stop app || true
  kill_port "$port"
}

# ============================================================================
# Proxy command tests
# ============================================================================

@test "proxy status shows disabled when proxy is off" {
  create_pitchfork_toml <<'EOF'
[daemons.dummy]
run = "sleep 1"
EOF

  run env PITCHFORK_PROXY_ENABLE=false pitchfork proxy status
  assert_success
  assert_output --partial "disabled"
}

@test "proxy status shows enabled when proxy is on" {
  create_pitchfork_toml <<'EOF'
[daemons.dummy]
run = "sleep 1"
EOF

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7777 pitchfork proxy status
  assert_success
  assert_output --partial "enabled"
  assert_output --partial "localhost"
  assert_output --partial "7777"
}

@test "proxy trust fails when certificate is missing" {
  create_pitchfork_toml <<'EOF'
[daemons.dummy]
run = "sleep 1"
EOF

  run pitchfork proxy trust 2>&1
  assert_failure

  [[ "$output" == *"not found"* ]] || [[ "$output" == *"certificate"* ]] || [[ "$output" == *"Certificate"* ]] || [[ "$output" == *"ca.pem"* ]]
}

# ============================================================================
# Proxy URL format tests
# ============================================================================

@test "global namespace daemon without slug has no proxy URL" {
  local proj="$TEST_TEMP_DIR/global-proxy"
  mkdir -p "$proj"
  cd "$proj"

  local port
  port=$(_free_port)

  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
namespace = "global"

[daemons.myapi]
run = "python3 -u $http_script 0 $port"
expect = [$port]
EOF

  run pitchfork start myapi
  assert_success
  sleep 2

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7777 pitchfork status myapi
  assert_success
  [[ "$output" != *"Proxy:"* ]]

  run pitchfork stop myapi || true
  kill_port "$port"
}

@test "local namespace daemon without slug has no proxy URL" {
  local proj="$TEST_TEMP_DIR/myproject"
  mkdir -p "$proj"
  cd "$proj"

  local port
  port=$(_free_port)

  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
namespace = "myproject"

[daemons.api]
run = "python3 -u $http_script 0 $port"
expect = [$port]
EOF

  run pitchfork start api
  assert_success
  sleep 2

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7777 pitchfork status api
  assert_success
  [[ "$output" != *"Proxy:"* ]]

  run pitchfork stop api || true
  kill_port "$port"
}
