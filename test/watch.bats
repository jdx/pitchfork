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
watch_mode = "poll"
ready_port = $port
EOF

  echo "initial" > watch_test_marker.txt

  run pitchfork start watch_test
  assert_success
  wait_for_status watch_test running

  sleep 2
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
    sleep 2
  done

  [[ -n "$new_pid" ]]
  [[ "$new_pid" != "$original_pid" ]]
  wait_for_status watch_test running

  pitchfork stop watch_test
}

@test "a dependent waits for a file-triggered restart to become ready" {
  touch ready marker
  create_pitchfork_toml <<'TOML'
[daemons.watched_ready]
run = "if test -e started; then touch restarted; else touch started; fi; while ! test -e ready; do sleep 0.1; done; echo READY; sleep 30"
ready_output = "READY"
watch = ["marker"]
watch_mode = "poll"

[daemons.dependent]
run = "touch dependent_started; sleep 30"
depends = ["watched_ready"]
ready_delay = 0
TOML

  run pitchfork start watched_ready
  assert_success
  # Let the polling watcher take its initial sample before changing the file.
  sleep 2
  rm ready
  echo changed >marker
  wait_for_file restarted 15

  pitchfork start dependent >waiting_start.log 2>&1 &
  local waiting_start=$!
  wait_for_readiness_waiter watched_ready
  assert_file_not_exist dependent_started
  touch ready
  wait "$waiting_start"
  wait_for_file dependent_started

  pitchfork stop --all
}

# ============================================================================
# Watch config persistence
# ============================================================================

# Regression: https://github.com/jdx/pitchfork/pull/1108#discussion_r4505580962
@test "a dependent waits through retries of a file-triggered restart" {
  touch marker
  cat > restart_retry.sh <<'SCRIPT'
count=0
test ! -e attempts || count=$(cat attempts)
count=$((count + 1))
echo "$count" >attempts
if test "$count" -eq 2; then
  touch restarting
  while test ! -e release_failure; do sleep 0.1; done
  exit 4
fi
echo READY
sleep 60
SCRIPT
  create_pitchfork_toml <<'TOML'
[daemons.restart_retry]
run = "sh restart_retry.sh"
ready_output = "READY"
retry = 1
watch = ["marker"]
watch_mode = "poll"

[daemons.restart_dependent]
run = "touch dependent_started; sleep 30"
depends = ["restart_retry"]
ready_delay = 0
TOML

  run pitchfork start restart_retry
  assert_success
  sleep 2
  echo changed >marker
  wait_for_file restarting 15
  (
    local start_status=0
    pitchfork start restart_dependent >waiting_start.log 2>&1 || start_status=$?
    echo "$start_status" >result
  ) &
  local waiting_start=$!
  wait_for_readiness_waiter restart_retry
  touch release_failure
  wait_for_file result 15
  wait "$waiting_start"
  run cat result
  assert_output "0"
  wait_for_file dependent_started
  run cat attempts
  assert_output "3"
}

# Regression: https://github.com/jdx/pitchfork/pull/1108#discussion_r4505580973
@test "a restart readiness timeout remains a failure after a clean SIGTERM exit" {
  skip_on_windows "this regression exercises Unix SIGTERM handling"
  touch marker
  cat > clean_exit.py <<'PYTHON'
import pathlib
import signal
import sys
import time

signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
attempts = pathlib.Path("attempts")
count = int(attempts.read_text()) + 1 if attempts.exists() else 1
attempts.write_text(str(count))
if count == 1:
    print("READY", flush=True)
else:
    pathlib.Path("restarted").touch()
while True:
    time.sleep(0.1)
PYTHON
  create_pitchfork_toml <<'TOML'
[daemons.clean_timeout]
run = ["python3", "-u", "clean_exit.py"]
ready_output = { pattern = "READY", timeout = "1s" }
retry = 0
watch = ["marker"]
watch_mode = "poll"

[daemons.clean_timeout.hooks]
on_fail = 'echo "$PITCHFORK_EXIT_CODE" >failure_code'
TOML

  run pitchfork start clean_timeout
  assert_success
  sleep 2
  echo changed >marker
  wait_for_file restarted 15
  run cat attempts
  assert_output "2"
  wait_for_file failure_code 15
  run cat failure_code
  assert_output "124"
  wait_for_status clean_timeout errored
}

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
watch_mode = "poll"
ready_port = $port
EOF

  mkdir -p lib config
  touch lib/main.ts
  echo '{"port": 8080}' > config/app.json

  run pitchfork start glob_watch_test
  assert_success
  wait_for_status glob_watch_test running

  sleep 2
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
    sleep 2
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
    sleep 2
  done
  [[ "$second_pid" != "$first_pid" ]]
  wait_for_status glob_watch_test running

  pitchfork stop glob_watch_test
}

