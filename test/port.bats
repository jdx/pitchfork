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

# Writing a config adds the deprecated port fields beside `port` for older
# versions. Reading them back must not warn, since they say the same thing.
@test "a config pitchfork wrote reads back without a deprecation warning" {
  create_pitchfork_toml <<'EOF'
[daemons.web]
run = "sleep 60"
port = { expect = [4000], bump = 3 }
EOF
  run pitchfork daemons add api --run "sleep 60" --expected-port 8080 --bump
  assert_success
  run cat pitchfork.toml
  assert_output --partial 'expected_port = [4000]'
  assert_output --partial 'expected_port = [8080]'

  PITCHFORK_LOG=warn run pitchfork daemons
  assert_success
  assert_output --partial "web"
  refute_output --partial "deprecated"
}

@test "deprecated port fields that disagree with port still warn" {
  create_pitchfork_toml <<'EOF'
[daemons.web]
run = "sleep 60"
port = { expect = [4000] }
expected_port = [5000]
EOF

  PITCHFORK_LOG=warn run pitchfork daemons
  assert_success
  assert_output --partial "ignoring deprecated fields"
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
  local port
  port="$(free_port)"
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

  # The error names the address whose bind found the port taken.
  assert_output --partial "is already in use on "

  kill "$blocker_pid" 2>/dev/null || true
  wait "$blocker_pid" 2>/dev/null || true
  run pitchfork stop port_conflict || true
}

@test "port auto-bump succeeds when expected port is occupied" {
  # The daemon is bumped to the next port, so that one is claimed too.
  local port
  port="$(free_port_run 2)"
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
  local port
  port="$(free_port)"
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
  local port
  port="$(free_port_run 2)"
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
  # Three occupied ports, then the one the daemon is bumped to.
  local base_port
  base_port="$(free_port_run 4)"
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

# Hold $1 until the returned PID is killed. `occupy_port` lets go after five
# seconds, which a slow start could outlast.
_occupy_port_until_killed() {
  local port="$1"
  nohup python3 -c "
import socket, time
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('0.0.0.0', $port))
s.listen(1)
time.sleep(300)
" >/dev/null 2>&1 &
  echo $!
}

# A ready port given with `--port` is bumped along with the expected ports. A
# restart that finds the expected port free again must probe that port, not
# the one the last run bumped it to.
@test "restarting a bumped ad-hoc daemon probes the port it starts on" {
  local port
  port="$(free_port_run 2)"
  kill_port "$port"
  kill_port "$((port + 1))"
  local blocker_pid
  blocker_pid=$(_occupy_port_until_killed "$port")
  _wait_for_port_bound "$port" || true

  create_pitchfork_toml <<EOF
EOF

  local http_script
  http_script="$(script_path http_server.py)"
  run pitchfork run adhoc_bumped --expected-port "$port" --bump 5 --port "$port" -- \
    sh -c 'python3 -u "$0" 0 "$PORT"' "$http_script"
  assert_success
  wait_for_logs adhoc_bumped "Starting HTTP server on port $((port + 1))\."

  kill "$blocker_pid" 2>/dev/null || true
  wait "$blocker_pid" 2>/dev/null || true

  run timeout 20 pitchfork restart adhoc_bumped
  assert_success
  wait_for_logs adhoc_bumped "Starting HTTP server on port $port\."

  run pitchfork stop adhoc_bumped || true
  kill_port "$port"
  kill_port "$((port + 1))"
}

# A ready port outside the expected ports is not bumped, even when it is the
# number a bump would produce. A restart keeps probing it as given.
@test "restarting a bumped ad-hoc daemon keeps a ready port given as another port" {
  local port
  port="$(free_port_run 2)"
  kill_port "$port"
  kill_port "$((port + 1))"
  local blocker_pid
  blocker_pid=$(_occupy_port_until_killed "$port")
  _wait_for_port_bound "$port" || true

  create_pitchfork_toml <<EOF
EOF

  # The server always listens on port + 1, the ready port given.
  local http_script
  http_script="$(script_path http_server.py)"
  run pitchfork run adhoc_ready_other --expected-port "$port" --bump 5 --port "$((port + 1))" -- \
    python3 -u "$http_script" 0 "$((port + 1))"
  assert_success

  kill "$blocker_pid" 2>/dev/null || true
  wait "$blocker_pid" 2>/dev/null || true

  run timeout 20 pitchfork restart adhoc_ready_other
  assert_success
  run pitchfork status adhoc_ready_other
  assert_output --partial "running"

  run pitchfork stop adhoc_ready_other || true
  kill_port "$port"
  kill_port "$((port + 1))"
}
