#!/usr/bin/env bats

# Idle shutdown of daemons the proxy auto-started (`proxy.idle_timeout`,
# per-daemon `proxy_idle_timeout`).

setup() {
  load test_helper/common_setup
  _common_setup
  skip_on_windows "the tests use POSIX shell PIDs and python test servers"

  PROXY_PORT=$(_free_port)
  STREAM_SCRIPT="$(to_shell_path "$(script_path stream_server.py)")"
  # Check idleness often, so a short grace period is noticed promptly.
  export PITCHFORK_INTERVAL=500ms
}

teardown() {
  _common_teardown
}

_free_port() {
  python3 -c "import socket; s=socket.socket(); s.bind(('127.0.0.1', 0)); print(s.getsockname()[1]); s.close()"
}

# Start the supervisor with the proxy on plain HTTP. Extra `VAR=value`
# arguments are passed to it as environment.
start_proxy_supervisor() {
  env PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_DNS=false \
    PITCHFORK_PROXY_PORT="$PROXY_PORT" \
    "$@" \
    pitchfork supervisor start --force >/dev/null 2>&1
}

# Write a daemon that serves stream_server.py on `port`.
# Usage: stream_daemon <name> <port> [extra toml lines...]
stream_daemon() {
  local name="$1" port="$2"
  shift 2
  echo "[daemons.$name]"
  echo "run = 'python3 -u $STREAM_SCRIPT $port'"
  echo "ready_http = \"http://127.0.0.1:$port/health\""
  local line
  for line in "$@"; do
    echo "$line"
  done
  echo
}

# Start and stop daemons once, which records the project in the state file so
# its hostnames are known to the proxy, and leaves nothing running.
register_project() {
  run pitchfork start "$@"
  assert_success
  local d
  for d in "$@"; do
    pitchfork stop "$d" >/dev/null 2>&1 || true
  done
  for d in "$@"; do
    wait_for_status "$d" stopped
  done
  # The hostname registry is cached for a couple of seconds.
  sleep 3
}

# Request a path on a daemon's hostname through the proxy; prints the status.
proxy_get() {
  curl -s -o /dev/null -w "%{http_code}" --max-time 40 \
    -H "Host: $1" "http://127.0.0.1:$PROXY_PORT${2:-/health}"
}

supervisor_log() {
  cat "$PITCHFORK_LOGS_DIR/pitchfork/pitchfork.log" 2>/dev/null
}

@test "proxy-started daemons stop when idle, dependents before dependencies" {
  local proj="$TEST_TEMP_DIR/idleproj"
  mkdir -p "$proj" && cd "$proj"
  local db_port api_port manual_port
  db_port=$(_free_port)
  api_port=$(_free_port)
  manual_port=$(_free_port)
  {
    stream_daemon db "$db_port"
    stream_daemon api "$api_port" "port = $api_port" 'depends = ["db"]'
    stream_daemon manual "$manual_port" "port = $manual_port"
  } >pitchfork.toml

  start_proxy_supervisor PITCHFORK_PROXY_IDLE_TIMEOUT=3s
  register_project api db
  # Started explicitly: never stopped for inactivity.
  run pitchfork start manual
  assert_success

  run proxy_get api.idleproj.localhost
  assert_output "200"
  wait_for_status db running 5
  wait_for_status api running 5

  # No further traffic: api goes after its grace period, then db, which
  # nothing needs any more.
  wait_for_status api stopped 20
  wait_for_status db stopped 10
  local api_line db_line
  api_line=$(supervisor_log | grep -n "stopping idleproj/api: no proxy activity" | head -1 | cut -d: -f1)
  db_line=$(supervisor_log | grep -n "stopping idleproj/db: no proxy activity" | head -1 | cut -d: -f1)
  [[ -n "$api_line" && -n "$db_line" && "$api_line" -lt "$db_line" ]]

  # The explicitly started daemon saw no traffic either, and is untouched.
  wait_for_status manual running 1

  # The next request starts the stack again.
  run proxy_get api.idleproj.localhost
  assert_output "200"
  wait_for_status db running 5
  wait_for_status api running 5
}

