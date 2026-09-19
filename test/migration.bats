#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  bats_require_minimum_version 1.5.0
  _common_setup
}

teardown() {
  _common_teardown
}

# Stop the supervisor that _common_setup started before replacing the state
# file. Overwriting it while that supervisor runs would drop its own record, so
# the next command would auto-start a second supervisor and teardown would only
# stop that one, leaving the first running with bats' output pipe open.
# Wait for it to exit so it cannot flush its own state over the fixture.
stop_supervisor_for_state_rewrite() {
  pitchfork supervisor stop 2>/dev/null || true
  # `stop` finds the supervisor through state.toml. Also stop any supervisor
  # for this state dir that it missed, e.g. because its record was not flushed.
  _stop_leaked_supervisors
  [[ -n "${_SETUP_SUPERVISOR_PID:-}" ]] || return 0
  local _
  for _ in $(seq 1 100); do
    if [[ -n "${_SETUP_SUPERVISOR_IDENTITY:-}" ]]; then
      # Compare identities so a reused PID does not look like the supervisor.
      [[ "$(_supervisor_identity "$_SETUP_SUPERVISOR_PID")" == "$_SETUP_SUPERVISOR_IDENTITY" ]] || return 0
    else
      pid_alive "$_SETUP_SUPERVISOR_PID" || return 0
    fi
    sleep 0.1
  done
  echo "supervisor $_SETUP_SUPERVISOR_PID still running after stop" >&2
  return 1
}

assert_state_keys_qualified() {
  local state_file="$PITCHFORK_STATE_DIR/state.toml"
  local key
  while IFS= read -r key; do
    if [[ "$key" != *"/"* ]]; then
      echo "Unqualified daemon key found: $key" >&2
      return 1
    fi
  done < <(grep -oE '^\[daemons\.[^]]+\]' "$state_file" | sed 's/^\[daemons\.//; s/\]$//')
}

@test "state file is migrated from old bare-name format" {
  local state_file="$PITCHFORK_STATE_DIR/state.toml"
  stop_supervisor_for_state_rewrite
  cat > "$state_file" <<'EOF'
[daemons.myservice]
id = "myservice"
autostop = false
retry = 0
retry_count = 0
status = "stopped"

[daemons.worker]
id = "worker"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
last_exit_success = true
EOF

  create_pitchfork_toml <<EOF
namespace = "project"

[daemons.probe]
run = "sleep 60"
EOF

  run --separate-stderr pitchfork list
  assert_success
  [[ "$stderr" != *"Please delete"* ]]
  [[ "$stderr" != *"WARN"* ]]

  sleep 0.5

  assert_state_keys_qualified

  ! grep -q '^\[daemons\.myservice\]' "$state_file"
  ! grep -q '^\[daemons\.worker\]' "$state_file"
}

@test "state file migration preserves disabled daemons" {
  local state_file="$PITCHFORK_STATE_DIR/state.toml"
  stop_supervisor_for_state_rewrite
  cat > "$state_file" <<'EOF'
disabled = ["api"]

[daemons.api]
id = "api"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
EOF

  create_pitchfork_toml <<EOF
namespace = "project"

[daemons.probe]
run = "sleep 60"
EOF

  run --separate-stderr pitchfork list
  assert_success
  [[ "$stderr" != *"Please delete"* ]]
  [[ "$stderr" != *"WARN"* ]]

  sleep 0.5

  ! grep -q '^\[daemons\.api\]' "$state_file"
}

@test "log directories are migrated from old bare-name layout" {
  mkdir -p "$PITCHFORK_LOGS_DIR/api"
  mkdir -p "$PITCHFORK_LOGS_DIR/worker"
  echo "2025-01-01 00:00:00 api hello" > "$PITCHFORK_LOGS_DIR/api/api.log"
  echo "2025-01-01 00:00:01 worker starting" > "$PITCHFORK_LOGS_DIR/worker/worker.log"

  run --separate-stderr pitchfork logs
  assert_success

  [[ ! -d "$PITCHFORK_LOGS_DIR/api" ]]
  [[ ! -d "$PITCHFORK_LOGS_DIR/worker" ]]
  [[ -d "$PITCHFORK_LOGS_DIR/legacy--api" ]]
  [[ -d "$PITCHFORK_LOGS_DIR/legacy--worker" ]]
  [[ ! -e "$PITCHFORK_LOGS_DIR/legacy--api/legacy--api.log" ]]
  [[ ! -e "$PITCHFORK_LOGS_DIR/legacy--worker/legacy--worker.log" ]]

  run pitchfork logs legacy/api --raw
  assert_output --partial "hello"

  run pitchfork logs legacy/worker --raw
  assert_output --partial "starting"
}

@test "log directory migration is idempotent" {
  mkdir -p "$PITCHFORK_LOGS_DIR/legacy--api"
  echo "2025-01-01 00:00:00 legacy/api already migrated" > "$PITCHFORK_LOGS_DIR/legacy--api/legacy--api.log"

  run pitchfork logs
  assert_success

  [[ -d "$PITCHFORK_LOGS_DIR/legacy--api" ]]
  [[ ! -e "$PITCHFORK_LOGS_DIR/legacy--api/legacy--api.log" ]]

  run pitchfork logs legacy/api --raw
  assert_output --partial "already migrated"

  rm -rf "$PITCHFORK_LOGS_DIR/pitchfork"

  run --separate-stderr pitchfork logs
  assert_success

  [[ ! -d "$PITCHFORK_LOGS_DIR/legacy--legacy--api" ]]
  [[ $(grep -c "auto-migrated" <<< "$stderr") -eq 0 ]]
}

@test "log migration only touches old-format directories" {
  mkdir -p "$PITCHFORK_LOGS_DIR/legacy"
  echo "2025-01-01 00:00:00 legacy old log" > "$PITCHFORK_LOGS_DIR/legacy/legacy.log"

  mkdir -p "$PITCHFORK_LOGS_DIR/myns--svc"
  echo "2025-01-01 00:00:00 myns/svc new log" > "$PITCHFORK_LOGS_DIR/myns--svc/myns--svc.log"

  run --separate-stderr pitchfork logs
  assert_success

  [[ ! -d "$PITCHFORK_LOGS_DIR/legacy" ]]
  [[ -d "$PITCHFORK_LOGS_DIR/legacy--legacy" ]]
  [[ -d "$PITCHFORK_LOGS_DIR/myns--svc" ]]
  [[ ! -d "$PITCHFORK_LOGS_DIR/legacy--myns--svc" ]]
}

@test "new-format state file is not migrated" {
  local state_file="$PITCHFORK_STATE_DIR/state.toml"
  stop_supervisor_for_state_rewrite
  cat > "$state_file" <<'EOF'
disabled = []

[daemons."legacy/myservice"]
id = "legacy/myservice"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
last_exit_success = true
EOF

  create_pitchfork_toml <<EOF
namespace = "project"

[daemons.probe]
run = "sleep 60"
EOF

  run --separate-stderr pitchfork list
  assert_success
  [[ "$stderr" != *"Please delete"* ]]
  [[ "$stderr" != *"WARN"* ]]

  sleep 0.3

  assert_state_keys_qualified
}
