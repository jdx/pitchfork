#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

require_sqlite3() { command -v sqlite3 >/dev/null 2>&1 || skip "sqlite3 not available"; }

_log_messages() {
  local pattern="$1"
  local db_path="$PITCHFORK_LOGS_DIR/logs.db"
  if command -v cygpath >/dev/null 2>&1; then
    db_path="$(cygpath -m "$db_path")"
  fi
  sqlite3 "$db_path" "SELECT message FROM log_entries WHERE daemon_id LIKE '%$pattern';" 2>/dev/null
}

_wait_for_sqlite_message() {
  local pattern="$1"
  local needle="$2"
  local timeout_secs="${3:-10}"
  for _ in $(seq 1 "$((timeout_secs * 5))"); do
    if _log_messages "$pattern" | grep -q "$needle"; then
      return 0
    fi
    sleep 0.2
  done
  echo "Timed out waiting for SQLite messages of '$pattern' to contain: $needle" >&2
  return 1
}

_messages_contain_ansi() {
  local pattern="$1"
  local db_path="$PITCHFORK_LOGS_DIR/logs.db"
  if command -v cygpath >/dev/null 2>&1; then
    db_path="$(cygpath -m "$db_path")"
  fi
  python3 - <<PY
import sqlite3, sys
conn = sqlite3.connect("$db_path")
c = conn.cursor()
c.execute("SELECT message FROM log_entries WHERE daemon_id LIKE '%$pattern'")
for row in c.fetchall():
    if "\x1b[" in row[0]:
        sys.exit(0)
sys.exit(1)
PY
}

# ============================================================================
# PTY / ANSI preservation tests
# ============================================================================

@test "log preserves ANSI escape codes by default" {
  require_sqlite3
  local ansi_script
  ansi_script="$(script_path ansi_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.ansi_test]
run = 'bash $ansi_script 32 green && sleep 60'
EOF

  run pitchfork start ansi_test
  assert_success

  _wait_for_sqlite_message "ansi_test" "green" 5

  run _messages_contain_ansi "ansi_test"
  assert_success

  pitchfork stop ansi_test || true
}

@test "pty = true allocates a pseudo-terminal" {
  skip_on_windows "PTY allocation is not supported on Windows"
  create_pitchfork_toml <<'EOF'
[daemons.with_pty]
run = "if [ -t 0 ] && [ -t 1 ]; then echo HAS_TTY; else echo NO_TTY; fi && sleep 30"
pty = true
EOF

  run pitchfork start with_pty
  assert_success

  wait_for_logs with_pty "HAS_TTY" 5

  pitchfork stop with_pty || true
}

@test "pty = false (default) does not allocate a pseudo-terminal" {
  create_pitchfork_toml <<'EOF'
[daemons.no_pty]
run = "if [ -t 0 ] && [ -t 1 ]; then echo HAS_TTY; else echo NO_TTY; fi && sleep 30"
EOF

  run pitchfork start no_pty
  assert_success

  wait_for_logs no_pty "NO_TTY" 5

  pitchfork stop no_pty || true
}

@test "pty mode preserves ANSI escape codes in logs" {
  require_sqlite3
  local ansi_script
  ansi_script="$(script_path ansi_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.pty_ansi]
run = 'bash $ansi_script 31 red && sleep 60'
pty = true
EOF

  run pitchfork start pty_ansi
  assert_success

  _wait_for_sqlite_message "pty_ansi" "red" 5

  run _messages_contain_ansi "pty_ansi"
  assert_success

  pitchfork stop pty_ansi || true
}
