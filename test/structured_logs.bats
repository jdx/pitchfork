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

# ============================================================================
# Group A: JSON log format parsing
# ============================================================================

@test "json log_format extracts level msg logger from JSON lines" {
  require_sqlite3

  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"info","msg":"startup complete","logger":"main"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.json_parse]
run = "bash $PWD/emit.sh"
ready_output = "startup complete"

[daemons.json_parse.logs]
log_format = "json"
EOF

  pitchfork start json_parse
  wait_for_logs json_parse "startup complete" 10

  local level logger fields
  level="$(query_logs_db "SELECT level FROM log_entries WHERE daemon_id LIKE '%/json_parse' LIMIT 1")"
  logger="$(query_logs_db "SELECT logger FROM log_entries WHERE daemon_id LIKE '%/json_parse' LIMIT 1")"
  fields="$(query_logs_db "SELECT fields_json FROM log_entries WHERE daemon_id LIKE '%/json_parse' LIMIT 1")"

  [[ "$level" == "info" ]]
  [[ "$logger" == "main" ]]
  [[ "$fields" == *"startup complete"* ]]

  run pitchfork logs json_parse --raw --no-timestamp
  assert_success
  [[ "$output" == *'{"level":"info","msg":"startup complete","logger":"main"}'* ]]

  pitchfork stop json_parse
}

@test "json log_format falls back to plain text on invalid JSON" {
  require_sqlite3

  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'not_valid_json'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.json_fallback]
run = "bash $PWD/emit.sh"
ready_output = "not_valid_json"

[daemons.json_fallback.logs]
log_format = "json"
EOF

  pitchfork start json_fallback
  wait_for_logs json_fallback "not_valid_json" 10

  local level fields
  level="$(query_logs_db "SELECT level FROM log_entries WHERE daemon_id LIKE '%/json_fallback' LIMIT 1")"
  fields="$(query_logs_db "SELECT fields_json FROM log_entries WHERE daemon_id LIKE '%/json_fallback' LIMIT 1")"

  [[ -z "$level" ]]
  [[ -z "$fields" ]]

  run pitchfork logs json_fallback --raw --no-timestamp
  assert_success
  [[ "$output" == *"not_valid_json"* ]]

  pitchfork stop json_fallback
}

@test "json log_format normalizes FATAL to error level" {
  require_sqlite3

  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"FATAL","msg":"crash"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.json_fatal]
run = "bash $PWD/emit.sh"
ready_output = "crash"

[daemons.json_fatal.logs]
log_format = "json"
EOF

  pitchfork start json_fatal
  wait_for_logs json_fatal "crash" 10

  local level
  level="$(query_logs_db "SELECT level FROM log_entries WHERE daemon_id LIKE '%/json_fatal' LIMIT 1")"
  [[ "$level" == "error" ]]

  pitchfork stop json_fatal
}

@test "json log_format normalizes pino integer level 50 to error" {
  require_sqlite3

  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":50,"msg":"oops"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.json_pino]
run = "bash $PWD/emit.sh"
ready_output = "oops"

[daemons.json_pino.logs]
log_format = "json"
EOF

  pitchfork start json_pino
  wait_for_logs json_pino "oops" 10

  local level
  level="$(query_logs_db "SELECT level FROM log_entries WHERE daemon_id LIKE '%/json_pino' LIMIT 1")"
  [[ "$level" == "error" ]]

  pitchfork stop json_pino
}

# ============================================================================
# Group B: logfmt parsing
# ============================================================================

@test "logfmt log_format extracts fields from key=value pairs" {
  require_sqlite3

  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'level=info msg="server started" logger=api'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.logfmt_parse]
run = "bash $PWD/emit.sh"
ready_output = "server started"

[daemons.logfmt_parse.logs]
log_format = "logfmt"
EOF

  pitchfork start logfmt_parse
  wait_for_logs logfmt_parse "server started" 10

  local level logger
  level="$(query_logs_db "SELECT level FROM log_entries WHERE daemon_id LIKE '%/logfmt_parse' LIMIT 1")"
  logger="$(query_logs_db "SELECT logger FROM log_entries WHERE daemon_id LIKE '%/logfmt_parse' LIMIT 1")"

  [[ "$level" == "info" ]]
  [[ "$logger" == "api" ]]

  pitchfork stop logfmt_parse
}

