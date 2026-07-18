#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  bats_require_minimum_version 1.5.0
  # Fast autostop/refresh for responsive tests. Must be set before
  # _common_setup pre-starts the supervisor so the supervisor process inherits
  # them (env vars set in the test body arrive too late — the supervisor is
  # already running).
  export PITCHFORK_AUTOSTOP_DELAY=0s
  export PITCHFORK_INTERVAL=2s
  _common_setup
}

teardown() {
  # Kill any liveness helper processes left behind by the shared/multi-pid tests.
  if [[ -n "${PROJECT_LIVENESS_PIDS[*]}" ]]; then
    for pid in "${PROJECT_LIVENESS_PIDS[@]}"; do
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    done
    unset PROJECT_LIVENESS_PIDS
  fi

  # Remove any sibling override directory created by directory tests.
  if [[ -n "$PROJECT_OTHER_DIR" ]]; then
    rm -rf "$PROJECT_OTHER_DIR" 2>/dev/null || true
    unset PROJECT_OTHER_DIR
  fi

  _common_teardown
}

# ---------------------------------------------------------------------------
# Project session state helpers
# ---------------------------------------------------------------------------

state_file() {
  echo "$PITCHFORK_STATE_DIR/state.toml"
}

# Canonicalize a path to match the form Rust's `canonicalize()` writes into
# the state file (resolves symlinks like /tmp -> /private/tmp on macOS).
canonicalize() {
  local p="$1"
  if command -v python3 >/dev/null 2>&1; then
    python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$p"
  elif command -v realpath >/dev/null 2>&1; then
    realpath "$p"
  else
    echo "$p"
  fi
}

# Count project session entries for a given host PID in the persisted state.
# The state file stores each session as a deeply-nested table header. The PID
# key is serialized bare (unquoted) because TOML allows all-digit bare keys:
#   [project_sessions.1234."/path/a"]
#   liveness_title = "..."
project_session_count_for_pid() {
  local pid="$1"
  [[ -f "$(state_file)" ]] || { echo 0; return; }
  local n
  n=$(grep -c "^\[project_sessions\.${pid}\." "$(state_file)" 2>/dev/null)
  echo "${n:-0}"
}

# Return true if a project session exists for (pid, dir).
project_session_exists() {
  local pid="$1"
  local dir="$2"
  local canon
  canon="$(canonicalize "$dir")"
  [[ -f "$(state_file)" ]] || return 1
  grep -qF "[project_sessions.${pid}.\"${canon}\"]" "$(state_file)"
}

PROJECT_LIVENESS_PIDS=()

# Spawn a long-lived `sleep` helper to use as a host PID and register it for
# teardown cleanup. Sets the global `LIVENESS_HELPER_PID`. Must be called
# directly (NOT via `$(...)` command substitution): a command-substitution
# subshell exits immediately after returning, which causes bash to reap the
# backgrounded `sleep`, leaving the supervisor with a dead PID.
spawn_liveness_helper() {
  sleep 120 &
  LIVENESS_HELPER_PID=$!
  PROJECT_LIVENESS_PIDS+=("$LIVENESS_HELPER_PID")
}

# ---------------------------------------------------------------------------
# Group A: enter auto-start and leave autostop
# ---------------------------------------------------------------------------

@test "project enter starts auto-start daemons and leave triggers autostop" {

  create_pitchfork_toml <<'EOF'
namespace = "project"

[daemons.auto_svc]
run = "sleep 120"
auto = ["start", "stop"]
ready_delay = 1
EOF

  run pitchfork project enter --pid $$
  assert_success

  wait_for_status project/auto_svc running

  run pitchfork project leave --pid $$
  assert_success

  wait_for_status project/auto_svc stopped
}

# ---------------------------------------------------------------------------
# Group B: directory override
# ---------------------------------------------------------------------------

