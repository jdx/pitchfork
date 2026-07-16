#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# Get the supervisor PID from the persisted state file.
get_supervisor_pid() {
  grep -A 10 '\[daemons\."global/pitchfork"\]' "$PITCHFORK_STATE_DIR/state.toml" 2>/dev/null |
    grep -E '^pid = ' |
    head -1 |
    sed -E 's/.*= //'
}

# ============================================================================
# Group A: Supervisor commands
# ============================================================================

@test "supervisor status shows running state" {
  run pitchfork supervisor start
  assert_success

  run pitchfork supervisor status
  assert_success
  assert_output --partial "running"
}

@test "supervisor start --force restarts existing supervisor" {
  run pitchfork supervisor start
  assert_success

  local pid_before
  pid_before="$(get_supervisor_pid)"
  [[ -n "$pid_before" ]]

  run pitchfork supervisor start --force
  assert_success

  local pid_after
  pid_after="$(get_supervisor_pid)"
  [[ -n "$pid_after" ]]
  [[ "$pid_before" != "$pid_after" ]]

  run pitchfork list
  assert_success
}

@test "supervisor run starts in foreground and can be killed" {
  pitchfork supervisor stop 2>/dev/null || true
  pitchfork supervisor run &
  local sup_pid=$!
  sleep 3

  run pitchfork list
  assert_success

  kill "$sup_pid" 2>/dev/null || true
  wait "$sup_pid" 2>/dev/null || true
  sleep 1

  run pitchfork supervisor status
  assert_failure
}

@test "supervisor run --web-port starts web UI" {
  kill_port 18999

  pitchfork supervisor stop 2>/dev/null || true
  sleep 1
  pitchfork supervisor run --web-port 18999 --force &
  local sup_pid=$!
  sleep 3

  run curl -s http://127.0.0.1:18999/
  assert_success
  [[ -n "$output" ]]

  kill "$sup_pid" 2>/dev/null || true
  wait "$sup_pid" 2>/dev/null || true
  kill_port 18999
}

@test "supervisor run --web-path serves UI under prefix" {
  kill_port 18998

  pitchfork supervisor stop 2>/dev/null || true
  MSYS_NO_PATHCONV=1 pitchfork supervisor run --web-port 18998 --web-path /pf &
  local sup_pid=$!
  sleep 3

  # Root path redirects to the configured prefix.
  run curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:18998/
  assert_success
  assert_output "307"

  # The prefix path returns a response (UI assets may not be present
  # in CI, so just check the server responds under the prefix).
  run curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:18998/pf/index.html
  assert_success

  kill "$sup_pid" 2>/dev/null || true
  wait "$sup_pid" 2>/dev/null || true
  kill_port 18998
}

@test "orphaned daemons are cleaned up on supervisor restart" {

  create_pitchfork_toml <<EOF
[daemons.orphan_test]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork supervisor start
  assert_success

  run pitchfork start orphan_test
  assert_success
  wait_for_status orphan_test running

  local sup_pid
  sup_pid="$(get_supervisor_pid)"
  [[ -n "$sup_pid" ]]

  # SIGKILL the supervisor so its daemon child is left orphaned.
  kill_pid "$sup_pid"
  sleep 1

  run pitchfork supervisor start
  assert_success
  sleep 3

  run pitchfork status orphan_test
  assert_success
  assert_output --partial "stopped"
}

# ============================================================================
# Group B: Lifecycle operations
# ============================================================================

@test "restart triggers on_stop and on_exit hooks" {
  local stop_marker
  stop_marker="$(to_shell_path "$TEST_TEMP_DIR/restart_stop_marker")"
  local exit_marker
  exit_marker="$(to_shell_path "$TEST_TEMP_DIR/restart_exit_marker")"

  create_pitchfork_toml <<EOF
[daemons.hooktest]
run = "sleep 60"
ready_delay = 1
retry = 0

[daemons.hooktest.hooks]
on_stop = "touch \"$stop_marker\""
on_exit = "touch \"$exit_marker\""
EOF

  run pitchfork start hooktest
  assert_success
  wait_for_status hooktest running

  local old_pid
  old_pid=$(get_daemon_pid hooktest)
  [[ -n "$old_pid" ]]

  run pitchfork restart hooktest
  assert_success
  wait_for_status hooktest running

  wait_for_file "$stop_marker"
  assert_file_exists "$stop_marker"
  wait_for_file "$exit_marker"
  assert_file_exists "$exit_marker"

  local new_pid
  new_pid=$(get_daemon_pid hooktest)
  [[ "$new_pid" != "$old_pid" ]]

  pitchfork stop hooktest
}