@test "logfmt log_format handles quoted values with spaces" {
  require_sqlite3

  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'level=error msg="something went wrong" request_id=req_123'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.logfmt_quoted]
run = "bash $PWD/emit.sh"
ready_output = "something went wrong"

[daemons.logfmt_quoted.logs]
log_format = "logfmt"
EOF

  pitchfork start logfmt_quoted
  wait_for_logs logfmt_quoted "something went wrong" 10

  local level fields
  level="$(query_logs_db "SELECT level FROM log_entries WHERE daemon_id LIKE '%/logfmt_quoted' LIMIT 1")"
  fields="$(query_logs_db "SELECT fields_json FROM log_entries WHERE daemon_id LIKE '%/logfmt_quoted' LIMIT 1")"

  [[ "$level" == "error" ]]
  [[ "$fields" == *"request_id"* ]]
  [[ "$fields" == *"req_123"* ]]

  pitchfork stop logfmt_quoted
}

# ============================================================================
# Group C: --level filter
# ============================================================================

@test "logs --level filters by normalized level" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"info","msg":"info_msg"}' '{"level":"error","msg":"error_msg"}' '{"level":"warn","msg":"warn_msg"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.level_filter]
run = "bash $PWD/emit.sh"
ready_output = "warn_msg"

[daemons.level_filter.logs]
log_format = "json"
EOF

  pitchfork start level_filter
  wait_for_logs level_filter "warn_msg" 10

  PITCHFORK_LOG=error run pitchfork logs level_filter --level error --raw --no-timestamp
  assert_success
  [[ "$output" == *'"msg":"error_msg"'* ]]
  [[ "$output" != *'"msg":"info_msg"'* ]]
  [[ "$output" != *'"msg":"warn_msg"'* ]]

  pitchfork stop level_filter
}

@test "logs --level is case-insensitive" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"info","msg":"info_msg"}' '{"level":"WARN","msg":"warn_msg"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.level_case]
run = "bash $PWD/emit.sh"
ready_output = "warn_msg"

[daemons.level_case.logs]
log_format = "json"
EOF

  pitchfork start level_case
  wait_for_logs level_case "warn_msg" 10

  PITCHFORK_LOG=error run pitchfork logs level_case --level warn --raw --no-timestamp
  assert_success
  [[ "$output" == *'"msg":"warn_msg"'* ]]
  [[ "$output" != *'"msg":"info_msg"'* ]]

  pitchfork stop level_case
}

@test "logs --level shows entries at or above the threshold" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"trace","msg":"trace_msg"}' '{"level":"debug","msg":"debug_msg"}' '{"level":"info","msg":"info_msg"}' '{"level":"warn","msg":"warn_msg"}' '{"level":"error","msg":"error_msg"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.level_threshold]
run = "bash $PWD/emit.sh"
ready_output = "error_msg"

[daemons.level_threshold.logs]
log_format = "json"
EOF

  pitchfork start level_threshold
  wait_for_logs level_threshold "error_msg" 10

  # --level warn shows warn and error, excludes info/debug/trace
  PITCHFORK_LOG=error run pitchfork logs level_threshold --level warn --raw --no-timestamp
  assert_success
  [[ "$output" == *'"msg":"warn_msg"'* ]]
  [[ "$output" == *'"msg":"error_msg"'* ]]
  [[ "$output" != *'"msg":"info_msg"'* ]]
  [[ "$output" != *'"msg":"debug_msg"'* ]]
  [[ "$output" != *'"msg":"trace_msg"'* ]]

  # --level info shows info, warn, error
  PITCHFORK_LOG=error run pitchfork logs level_threshold --level info --raw --no-timestamp
  assert_success
  [[ "$output" == *'"msg":"info_msg"'* ]]
  [[ "$output" == *'"msg":"warn_msg"'* ]]
  [[ "$output" == *'"msg":"error_msg"'* ]]
  [[ "$output" != *'"msg":"debug_msg"'* ]]
  [[ "$output" != *'"msg":"trace_msg"'* ]]

  pitchfork stop level_threshold
}

@test "logs --level with invalid value gives error" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"info","msg":"info_msg"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.level_invalid]
run = "bash $PWD/emit.sh"
ready_output = "info_msg"

[daemons.level_invalid.logs]
log_format = "json"
EOF

  pitchfork start level_invalid
  wait_for_logs level_invalid "info_msg" 10

  run pitchfork logs level_invalid --level bogus --raw
  assert_failure
  [[ "$output" == *"invalid level"* || "$output" == *"valid level"* ]]

  pitchfork stop level_invalid
}

