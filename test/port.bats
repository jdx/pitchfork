#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

@test "config add with port and auto_bump_port" {
  run pitchfork config add api --run "python3 -m http.server 8080" --port 8080 --auto-bump-port
  assert_success
  
  run cat pitchfork.toml
  assert_output --partial 'port = 8080'
  assert_output --partial 'auto_bump_port = true'
}

@test "config add with only port" {
  run pitchfork config add api --run "python3 -m http.server 3000" --port 3000
  assert_success
  
  run cat pitchfork.toml
  assert_output --partial 'port = 3000'
  refute_output --partial 'auto_bump_port = true'
}

@test "start with --expected-port flag" {
  # Create a simple server script
  cat > server.sh <<'EOF'
#!/bin/bash
echo "Server starting on port $PORT"
sleep 1
echo "ready"
sleep 30
EOF
  chmod +x server.sh
  
  run pitchfork config add test-server --run "./server.sh" --ready-output "ready" --retry 0
  assert_success
  
  # Start with expected-port
  run pitchfork start test-server --expected-port 9999
  assert_success
  
  # Cleanup
  run pitchfork stop test-server || true
}

@test "start with --expected-port and --auto-bump-port flags" {
  # Create a simple server script
  cat > server.sh <<'EOF'
#!/bin/bash
echo "Server starting on port $PORT"
sleep 1
echo "ready"
sleep 30
EOF
  chmod +x server.sh
  
  run pitchfork config add test-server --run "./server.sh" --ready-output "ready" --retry 0
  assert_success
  
  # Start with both flags
  run pitchfork start test-server --expected-port 9999 --auto-bump-port
  assert_success
  
  # Cleanup
  run pitchfork stop test-server || true
}

@test "start fails when expected-port is in use without auto-bump" {
  # Bind to a port first
  python3 -c "
import socket
import time
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(('127.0.0.1', 38888))
s.listen(1)
time.sleep(5)
" &
  BLOCKER_PID=$!
  
  # Wait a bit for the port to be bound
  sleep 0.5
  
  # Create a simple server script
  cat > server.sh <<'EOF'
#!/bin/bash
echo "Server starting"
sleep 1
echo "ready"
sleep 30
EOF
  chmod +x server.sh
  
  run pitchfork config add test-server --run "./server.sh" --ready-output "ready" --retry 0
  assert_success
  
  # Try to start with the occupied port - should fail
  run pitchfork start test-server --expected-port 38888
  assert_failure
  
  # Clean up the blocking process
  kill $BLOCKER_PID 2>/dev/null || true
  wait $BLOCKER_PID 2>/dev/null || true
  
  # Cleanup pitchfork daemon
  run pitchfork stop test-server || true
}

@test "start succeeds when expected-port is in use with auto-bump" {
  # Bind to a port first
  python3 -c "
import socket
import time
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(('127.0.0.1', 38889))
s.listen(1)
time.sleep(5)
" &
  BLOCKER_PID=$!
  
  # Wait a bit for the port to be bound
  sleep 0.5
  
  # Create a simple server script
  cat > server.sh <<'EOF'
#!/bin/bash
echo "Server starting on port $PORT"
sleep 1
echo "ready"
sleep 30
EOF
  chmod +x server.sh
  
  run pitchfork config add test-server --run "./server.sh" --ready-output "ready" --retry 0
  assert_success
  
  # Try to start with the occupied port but with auto-bump - should succeed
  run pitchfork start test-server --expected-port 38889 --auto-bump-port
  assert_success
  
  # Clean up the blocking process
  kill $BLOCKER_PID 2>/dev/null || true
  wait $BLOCKER_PID 2>/dev/null || true
  
  # Cleanup pitchfork daemon
  run pitchfork stop test-server || true
}

@test "PORT environment variable is set correctly" {
  # Create a script that outputs the PORT env var
  cat > check_port.sh <<'EOF'
#!/bin/bash
echo "PORT_VALUE=$PORT" >&2
sleep 1
echo "ready"
sleep 30
EOF
  chmod +x check_port.sh
  
  run pitchfork config add port-test --run "./check_port.sh" --port 7777 --ready-output "ready" --retry 0
  assert_success
  
  run pitchfork start port-test
  assert_success
  
  # Check logs for PORT_VALUE
  run pitchfork logs port-test
  assert_output --partial "PORT_VALUE=7777"
  
  # Cleanup
  run pitchfork stop port-test || true
}
