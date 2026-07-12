#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# ============================================================================
# Basic watch restart
# ============================================================================

@test "watching a file triggers daemon restart" {
  local http_script port
  http_script="$(script_path http_server.py)"
  port=19191
  kill_port "$port"

  create_pitchfork_toml <<EOF
[daemons.watch_test]
run = "python3 -u $http_script 0 $port"
watch = ["watch_test_marker.txt"]
ready_port = $port
EOF

  echo "initial" > watch_test_marker.txt

  run pitchfork start watch_test
  assert_success
  wait_for_status watch_test running

  sleep 0.5
  local original_pid
  original_pid="$(get_daemon_pid watch_test)"
  [[ -n "$original_pid" ]]

  echo "modified" > watch_test_marker.txt

  local new_pid current_pid
  new_pid=""
  for _ in $(seq 1 30); do
    current_pid="$(get_daemon_pid watch_test)"
    if [[ -n "$current_pid" && "$current_pid" != "$original_pid" ]]; then
      new_pid="$current_pid"
      break
    fi
    sleep 0.5
  done

  [[ -n "$new_pid" ]]
  [[ "$new_pid" != "$original_pid" ]]
  wait_for_status watch_test running

  pitchfork stop watch_test
}

# ============================================================================
# Watch config persistence
# ============================================================================

@test "watch configuration persists in state across supervisor restart" {
  create_pitchfork_toml <<'EOF'
[daemons.persist_test]
run = "sleep 60"
watch = ["src/**/*.rs", "Cargo.toml"]
ready_delay = 1
EOF

  mkdir -p src
  touch src/main.rs Cargo.toml

  run pitchfork start persist_test
  assert_success
  wait_for_status persist_test running

  local state_file
  state_file="$PITCHFORK_STATE_DIR/state.toml"
  [[ -f "$state_file" ]]
  grep -F "src/**/*.rs" "$state_file"
  grep -F "Cargo.toml" "$state_file"

  run pitchfork supervisor stop
  sleep 1

  run pitchfork start persist_test
  assert_success
  wait_for_status persist_test running

  grep -F "watch" "$state_file"

  pitchfork stop persist_test
}

# ============================================================================
# Daemons without watch
# ============================================================================

@test "daemon without watch config stays running and does not restart" {
  create_pitchfork_toml <<'EOF'
[daemons.no_watch_test]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start no_watch_test
  assert_success
  wait_for_status no_watch_test running

  local original_pid new_pid
  original_pid="$(get_daemon_pid no_watch_test)"
  [[ -n "$original_pid" ]]

  sleep 2

  new_pid="$(get_daemon_pid no_watch_test)"
  [[ "$new_pid" == "$original_pid" ]]
  [[ "$(get_daemon_status no_watch_test)" == "running" ]]

  pitchfork stop no_watch_test
}

# ============================================================================
# Glob watch patterns
# ============================================================================

@test "glob watch patterns restart daemon on matching file changes" {
  local http_script port
  http_script="$(script_path http_server.py)"
  port=19192
  kill_port "$port"

  create_pitchfork_toml <<EOF
[daemons.glob_watch_test]
run = "python3 -u $http_script 0 $port"
watch = ["lib/**/*.ts", "config/*.json"]
ready_port = $port
EOF

  mkdir -p lib config
  touch lib/main.ts
  echo '{"port": 8080}' > config/app.json

  run pitchfork start glob_watch_test
  assert_success
  wait_for_status glob_watch_test running

  sleep 0.5
  local original_pid first_pid second_pid current_pid
  original_pid="$(get_daemon_pid glob_watch_test)"
  [[ -n "$original_pid" ]]

  echo 'export const x = 1;' > lib/helper.ts

  first_pid="$original_pid"
  for _ in $(seq 1 20); do
    current_pid="$(get_daemon_pid glob_watch_test)"
    if [[ -n "$current_pid" && "$current_pid" != "$original_pid" ]]; then
      first_pid="$current_pid"
      break
    fi
    sleep 0.5
  done
  [[ "$first_pid" != "$original_pid" ]]
  wait_for_status glob_watch_test running

  echo '{"port": 9090}' > config/app.json

  second_pid="$first_pid"
  for _ in $(seq 1 20); do
    current_pid="$(get_daemon_pid glob_watch_test)"
    if [[ -n "$current_pid" && "$current_pid" != "$first_pid" ]]; then
      second_pid="$current_pid"
      break
    fi
    sleep 0.5
  done
  [[ "$second_pid" != "$first_pid" ]]
  wait_for_status glob_watch_test running

  pitchfork stop glob_watch_test
}

# ============================================================================
# Relative watch paths
# ============================================================================

@test "relative watch paths trigger restart on file change" {
  local http_script port
  http_script="$(script_path http_server.py)"
  port=19193
  kill_port "$port"

  create_pitchfork_toml <<EOF
[daemons.relative_watch_test]
run = "python3 -u $http_script 0 $port"
watch = ["./relative_test.txt"]
ready_port = $port
EOF

  echo "initial" > relative_test.txt

  run pitchfork start relative_watch_test
  assert_success
  wait_for_status relative_watch_test running

  sleep 0.5
  local original_pid new_pid current_pid
  original_pid="$(get_daemon_pid relative_watch_test)"
  [[ -n "$original_pid" ]]

  echo "modified" > relative_test.txt

  new_pid="$original_pid"
  for _ in $(seq 1 20); do
    current_pid="$(get_daemon_pid relative_watch_test)"
    if [[ -n "$current_pid" && "$current_pid" != "$original_pid" ]]; then
      new_pid="$current_pid"
      break
    fi
    sleep 0.5
  done
  [[ "$new_pid" != "$original_pid" ]]
  wait_for_status relative_watch_test running

  pitchfork stop relative_watch_test
}

# ============================================================================
# Watch modes
# ============================================================================

@test "watch_mode poll and auto both trigger restart on file changes" {
  local http_script port
  http_script="$(script_path http_server.py)"

  for mode in poll auto; do
    if [[ "$mode" == "poll" ]]; then
      port=19194
    else
      port=19195
    fi
    kill_port "$port"

    create_pitchfork_toml <<EOF
[daemons.${mode}_watch_test]
run = "python3 -u $http_script 0 $port"
watch = ["${mode}_watch_marker.txt"]
watch_mode = "$mode"
ready_port = $port
EOF

    echo "initial" > "${mode}_watch_marker.txt"

    run pitchfork start ${mode}_watch_test
    assert_success
    wait_for_status ${mode}_watch_test running

    local state_file
    state_file="$PITCHFORK_STATE_DIR/state.toml"
    grep -F "watch_mode = \"$mode\"" "$state_file"

    sleep 0.5
    local original_pid new_pid current_pid
    original_pid="$(get_daemon_pid ${mode}_watch_test)"
    [[ -n "$original_pid" ]]

    echo "changed" > "${mode}_watch_marker.txt"

    new_pid="$original_pid"
    for _ in $(seq 1 30); do
      current_pid="$(get_daemon_pid ${mode}_watch_test)"
      if [[ -n "$current_pid" && "$current_pid" != "$original_pid" ]]; then
        new_pid="$current_pid"
        break
      fi
      sleep 0.3
    done
    [[ "$new_pid" != "$original_pid" ]]
    [[ "$(get_daemon_status ${mode}_watch_test)" == "running" ]]

    pitchfork stop ${mode}_watch_test
  done
}
