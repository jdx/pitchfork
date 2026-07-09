#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# Query the SQLite log database directly
query_logs_db() { sqlite3 "$PITCHFORK_LOGS_DIR/logs.db" "$1" 2>/dev/null; }

# Skip tests that need sqlite3 if it is not installed
require_sqlite3() { command -v sqlite3 >/dev/null 2>&1 || skip "sqlite3 not available"; }

@test "clearing logs for one daemon preserves others" {
  require_sqlite3
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.daemon1]
run = 'bash $slowly_output 1 5'

[daemons.daemon2]
run = 'bash $slowly_output 1 5'
EOF

  run pitchfork start daemon1
  assert_success
  run pitchfork start daemon2
  assert_success

  wait_for_log_lines daemon1 3
  wait_for_log_lines daemon2 3

  run pitchfork logs daemon1 --clear
  assert_success

  PITCHFORK_LOG=error run pitchfork logs daemon1 --raw
  assert_output ""
  refute_output --partial "Output"

  run pitchfork logs daemon2 --raw
  assert_output --partial "Output 1/5"
  assert_output --partial "Output 3/5"

  pitchfork stop daemon1
  pitchfork stop daemon2
}

@test "tail cursor advances and does not replay old logs" {
  require_sqlite3
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.tail_test]
run = 'bash $slowly_output 1 30'
EOF

  run pitchfork start tail_test
  assert_success

  wait_for_log_lines tail_test 3

  # Capture tail output for a short window
  run timeout 3 pitchfork logs tail_test --tail -n 3 --raw --no-timestamp
  [[ "$status" -eq 0 || "$status" -eq 124 ]]
  local first_output="$output"

  sleep 3

  run timeout 3 pitchfork logs tail_test --tail -n 3 --raw --no-timestamp
  [[ "$status" -eq 0 || "$status" -eq 124 ]]
  local second_output="$output"

  # The first line of the first output should NOT appear in the second output
  # because the tail cursor advanced past it.
  local first_line
  first_line=$(echo "$first_output" | grep "Output" | head -1)
  [[ -n "$first_line" ]]
  [[ "$second_output" != *"$first_line"* ]]
}

@test "time retention setting is accepted in config" {
  require_sqlite3
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  export PITCHFORK_INTERVAL=1s

  create_pitchfork_toml <<EOF
[settings]
interval = "1s"

[settings.logs]
time_retention = "2s"

[daemons.retention_test]
run = 'bash $slowly_output 0.5 20'
EOF

  # Config should be parsed without error
  run pitchfork list
  assert_success

  # Daemon should start and produce logs normally
  run pitchfork start retention_test
  assert_success
  wait_for_log_lines retention_test 10

  # Retention runs at most once per hour, so we can't verify pruning in
  # a fast test. Just verify the config was accepted and logs exist.
  run pitchfork logs retention_test --raw --no-timestamp
  assert_output --partial "Output"

  pitchfork stop retention_test
}

@test "line retention setting is accepted in config" {
  require_sqlite3
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  export PITCHFORK_INTERVAL=1s

  create_pitchfork_toml <<EOF
[settings]
interval = "1s"

[settings.logs]
line_retention = 5

[daemons.line_retention_test]
run = 'bash $slowly_output 0.5 20'
EOF

  # Config should be parsed without error
  run pitchfork list
  assert_success

  # Daemon should start and produce logs normally
  run pitchfork start line_retention_test
  assert_success
  wait_for_log_lines line_retention_test 10

  # Retention runs at most once per hour, so we can't verify pruning in
  # a fast test. Just verify the config was accepted and logs exist.
  run pitchfork logs line_retention_test --raw --no-timestamp
  assert_output --partial "Output"

  pitchfork stop line_retention_test
}

@test "text log migration to SQLite ingests old format" {
  require_sqlite3

  # Create old-format log directory: logs/oldsvc/oldsvc.log (no "--" in name)
  # The migration expects timestamped log lines: "YYYY-MM-DD HH:MM:SS daemon_id message"
  local old_dir="$PITCHFORK_LOGS_DIR/oldsvc"
  mkdir -p "$old_dir"
  echo "2025-01-01 12:00:00 oldsvc line1" > "$old_dir/oldsvc.log"
  echo "2025-01-01 12:00:01 oldsvc line2" >> "$old_dir/oldsvc.log"

  # First call triggers directory rename; content may not yet be in SQLite.
  # A second call (or list) picks up the migrated new-format directory.
  PITCHFORK_LOG=error run pitchfork logs legacy/oldsvc --raw --no-timestamp
  # May be empty on first call if migration just renamed but hasn't ingested.
  # Run list to force a fresh log store init that picks up the new-format dir.
  PITCHFORK_LOG=error run pitchfork list
  PITCHFORK_LOG=error run pitchfork logs legacy/oldsvc --raw --no-timestamp
  assert_success
  assert_output --partial "line1"
  assert_output --partial "line2"

  # The old text log file should have been consumed/deleted
  [ ! -e "$old_dir/oldsvc.log" ]
}