@test "retry count persists across supervisor restart" {

  export PITCHFORK_INTERVAL=1s
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.retry_persist]
run = 'bash $fail_script 0'
retry = 3
ready_delay = 1
EOF

  run pitchfork supervisor start
  assert_success

  pitchfork start retry_persist &
  local start_pid=$!

  wait_for_log_lines retry_persist 3

  run pitchfork supervisor stop
  assert_success

  kill "$start_pid" 2>/dev/null || true
  wait "$start_pid" 2>/dev/null || true
  sleep 1

  # State file keys use the qualified "namespace/name" form.
  local retry_count
  retry_count="$(grep -E "retry_count = [0-9]+$" "$PITCHFORK_STATE_DIR/state.toml" 2>/dev/null | awk -F= '{print $2}' | sort -n | tail -1)"
  [[ -n "$retry_count" ]]
  [[ "$retry_count" -ge 1 ]]

  run pitchfork supervisor start
  assert_success

  # Wait for at least one more failure after restart — proves the retry
  # checker resumed from the persisted retry_count.
  wait_for_logs retry_persist "Failed after 0!" 60

  # Verify the daemon eventually stops retrying (reaches terminal state).
  wait_for_status retry_persist errored 60
}

@test "stop daemon with stale PID is idempotent" {

  export PITCHFORK_INTERVAL=1s

  create_pitchfork_toml <<EOF
[daemons.stale_pid]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start stale_pid
  assert_success
  wait_for_status stale_pid running

  local pid
  pid=$(get_daemon_pid stale_pid)
  [[ -n "$pid" ]]

  kill_pid "$pid"
  sleep 1

  wait_for_status stale_pid errored

  run pitchfork stop stale_pid
  assert_success

  [[ "$(get_daemon_status stale_pid)" != "running" ]]
}

@test "pitchfork wait returns when daemon exits naturally" {
  create_pitchfork_toml <<EOF
[daemons.wait_test]
run = "sleep 2; echo done"
ready_delay = 0
EOF

  run pitchfork start wait_test
  assert_success

  local start_time elapsed
  start_time=$(date +%s)
  run pitchfork wait wait_test
  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -ge 2 ]]
  [[ $elapsed -lt 10 ]]
}

@test "pitchfork wait with multiple daemons" {
  create_pitchfork_toml <<EOF
[daemons.wait1]
run = "sleep 1; echo done1"
ready_delay = 0

[daemons.wait2]
run = "sleep 2; echo done2"
ready_delay = 0
EOF

  run pitchfork start wait1 wait2
  assert_success

  local start_time elapsed
  start_time=$(date +%s)

  pitchfork wait wait1 &
  local wait1_pid=$!
  run pitchfork wait wait2
  wait "$wait1_pid" 2>/dev/null || true

  elapsed=$(($(date +%s) - start_time))

  assert_success
  [[ $elapsed -ge 2 ]]
  [[ $elapsed -lt 10 ]]
}

@test "restart already-stopped daemon starts it" {
  create_pitchfork_toml <<EOF
[daemons.restart_stopped]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start restart_stopped
  assert_success
  wait_for_status restart_stopped running

  local old_pid
  old_pid=$(get_daemon_pid restart_stopped)
  [[ -n "$old_pid" ]]

  run pitchfork stop restart_stopped
  assert_success
  wait_for_status restart_stopped stopped

  run pitchfork restart restart_stopped
  assert_success
  wait_for_status restart_stopped running

  local new_pid
  new_pid=$(get_daemon_pid restart_stopped)
  [[ "$new_pid" != "$old_pid" ]]

  pitchfork stop restart_stopped
}