@test "project enter directory override resolves config from the given directory" {

  create_pitchfork_toml <<'EOF'
namespace = "project"

[daemons.local_svc]
run = "sleep 120"
auto = ["start"]
ready_delay = 1
EOF

  local other_dir
  other_dir="$(mktemp -d /tmp/pf-project-other-XXXXXX)"
  PROJECT_OTHER_DIR="$other_dir"
  mkdir -p "$other_dir"

  cat > "$other_dir/pitchfork.toml" <<'EOF'
namespace = "other"

[daemons.other_svc]
run = "sleep 120"
auto = ["start"]
ready_delay = 1
EOF

  run pitchfork project enter --pid $$ --directory "$other_dir"
  assert_success

  wait_for_status other/other_svc running

  run pitchfork status project/local_svc
  assert_success
  refute_output --partial "running"
}

# ---------------------------------------------------------------------------
# Group C: repeated enter with the same pid and dir is idempotent
# ---------------------------------------------------------------------------

@test "repeated project enter with same pid and dir is idempotent" {

  create_pitchfork_toml <<'EOF'
namespace = "project"

[daemons.svc]
run = "sleep 120"
auto = ["start"]
ready_delay = 1
EOF

  run pitchfork project enter --pid $$
  assert_success
  wait_for_status project/svc running

  # Second enter with the same (pid, dir) replaces the session but must not
  # restart the daemon or create a duplicate session.
  run pitchfork project enter --pid $$
  assert_success

  sleep 3
  run pitchfork status project/svc
  assert_success
  assert_output --partial "running"

  [[ "$(project_session_count_for_pid $$)" == "1" ]]

  run pitchfork project leave --pid $$
  assert_success
}

# ---------------------------------------------------------------------------
# Group D: shared liveness PID across multiple directories
# ---------------------------------------------------------------------------

@test "shared liveness pid revokes all session directories when the pid dies" {
  skip_on_windows "liveness pid tracking relies on Unix process visibility"

  local dir1 dir2
  dir1="$TEST_TEMP_DIR/dir1"
  dir2="$TEST_TEMP_DIR/dir2"
  mkdir -p "$dir1" "$dir2"

  cat > "$dir1/pitchfork.toml" <<'EOF'
namespace = "dir1"

[daemons.d1]
run = "sleep 120"
auto = ["start", "stop"]
ready_delay = 1
EOF

  cat > "$dir2/pitchfork.toml" <<'EOF'
namespace = "dir2"

[daemons.d2]
run = "sleep 120"
auto = ["start", "stop"]
ready_delay = 1
EOF

  spawn_liveness_helper
  local liveness_pid="$LIVENESS_HELPER_PID"

  run pitchfork project enter --pid "$liveness_pid" --directory "$dir1"
  assert_success
  wait_for_status dir1/d1 running

  run pitchfork project enter --pid "$liveness_pid" --directory "$dir2"
  assert_success
  wait_for_status dir2/d2 running

  project_session_exists "$liveness_pid" "$dir1"
  project_session_exists "$liveness_pid" "$dir2"

  kill "$liveness_pid"
  wait "$liveness_pid" 2>/dev/null || true

  # Wait for the refresh loop to notice the dead PID and revoke both sessions.
  sleep 5

  ! project_session_exists "$liveness_pid" "$dir1"
  ! project_session_exists "$liveness_pid" "$dir2"

  wait_for_status dir1/d1 stopped
  wait_for_status dir2/d2 stopped
}

# ---------------------------------------------------------------------------
# Group E: multiple pids sharing the same directory
# ---------------------------------------------------------------------------

@test "multiple pids in the same directory keep the daemon alive until both leave" {
  skip_on_windows "liveness pid tracking relies on Unix process visibility"

  local shared_dir
  shared_dir="$TEST_TEMP_DIR/shared"
  mkdir -p "$shared_dir"

  cat > "$shared_dir/pitchfork.toml" <<'EOF'
namespace = "shared"

[daemons.svc]
run = "sleep 120"
auto = ["start", "stop"]
ready_delay = 1
EOF

  spawn_liveness_helper
  local pid1="$LIVENESS_HELPER_PID"
  spawn_liveness_helper
  local pid2="$LIVENESS_HELPER_PID"

  run pitchfork project enter --pid "$pid1" --directory "$shared_dir"
  assert_success
  wait_for_status shared/svc running

  # Second pid enters the same directory; daemon is already running.
  run pitchfork project enter --pid "$pid2" --directory "$shared_dir"
  assert_success

  [[ "$(project_session_count_for_pid "$pid1")" == "1" ]]
  [[ "$(project_session_count_for_pid "$pid2")" == "1" ]]

  # Leaving the first pid must not autostop: the second pid still covers the dir.
  run pitchfork project leave --pid "$pid1" --directory "$shared_dir"
  assert_success
  sleep 3
  run pitchfork status shared/svc
  assert_success
  assert_output --partial "running"

  # Leaving the second pid removes the last active directory and autostops.
  run pitchfork project leave --pid "$pid2" --directory "$shared_dir"
  assert_success
  wait_for_status shared/svc stopped
}

