#!/usr/bin/env bats

setup() {
  export PITCHFORK_INTERVAL=2s
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

@test "health cmd failure kills daemon and retry restarts it" {
  create_pitchfork_toml <<EOF
[daemons.unhealthy]
run = "echo started; sleep 300"
retry = 1
health_cmd = { run = "exit 1", interval = "1s", retries = 2 }
EOF

  run pitchfork start unhealthy
  assert_success

  local deadline
  deadline=$(($(date +%s) + 40))

  # The daemon must be killed through the crash path (health check named as
  # the reason in the supervisor log), then restarted by the retry checker
  # ("started" once per run), then killed again. End state: errored with
  # exactly two runs.
  while true; do
    local sup_log status logs count
    sup_log="$PITCHFORK_LOGS_DIR/pitchfork/pitchfork.log"
    status=$(get_daemon_status unhealthy)
    logs=$(read_logs unhealthy)
    count=$(grep -c "started" <<< "$logs" || true)
    if [[ "$status" == *"errored"* ]] \
      && [[ $count -ge 2 ]] \
      && grep -q "health check failure" "$sup_log" 2>/dev/null; then
      break
    fi
    if [[ $(date +%s) -ge $deadline ]]; then
      echo "timed out: status=$status runs=$count" >&2
      break
    fi
    sleep 1
  done

  run pitchfork status unhealthy
  assert_output --partial "errored"

  run pitchfork logs unhealthy --raw
  local count
  count=$(grep -c "started" <<< "$output" || true)
  [[ $count -eq 2 ]]

  pitchfork stop unhealthy || true
}

@test "daemon with passing health cmd keeps running untouched" {
  create_pitchfork_toml <<EOF
[daemons.healthy]
run = "sleep 300"
health_cmd = { run = "exit 0", interval = "1s", retries = 2 }
EOF

  run pitchfork start healthy
  assert_success
  wait_for_status healthy running 30

  local pid_before pid_after
  pid_before=$(get_daemon_pid healthy)
  [[ -n "$pid_before" ]]

  # Several health intervals must pass without the daemon being touched.
  sleep 5

  pid_after=$(get_daemon_pid healthy)
  [[ -n "$pid_after" ]]
  [[ "$pid_after" == "$pid_before" ]]

  run pitchfork status healthy
  assert_output --partial "running"

  pitchfork stop healthy
}

@test "health http failure kills daemon and retry restarts it" {
  local port
  port=$(python3 -c "import socket; s=socket.socket(); s.bind(('127.0.0.1', 0)); print(s.getsockname()[1]); s.close()")
  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.webhealth]
run = "python3 -u $http_script 0 $port 500"
retry = 1
health_http = { url = "http://127.0.0.1:$port/health", interval = "1s", retries = 2 }
EOF

  run pitchfork start webhealth
  assert_success

  local deadline
  deadline=$(($(date +%s) + 40))

  while true; do
    local status logs count
    status=$(get_daemon_status webhealth)
    logs=$(read_logs webhealth)
    count=$(grep -c "Server listening on" <<< "$logs" || true)
    if [[ "$status" == *"errored"* ]] && [[ $count -ge 2 ]]; then
      break
    fi
    if [[ $(date +%s) -ge $deadline ]]; then
      echo "timed out: status=$status runs=$count" >&2
      break
    fi
    sleep 1
  done

  run pitchfork status webhealth
  assert_output --partial "errored"

  run pitchfork logs webhealth --raw
  local count
  count=$(grep -c "Server listening on" <<< "$output" || true)
  [[ $count -eq 2 ]]

  kill_port "$port"
  pitchfork stop webhealth || true
}

@test "health port failure kills daemon and retry restarts it" {
  local port
  port=$(python3 -c "import socket; s=socket.socket(); s.bind(('127.0.0.1', 0)); print(s.getsockname()[1]); s.close()")
  local port_script
  port_script="$(script_path health_port_server.py)"

  create_pitchfork_toml <<EOF
[daemons.porthealth]
run = "python3 -u $port_script $port 3"
retry = 1
health_port = { port = $port, interval = "1s", retries = 2 }
EOF

  run pitchfork start porthealth
  assert_success

  # While the port is listening the probe passes and the daemon is untouched.
  wait_for_status porthealth running 30

  local deadline
  deadline=$(($(date +%s) + 40))

  # The script closes its listener after 3s, so the health probe must fail
  # twice in a row, kill the daemon through the crash path, and the retry
  # checker starts it once more ("Listening on" per run). End state: errored
  # with exactly two runs.
  while true; do
    local status logs count
    status=$(get_daemon_status porthealth)
    logs=$(read_logs porthealth)
    count=$(grep -c "Listening on" <<< "$logs" || true)
    if [[ "$status" == *"errored"* ]] && [[ $count -ge 2 ]]; then
      break
    fi
    if [[ $(date +%s) -ge $deadline ]]; then
      echo "timed out: status=$status runs=$count" >&2
      break
    fi
    sleep 1
  done

  run pitchfork status porthealth
  assert_output --partial "errored"

  run pitchfork logs porthealth --raw
  local count
  count=$(grep -c "Listening on" <<< "$output" || true)
  [[ $count -eq 2 ]]

  pitchfork stop porthealth || true
}