# ============================================================================
# Group D: --field filter
# ============================================================================

@test "logs --field filters by structured field value" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"info","msg":"first_msg","request_id":"req_1"}' '{"level":"info","msg":"second_msg","request_id":"req_2"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.field_filter]
run = "bash $PWD/emit.sh"
ready_output = "second_msg"

[daemons.field_filter.logs]
log_format = "json"
EOF

  pitchfork start field_filter
  wait_for_logs field_filter "second_msg" 10

  PITCHFORK_LOG=error run pitchfork logs field_filter --field request_id=req_1 --raw --no-timestamp
  assert_success
  [[ "$output" == *'"msg":"first_msg"'* ]]
  [[ "$output" != *'"msg":"second_msg"'* ]]

  pitchfork stop field_filter
}

@test "logs --field with multiple values uses AND logic" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"method":"GET","path":"/error","msg":"get_error_msg"}' '{"method":"POST","path":"/error","msg":"post_error_msg"}' '{"method":"GET","path":"/ok","msg":"get_ok_msg"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.field_and]
run = "bash $PWD/emit.sh"
ready_output = "get_ok_msg"

[daemons.field_and.logs]
log_format = "json"
EOF

  pitchfork start field_and
  wait_for_logs field_and "get_ok_msg" 10

  PITCHFORK_LOG=error run pitchfork logs field_and --field method=GET --field path=/error --raw --no-timestamp
  assert_success
  [[ "$output" == *'"msg":"get_error_msg"'* ]]
  [[ "$output" != *'"msg":"post_error_msg"'* ]]
  [[ "$output" != *'"msg":"get_ok_msg"'* ]]

  pitchfork stop field_and
}

@test "logs --field rejects malformed KEY=VALUE" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'plain'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.field_malformed]
run = "bash $PWD/emit.sh"
ready_output = "plain"
EOF

  pitchfork start field_malformed
  wait_for_logs field_malformed "plain" 10

  run pitchfork logs field_malformed --field "no_equals_sign" --raw
  assert_failure

  pitchfork stop field_malformed
}

# ============================================================================
# Group E: --jq filter
# ============================================================================

@test "logs --jq filters by structured level field" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"info","msg":"info_msg"}' '{"level":"error","msg":"err_msg"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.jq_level]
run = "bash $PWD/emit.sh"
ready_output = "err_msg"

[daemons.jq_level.logs]
log_format = "json"
EOF

  pitchfork start jq_level
  wait_for_logs jq_level "err_msg" 10

  PITCHFORK_LOG=error run pitchfork logs jq_level --jq '.level == "error"' --raw --no-timestamp
  assert_success
  [[ "$output" == *'"msg":"err_msg"'* ]]
  [[ "$output" != *'"msg":"info_msg"'* ]]

  pitchfork stop jq_level
}

@test "logs --jq can access numeric fields" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"info","msg":"low_msg","status":200}' '{"level":"info","msg":"high_msg","status":500}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.jq_numeric]
run = "bash $PWD/emit.sh"
ready_output = "high_msg"

[daemons.jq_numeric.logs]
log_format = "json"
EOF

  pitchfork start jq_numeric
  wait_for_logs jq_numeric "high_msg" 10

  PITCHFORK_LOG=error run pitchfork logs jq_numeric --jq '.fields.status >= 500' --raw --no-timestamp
  assert_success
  [[ "$output" == *'"msg":"high_msg"'* ]]
  [[ "$output" != *'"msg":"low_msg"'* ]]

  pitchfork stop jq_numeric
}

@test "logs --jq with invalid expression gives error" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"info","msg":"a"}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.jq_invalid]
run = "bash $PWD/emit.sh"
ready_output = "a"

[daemons.jq_invalid.logs]
log_format = "json"
EOF

  pitchfork start jq_invalid
  wait_for_logs jq_invalid "a" 10

  run pitchfork logs jq_invalid --jq '.invalid syntax !!!' --raw
  assert_failure
  [[ "$output" == *"jq"* || "$output" == *"parse"* || "$output" == *"syntax"* ]]

  pitchfork stop jq_invalid
}

# ============================================================================
# Group F: --json output
# ============================================================================