@test "file created with a new matching directory triggers restart" {
  local http_script port
  http_script="$(script_path http_server.py)"
  port=19196
  kill_port "$port"

  create_pitchfork_toml <<EOF
[daemons.new_dir_watch_test]
run = "python3 -u $http_script 0 $port"
watch = ["crates/*/Cargo.toml", "conf/app/*.toml"]
watch_mode = "native"
ready_port = $port
EOF

  mkdir -p crates

  run pitchfork start new_dir_watch_test
  assert_success
  wait_for_status new_dir_watch_test running

  sleep 2
  local original_pid new_pid nested_pid current_pid
  original_pid="$(get_daemon_pid new_dir_watch_test)"
  [[ -n "$original_pid" ]]

  # Only crates/ is watched, so the file must be found via its new directory
  mkdir crates/new && touch crates/new/Cargo.toml

  new_pid="$original_pid"
  for _ in $(seq 1 20); do
    current_pid="$(get_daemon_pid new_dir_watch_test)"
    if [[ -n "$current_pid" && "$current_pid" != "$original_pid" ]]; then
      new_pid="$current_pid"
      break
    fi
    sleep 2
  done
  [[ "$new_pid" != "$original_pid" ]]
  wait_for_status new_dir_watch_test running

  # Neither directory exists yet; only the project directory is watched
  mkdir -p conf/app && touch conf/app/settings.toml

  nested_pid="$new_pid"
  for _ in $(seq 1 20); do
    current_pid="$(get_daemon_pid new_dir_watch_test)"
    if [[ -n "$current_pid" && "$current_pid" != "$new_pid" ]]; then
      nested_pid="$current_pid"
      break
    fi
    sleep 2
  done
  [[ "$nested_pid" != "$new_pid" ]]
  wait_for_status new_dir_watch_test running

  pitchfork stop new_dir_watch_test
}

@test "directory moved into a recursive watch triggers restart" {
  local http_script port
  http_script="$(script_path http_server.py)"
  port=19197
  kill_port "$port"

  create_pitchfork_toml <<EOF
[daemons.moved_dir_watch_test]
run = "python3 -u $http_script 0 $port"
watch = ["src/**/*.rs"]
watch_mode = "native"
ready_port = $port
EOF

  mkdir -p src staging/moved
  touch src/main.rs staging/moved/lib.rs

  run pitchfork start moved_dir_watch_test
  assert_success
  wait_for_status moved_dir_watch_test running

  sleep 2
  local original_pid new_pid current_pid
  original_pid="$(get_daemon_pid moved_dir_watch_test)"
  [[ -n "$original_pid" ]]

  # Only the directory's move is reported; its lib.rs must be found inside it
  mv staging/moved src/moved

  new_pid="$original_pid"
  for _ in $(seq 1 20); do
    current_pid="$(get_daemon_pid moved_dir_watch_test)"
    if [[ -n "$current_pid" && "$current_pid" != "$original_pid" ]]; then
      new_pid="$current_pid"
      break
    fi
    sleep 2
  done
  [[ "$new_pid" != "$original_pid" ]]
  wait_for_status moved_dir_watch_test running

  pitchfork stop moved_dir_watch_test
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
watch = ["relative_test.txt"]
watch_mode = "poll"
ready_port = $port
EOF

  echo "initial" > relative_test.txt

  run pitchfork start relative_watch_test
  assert_success
  wait_for_status relative_watch_test running

  sleep 2
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
    sleep 2
  done
  [[ "$new_pid" != "$original_pid" ]]
  wait_for_status relative_watch_test running

  pitchfork stop relative_watch_test
}

# The project directory is a path, not a pattern: `[1]` in its name must not
# be read as a character class.
@test "watch works in a project directory whose name has glob characters" {
  mkdir 'app[1]'
  cd 'app[1]'
  create_pitchfork_toml <<'EOF'
namespace = "app1"

[daemons.glob_dir_watch]
run = "sleep 60"
watch = ["*.js"]
watch_mode = "poll"
EOF
  echo "initial" > index.js

  run pitchfork start glob_dir_watch
  assert_success
  wait_for_status glob_dir_watch running

  sleep 2
  local original_pid new_pid current_pid
  original_pid="$(get_daemon_pid glob_dir_watch)"
  [[ -n "$original_pid" ]]

  echo "modified" > index.js

  new_pid="$original_pid"
  for _ in $(seq 1 20); do
    current_pid="$(get_daemon_pid glob_dir_watch)"
    if [[ -n "$current_pid" && "$current_pid" != "$original_pid" ]]; then
      new_pid="$current_pid"
      break
    fi
    sleep 2
  done
  [[ "$new_pid" != "$original_pid" ]]

  pitchfork stop glob_dir_watch
}

# ============================================================================
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

    sleep 2
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

@test "a stop during a file-watch restart cancels the restart" {
  # Hold the gap between the restart's stop and its start open.
  export PITCHFORK_RESTART_DELAY=4s
  pitchfork supervisor start --force >/dev/null 2>&1

  for how in one all; do
    create_pitchfork_toml <<EOF
[daemons.watch_cancel]
run = "sleep 60"
watch = ["watch_cancel_marker.txt"]
watch_mode = "poll"
ready_delay = 1
EOF
    echo "initial" > watch_cancel_marker.txt

    run pitchfork start watch_cancel
    assert_success
    wait_for_status watch_cancel running

    echo "changed $how" > watch_cancel_marker.txt
    wait_for_status watch_cancel restarting

    if [[ "$how" == all ]]; then
      run pitchfork stop --all
    else
      run pitchfork stop watch_cancel
    fi
    assert_success
    wait_for_status watch_cancel stopped

    # The restart does not start the daemon once its delay is over.
    sleep 6
    [[ "$(get_daemon_status watch_cancel)" == "stopped" ]]
  done
}