@test "idle shutdown is off by default, and a daemon can opt in on its own" {
  local proj="$TEST_TEMP_DIR/optproj"
  mkdir -p "$proj" && cd "$proj"
  local keep_port quick_port
  keep_port=$(_free_port)
  quick_port=$(_free_port)
  {
    stream_daemon keep "$keep_port" "port = $keep_port"
    stream_daemon quick "$quick_port" "port = $quick_port" 'proxy_idle_timeout = "2s"'
  } >pitchfork.toml

  start_proxy_supervisor
  register_project keep quick

  run proxy_get keep.optproj.localhost
  assert_output "200"
  run proxy_get quick.optproj.localhost
  assert_output "200"

  wait_for_status quick stopped 15
  # Well past the other daemon's grace period, and several idle checks later,
  # the daemon without a timeout still runs — the behavior before this
  # feature.
  sleep 3
  wait_for_status keep running 1
}

@test "a streaming response and an open WebSocket keep a daemon running" {
  local proj="$TEST_TEMP_DIR/streamproj"
  mkdir -p "$proj" && cd "$proj"
  local port
  port=$(_free_port)
  stream_daemon web "$port" "port = $port" >pitchfork.toml

  start_proxy_supervisor PITCHFORK_PROXY_IDLE_TIMEOUT=2s
  register_project web

  # A response streaming for longer than the grace period.
  run proxy_get web.streamproj.localhost
  assert_output "200"
  curl -s -o /dev/null --max-time 20 -H "Host: web.streamproj.localhost" \
    "http://127.0.0.1:$PROXY_PORT/stream?secs=6" &
  local stream_pid=$!
  sleep 4.5
  wait_for_status web running 1
  wait "$stream_pid"

  # A quiet upgraded connection held open longer than the grace period.
  python3 "$(script_path hold_upgrade.py)" "$PROXY_PORT" web.streamproj.localhost /ws 6 \
    >"$TEST_TEMP_DIR/upgrade.out" &
  local ws_pid=$!
  sleep 4.5
  run cat "$TEST_TEMP_DIR/upgrade.out"
  assert_output --partial "101"
  wait_for_status web running 1
  wait "$ws_pid"

  # Once both have ended, the grace period runs out.
  wait_for_status web stopped 15
}

@test "starting a proxy-started daemon explicitly keeps it running" {
  local proj="$TEST_TEMP_DIR/claimproj"
  mkdir -p "$proj" && cd "$proj"
  local port
  port=$(_free_port)
  stream_daemon api "$port" "port = $port" >pitchfork.toml

  start_proxy_supervisor PITCHFORK_PROXY_IDLE_TIMEOUT=2s
  register_project api

  run proxy_get api.claimproj.localhost
  assert_output "200"
  # Already running, so nothing is started, but the daemon is now the user's.
  run pitchfork start api
  assert_success

  sleep 5
  wait_for_status api running 1
}

@test "a shell inside the project keeps proxy-started daemons running" {
  local proj="$TEST_TEMP_DIR/shellproj"
  mkdir -p "$proj" && cd "$proj"
  local port
  port=$(_free_port)
  stream_daemon api "$port" "port = $port" >pitchfork.toml

  start_proxy_supervisor PITCHFORK_PROXY_IDLE_TIMEOUT=2s
  register_project api

  run proxy_get api.shellproj.localhost
  assert_output "200"
  # A shell (this test's own process) sitting in the project directory.
  run pitchfork cd --shell-pid $$
  assert_success

  sleep 5
  wait_for_status api running 1

  # Once the shell leaves, the grace period applies again.
  cd "$TEST_TEMP_DIR"
  run pitchfork cd --shell-pid $$
  assert_success
  wait_for_status api stopped 15
}

@test "a shared dependency stays while another consumer still runs" {
  local proj="$TEST_TEMP_DIR/shareproj"
  mkdir -p "$proj" && cd "$proj"
  local db_port api_port admin_port
  db_port=$(_free_port)
  api_port=$(_free_port)
  admin_port=$(_free_port)
  {
    stream_daemon db "$db_port"
    stream_daemon api "$api_port" "port = $api_port" 'depends = ["db"]'
    # Proxied too, but never stopped for inactivity.
    stream_daemon admin "$admin_port" "port = $admin_port" 'depends = ["db"]' \
      'proxy_idle_timeout = false'
  } >pitchfork.toml

  start_proxy_supervisor PITCHFORK_PROXY_IDLE_TIMEOUT=2s
  register_project api admin db

  # api starts db, so db is the proxy's; admin then finds it running.
  run proxy_get api.shareproj.localhost
  assert_output "200"
  run proxy_get admin.shareproj.localhost
  assert_output "200"

  wait_for_status api stopped 15
  # Several idle checks later, db is still needed by admin.
  sleep 3
  wait_for_status db running 1
  wait_for_status admin running 1
}
