#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# ============================================================================
# Config add tests
# ============================================================================

@test "config add with port and bump" {
  run pitchfork daemons add api --run "python3 -m http.server 8080" --expected-port 8080 --bump
  assert_success

  run cat pitchfork.toml
  assert_output --partial 'expect = [8080]'
  assert_output --partial 'bump = 10'
}

@test "config add with only port" {
  run pitchfork daemons add api --run "python3 -m http.server 3000" --expected-port 3000
  assert_success

  run cat pitchfork.toml
  assert_output --partial 'port = 3000'
}

# ============================================================================
# Port conflict and auto-bump tests
# ============================================================================

_wait_for_port_bound() {
  local port="$1"
  for _ in $(seq 1 20); do
    if (exec 3<>/dev/tcp/127.0.0.1/"$port") 2>/dev/null; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

@test "port conflict detection fails without auto-bump" {
  local port=45678
  local blocker_pid
  blocker_pid=$(occupy_port "$port")
  _wait_for_port_bound "$port" || true

  create_pitchfork_toml <<EOF
[daemons.port_conflict]
run = "python3 -m http.server $port"
port = $port
EOF

  run pitchfork start port_conflict 2>&1
  assert_failure

  [[ "$output" == *"already in use"* ]] || [[ "$output" == *"port"* ]] || [[ "$output" == *"Port"* ]]

  kill "$blocker_pid" 2>/dev/null || true
  wait "$blocker_pid" 2>/dev/null || true
  run pitchfork stop port_conflict || true
}

@test "port auto-bump succeeds when expected port is occupied" {
  local port=45679
  local blocker_pid
  blocker_pid=$(occupy_port "$port")

  cat > test_auto_bump.sh <<'EOF'
#!/bin/bash
python3 -c "
import http.server
import socketserver
import os
port = int(os.environ.get('PORT', 45680))
with socketserver.TCPServer(('', port), http.server.SimpleHTTPRequestHandler) as httpd:
    print(f'Server running on port {port}')
    httpd.handle_request()
" &
sleep 1
echo "ready"
sleep 30
EOF
  chmod +x test_auto_bump.sh

  create_pitchfork_toml <<EOF
[daemons.port_bump]
run = "bash $(pwd)/test_auto_bump.sh"
expect = [$port]
bump = 10
ready_output = "ready"
EOF

  run pitchfork start port_bump
  assert_success

  wait_for_status port_bump running

  kill "$blocker_pid" 2>/dev/null || true
  wait "$blocker_pid" 2>/dev/null || true
  run pitchfork stop port_bump || true
}

@test "PORT environment variable is injected into daemon" {
  local port=45800
  local marker="$TEST_TEMP_DIR/port_test_marker"

  cat > test_port.sh <<'EOF'
#!/bin/bash
echo "PORT=$PORT" > "$1"
sleep 30
EOF
  chmod +x test_port.sh

  create_pitchfork_toml <<EOF
[daemons.port_env]
run = "bash $(pwd)/test_port.sh $marker"
port = $port
EOF

  run pitchfork start port_env
  assert_success

  wait_for_file "$marker"
  run cat "$marker"
  assert_output "PORT=$port"

  run pitchfork stop port_env || true
}

@test "CLI --expected-port and --bump with occupied port" {
  local port=45681
  local blocker_pid
  blocker_pid=$(occupy_port "$port")
  _wait_for_port_bound "$port" || true

  create_pitchfork_toml <<EOF
[daemons.cli_port_test]
run = "python3 -m http.server 0"
EOF

  run pitchfork start cli_port_test --expected-port "$port"
  assert_failure

  run pitchfork start cli_port_test --expected-port "$port" --bump
  assert_success

  kill "$blocker_pid" 2>/dev/null || true
  wait "$blocker_pid" 2>/dev/null || true
  run pitchfork stop cli_port_test || true
}

@test "PITCHFORK_PORT_BUMP_ATTEMPTS env var limits bump attempts" {
  local base_port=45710
  local pids=()
  pids+=("$(occupy_port "$base_port")")
  pids+=("$(occupy_port "$((base_port + 1))")")
  pids+=("$(occupy_port "$((base_port + 2))")")
  _wait_for_port_bound "$base_port" || true
  _wait_for_port_bound "$((base_port + 1))" || true
  _wait_for_port_bound "$((base_port + 2))" || true

  cat > test_env_bump.sh <<'EOF'
#!/bin/bash
python3 -c "
import http.server
import socketserver
import os
port = int(os.environ.get('PORT', 45713))
with socketserver.TCPServer(('', port), http.server.SimpleHTTPRequestHandler) as httpd:
    print(f'Server running on port {port}')
    httpd.handle_request()
" &
sleep 1
echo "ready"
sleep 30
EOF
  chmod +x test_env_bump.sh

  create_pitchfork_toml <<EOF
[daemons.env_bump]
run = "bash $(pwd)/test_env_bump.sh"
expected_port = [$base_port]
auto_bump_port = true
ready_output = "ready"
EOF

  run env PITCHFORK_PORT_BUMP_ATTEMPTS=2 pitchfork start env_bump
  assert_failure

  run env PITCHFORK_PORT_BUMP_ATTEMPTS=5 pitchfork start env_bump
  assert_success

  wait_for_status env_bump running

  for pid in "${pids[@]}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  run pitchfork stop env_bump || true
}