@test "logs --json outputs valid JSON array with structured fields" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"info","msg":"hello","logger":"main","port":8080}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.json_out]
run = "bash $PWD/emit.sh"
ready_output = "hello"

[daemons.json_out.logs]
log_format = "json"
EOF

  pitchfork start json_out
  wait_for_logs json_out "hello" 10

  run bash -c 'pitchfork logs json_out --json 2>/dev/null | python3 -m json.tool'
  assert_success
  [[ "$output" == *"\"timestamp\""* ]]
  [[ "$output" == *"\"daemon_id\""* ]]
  [[ "$output" == *"\"message\""* ]]
  [[ "$output" == *"\"level\": \"info\""* ]]
  [[ "$output" == *"\"msg\": \"hello\""* ]]
  [[ "$output" == *"\"logger\": \"main\""* ]]
  [[ "$output" == *"\"fields\""* ]]

  pitchfork stop json_out
}

@test "logs --json on plain text daemon omits structured fields" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'plain text line'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.plain_out]
run = "bash $PWD/emit.sh"
ready_output = "plain text line"
EOF

  pitchfork start plain_out
  wait_for_logs plain_out "plain text line" 10

  run bash -c 'pitchfork logs plain_out --json 2>/dev/null | python3 -m json.tool'
  assert_success
  [[ "$output" == *"\"timestamp\""* ]]
  [[ "$output" == *"\"message\""* ]]
  [[ "$output" != *"\"level\""* ]]
  [[ "$output" != *"\"msg\""* ]]
  [[ "$output" != *"\"logger\""* ]]
  [[ "$output" != *"\"fields\""* ]]

  pitchfork stop plain_out
}

# ============================================================================
# Group G: --tail with new filters
# ============================================================================

@test "logs --tail with --level only streams matching new lines" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
while true; do
  printf '%s\n' '{"level":"info","msg":"info_msg"}' '{"level":"error","msg":"error_msg"}'
  sleep 1
done
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.tail_level]
run = "bash $PWD/emit.sh"
ready_output = "error_msg"

[daemons.tail_level.logs]
log_format = "json"
EOF

  pitchfork start tail_level
  wait_for_logs tail_level "error_msg" 10

  local output
  output="$(timeout 3 pitchfork logs tail_level --tail --level error --raw --no-timestamp 2>&1)" || true
  [[ "$output" == *"error_msg"* ]]
  [[ "$output" != *"info_msg"* ]]

  pitchfork stop tail_level
}

@test "logs --tail with --jq only streams matching entries" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
while true; do
  printf '%s\n' '{"status":200,"msg":"ok_msg"}' '{"status":500,"msg":"bad_msg"}'
  sleep 1
done
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.tail_jq]
run = "bash $PWD/emit.sh"
ready_output = "ok_msg"

[daemons.tail_jq.logs]
log_format = "json"
EOF

  pitchfork start tail_jq
  wait_for_logs tail_jq "ok_msg" 10

  local output
  output="$(timeout 3 pitchfork logs tail_jq --tail --jq '.fields.status == 200' --raw --no-timestamp 2>&1)" || true
  [[ "$output" == *"ok_msg"* ]]
  [[ "$output" != *"bad_msg"* ]]

  pitchfork stop tail_jq
}

# ============================================================================
# Group H: --jq + --level combination
# ============================================================================

@test "logs --jq composes with --level (SQL prefilter then jq postfilter)" {
  cat > "$PWD/emit.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '{"level":"error","msg":"not_found_msg","status":404}' '{"level":"error","msg":"server_error_msg","status":503}' '{"level":"info","msg":"info_msg","status":500}'
sleep 3600
EOF
  chmod +x "$PWD/emit.sh"

  create_pitchfork_toml <<EOF
[daemons.jq_level_combo]
run = "bash $PWD/emit.sh"
ready_output = "info_msg"

[daemons.jq_level_combo.logs]
log_format = "json"
EOF

  pitchfork start jq_level_combo
  wait_for_logs jq_level_combo "info_msg" 10

  PITCHFORK_LOG=error run pitchfork logs jq_level_combo --level error --jq '.fields.status >= 500' --raw --no-timestamp
  assert_success
  [[ "$output" == *'"msg":"server_error_msg"'* ]]
  [[ "$output" != *'"msg":"not_found_msg"'* ]]
  [[ "$output" != *'"msg":"info_msg"'* ]]

  pitchfork stop jq_level_combo
}