@test "log entries maintain correct ordering" {
  require_sqlite3
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.order_test]
run = 'bash $slowly_output 0 5'
EOF

  run pitchfork start order_test
  assert_success

  wait_for_log_lines order_test 5

  run pitchfork logs order_test --raw --no-timestamp
  assert_output --partial "Output 1/5"
  assert_output --partial "Output 2/5"
  assert_output --partial "Output 3/5"
  assert_output --partial "Output 4/5"
  assert_output --partial "Output 5/5"

  local pos1 pos5
  pos1=$(echo "$output" | grep -n "Output 1/5" | head -1 | cut -d: -f1)
  pos5=$(echo "$output" | grep -n "Output 5/5" | head -1 | cut -d: -f1)
  [[ "$pos1" -lt "$pos5" ]]
}

@test "archive_hook setting is accepted in daemon config" {
  require_sqlite3
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  local marker="$TEST_TEMP_DIR/archive_marker"

  export PITCHFORK_INTERVAL=1s

  # archive_hook is a per-daemon override (string). The global setting
  # is [settings.logs.archive_hook] with a command sub-field.
  # Retention runs at most once per hour, so we can't trigger the hook
  # in a fast test. Just verify the config is accepted without error.
  create_pitchfork_toml <<EOF
[settings]
interval = "1s"

[daemons.archive_test]
run = 'bash $slowly_output 0.5 10'
time_retention = "1s"
archive_hook = "cat > /dev/null && echo archived >> $marker"
EOF

  run pitchfork list
  assert_success

  run pitchfork start archive_test
  assert_success
  wait_for_log_lines archive_test 3

  pitchfork stop archive_test
}

@test "SQLite database is created on first log write" {
  require_sqlite3
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.db_test]
run = 'bash $slowly_output 1 3'
EOF

  run pitchfork start db_test
  assert_success

  wait_for_log_lines db_test 1

  assert_file_exists "$PITCHFORK_LOGS_DIR/logs.db"

  local count
  count=$(query_logs_db "SELECT count(*) FROM log_entries")
  [[ "$count" -gt 0 ]]
}

@test "clear all logs removes all entries from database" {
  require_sqlite3
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.daemon1]
run = 'bash $slowly_output 1 5'

[daemons.daemon2]
run = 'bash $slowly_output 1 5'
EOF

  run pitchfork start daemon1
  assert_success
  run pitchfork start daemon2
  assert_success

  wait_for_log_lines daemon1 5
  wait_for_log_lines daemon2 5

  local count_before
  count_before=$(query_logs_db "SELECT count(*) FROM log_entries")
  [[ "$count_before" -gt 0 ]]

  run pitchfork logs --clear
  assert_success

  local count_after
  count_after=$(query_logs_db "SELECT count(*) FROM log_entries")
  [[ "$count_after" -eq 0 ]]

  PITCHFORK_LOG=error run pitchfork logs daemon1 --raw
  assert_output ""
  PITCHFORK_LOG=error run pitchfork logs daemon2 --raw
  assert_output ""
}

@test "SQLite log_entries has structured columns and level index" {
  require_sqlite3
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.schema_test]
run = 'bash $slowly_output 1 3'
EOF

  run pitchfork start schema_test
  assert_success
  wait_for_log_lines schema_test 1

  # Verify new columns exist
  local cols
  cols=$(query_logs_db "PRAGMA table_info(log_entries)" | awk -F'|' '{print $2}' | tr '\n' ',')
  [[ "$cols" == *"level"* ]]
  [[ "$cols" == *"msg"* ]]
  [[ "$cols" == *"logger"* ]]
  [[ "$cols" == *"fields_json"* ]]

  # Verify level index exists
  local idx
  idx=$(query_logs_db "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_daemon_level_ts'")
  [[ "$idx" == "idx_daemon_level_ts" ]]

  pitchfork stop schema_test
}