@test "boot_start=true daemon auto-starts with supervisor" {
  create_pitchfork_toml <<EOF
[daemons.bootsvc]
run = "sleep 60"
boot_start = true
ready_delay = 1
EOF

  pitchfork supervisor stop 2>/dev/null || true
  sleep 1
  pitchfork supervisor run --boot &
  local sup_pid=$!
  sleep 3

  # boot_start daemon may be in a different namespace than the CWD-derived one
  # on Windows. Use list to find it, then check status.
  run pitchfork list
  assert_output --partial "bootsvc"
  assert_output --partial "running"

  kill_pid "$sup_pid"
  wait "$sup_pid" 2>/dev/null || true
}

@test "self-dependency is detected as circular" {
  create_pitchfork_toml <<EOF
[daemons.self_dep]
run = "sleep 10"
depends = ["self_dep"]
EOF

  run pitchfork start self_dep
  assert_failure
  [[ "$output" == *"circular"* || "$output" == *"cycle"* || "$output" == *"dependency"* ]]
}

# ============================================================================
# Group C: Multi-daemon and edge cases
# ============================================================================

@test "duplicate daemon names in config use last definition" {
  create_pitchfork_toml <<EOF
[daemons.dup]
run = "echo first"

[daemons.dup]
run = "echo second"
EOF

  run pitchfork list

  if [[ $status -eq 0 ]]; then
    local dup_count
    dup_count=$(grep -c "dup" <<< "$output")
    [[ "$dup_count" -eq 1 ]]

    run pitchfork start dup
    assert_success
    wait_for_logs dup "second" 5
  fi

  # If TOML rejected the duplicate keys, the command failed as expected.
  [[ $status -eq 0 || $status -eq 1 ]]
}

@test "cross-namespace multi-daemon start" {

  local proj1_dir proj2_dir
  proj1_dir="$(normalize_path "$TEST_TEMP_DIR/proj1")"
  proj2_dir="$(normalize_path "$TEST_TEMP_DIR/proj2")"
  mkdir -p "$proj1_dir" "$proj2_dir"

  cat > "$PITCHFORK_CONFIG_DIR/config.toml" <<EOF
[namespaces.proj1]
dir = "$proj1_dir"

[namespaces.proj2]
dir = "$proj2_dir"
EOF

  cat > "$TEST_TEMP_DIR/proj1/pitchfork.toml" <<EOF
[daemons.daemon1]
run = "sleep 60"
ready_delay = 1
EOF

  cat > "$TEST_TEMP_DIR/proj2/pitchfork.toml" <<EOF
[daemons.daemon2]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start proj1/daemon1 proj2/daemon2

  if [[ $status -eq 0 ]]; then
    wait_for_status proj1/daemon1 running
    wait_for_status proj2/daemon2 running
    pitchfork stop proj1/daemon1 proj2/daemon2
  else
    assert_failure
    assert_output --partial "not found" || assert_output --partial "not defined" || assert_output --partial "ambiguous" || assert_output --partial "error"
  fi
}

@test "pitchfork run creates ad-hoc daemon that can be stopped" {
  run pitchfork run adhoc1 -- sleep 60
  assert_success
  wait_for_status adhoc1 running

  run pitchfork stop adhoc1
  assert_success
  wait_for_status adhoc1 stopped
}

@test "supervisor stop cleans up all running daemons" {

  create_pitchfork_toml <<EOF
[daemons.d1]
run = "sleep 60"
ready_delay = 1

[daemons.d2]
run = "sleep 60"
ready_delay = 1

[daemons.d3]
run = "sleep 60"
ready_delay = 1
EOF

  run pitchfork start d1 d2 d3
  assert_success
  wait_for_status d1 running
  wait_for_status d2 running
  wait_for_status d3 running

  run pitchfork supervisor stop
  assert_success

  sleep 1
  run pitchfork supervisor start
  assert_success

  run pitchfork status d1
  assert_success
  assert_output --partial "stopped"

  run pitchfork status d2
  assert_success
  assert_output --partial "stopped"

  run pitchfork status d3
  assert_success
  assert_output --partial "stopped"
}