# ---------------------------------------------------------------------------
# Group F: shell directory + project session interaction
# ---------------------------------------------------------------------------

@test "project session keeps daemon alive when shell leaves the directory" {
  skip_on_windows "liveness pid tracking relies on Unix process visibility"

  create_pitchfork_toml <<'EOF'
namespace = "project"

[daemons.svc]
run = "sleep 120"
auto = ["start", "stop"]
ready_delay = 1
EOF

  local other_dir
  other_dir="$(mktemp -d /tmp/pf-project-shell-XXXXXX)"
  PROJECT_OTHER_DIR="$other_dir"
  mkdir -p "$other_dir"

  run pitchfork cd --shell-pid $$
  assert_success

  # Capture the directory the project session is entered in so the leave call
  # (issued after `cd` away) targets the same (pid, dir) key.
  local proj_dir="$PWD"

  run pitchfork project enter --pid $$ --directory "$proj_dir"
  assert_success
  wait_for_status project/svc running

  cd "$other_dir"
  run pitchfork cd --shell-pid $$
  assert_success

  # Give autostop a chance to fire; the project session should prevent it.
  sleep 3
  run pitchfork status project/svc
  assert_success
  assert_output --partial "running"

  run pitchfork project leave --pid $$ --directory "$proj_dir"
  assert_success

  wait_for_status project/svc stopped
}

# ---------------------------------------------------------------------------
# Group G: invalid config does not create a session
# ---------------------------------------------------------------------------

@test "project enter fails before creating session on invalid config" {
  create_pitchfork_toml <<'EOF'
namespace = "project"

[daemons.svc]
run = "sleep 120"
auto = ["start"
EOF

  run pitchfork project enter --pid $$
  assert_failure
  [[ "$output" == *"error reading"* || "$output" == *"TOML"* || "$output" == *"parse"* ]]

  # The state file may not exist if the supervisor was never started; if it
  # does, no project session for our pid should be present.
  [[ ! -f "$(state_file)" ]] || [[ "$(project_session_count_for_pid $$)" == "0" ]]
}

# ---------------------------------------------------------------------------
# Group H: project list
# ---------------------------------------------------------------------------

@test "project list shows active sessions and is empty after leave" {
  create_pitchfork_toml <<'EOF'
namespace = "project"

[daemons.svc]
run = "sleep 120"
auto = ["start"]
ready_delay = 1
EOF

  run pitchfork project enter --pid $$
  assert_success
  wait_for_status project/svc running

  local proj_dir
  proj_dir="$(canonicalize "$PWD")"

  # Table output (no header when piped): PID, directory, alive, title.
  # `--separate-stderr` keeps DEBUG logs (PITCHFORK_LOG=debug) out of
  # $output; the table itself goes to stdout.
  run --separate-stderr pitchfork project list
  assert_success
  assert_output --partial "$$"
  assert_output --partial "$proj_dir"
  assert_output --partial "alive"

  # JSON output includes the same fields with alive=true.
  run --separate-stderr pitchfork project list --json
  assert_success
  assert_output --partial "\"pid\": $$"
  assert_output --partial "\"directory\": \"$proj_dir\""
  assert_output --partial "\"alive\": true"

  # After leaving, the list is empty (no rows when piped).
  run pitchfork project leave --pid $$
  assert_success
  run --separate-stderr pitchfork project list
  assert_success
  assert_output ""
}
