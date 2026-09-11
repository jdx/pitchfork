#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
  mkdir -p "$TEST_TEMP_DIR/project/sub" "$TEST_TEMP_DIR/generated" "$TEST_TEMP_DIR/other"
  export EXTRA="$TEST_TEMP_DIR/generated/pitchfork.toml"
  cat > "$EXTRA" <<'TOML'
[daemons.external]
run = "pwd; exec sleep 60"
ready_delay = 0
TOML
  cd "$TEST_TEMP_DIR/project"
}

teardown() {
  _common_teardown
}

@test "external files attach idempotently, resolve project cwd, and survive namespace registration" {
  run pitchfork config add "$EXTRA"
  assert_success
  run pitchfork config add "$EXTRA"
  assert_success
  run pitchfork config list --json
  assert_success
  assert_output --partial '"namespace": "project"'
  assert_equal "$(jq -r '.[0].config[0] | gsub("\\\\"; "/") | endswith("/generated/pitchfork.toml")' <<< "$output")" true
  cd sub
  run pitchfork daemons
  assert_success
  assert_output --partial "external"
  run pitchfork start external
  assert_success
  wait_for_logs "project/external" "/project" 10
  run pitchfork config list --json
  assert_success
  assert_equal "$(jq -r '.[0].config[0] | gsub("\\\\"; "/") | endswith("/generated/pitchfork.toml")' <<< "$output")" true
  cd "$TEST_TEMP_DIR/other"
  run pitchfork daemons
  assert_success
  refute_output --partial "external"
  run pitchfork status project/external
  assert_success
  assert_output --partial "running"
  run pitchfork stop project/external
  assert_success
}

@test "extra config overrides project definitions and inherits explicit namespace" {
  cat > pitchfork.toml <<'TOML'
namespace = "explicit"
[daemons.external]
run = "false"
TOML
  run pitchfork config add "$EXTRA"
  assert_success
  run pitchfork start external
  assert_success
  run pitchfork status explicit/external
  assert_success
  assert_output --partial "running"
  run pitchfork config add "$EXTRA" --dir "$TEST_TEMP_DIR/other"
  assert_failure
}

@test "remove works after deletion and daemon editing avoids the attached file" {
  run pitchfork config add "$EXTRA"
  assert_success
  run pitchfork daemons add local --run 'sleep 1'
  assert_success
  assert_file_exists "$TEST_TEMP_DIR/project/pitchfork.toml"
  rm "$EXTRA"
  run pitchfork config remove "$EXTRA"
  assert_success
  run pitchfork config list --json
  assert_success
  refute_output --partial "$EXTRA"
}

@test "one-shot config lists and starts without registering or leaking into new supervisor" {
  pitchfork supervisor stop
  export PITCHFORK_CONFIG="$EXTRA"
  run pitchfork daemons
  assert_success
  assert_output --partial "external"
  run pitchfork start external
  assert_success
  unset PITCHFORK_CONFIG
  run pitchfork config list --json
  assert_success
  assert_output '[]'
  run pitchfork stop project/external
  assert_success
}

@test "existing supervisor discovers registered cron daemon from unrelated cwd" {
  cat > "$EXTRA" <<'TOML'
[daemons.external]
run = "echo extra-cron"
cron = "* * * * * *"
TOML
  run pitchfork config add "$EXTRA"
  assert_success
  cd "$TEST_TEMP_DIR/other"
  wait_for_logs "project/external" "extra-cron" 20
}

@test "project enter uses external auto-start definition" {
  cat >> "$EXTRA" <<'TOML'
auto = ["start", "stop"]
TOML
  run pitchfork config add "$EXTRA"
  assert_success
  cd "$TEST_TEMP_DIR/other"
  run pitchfork project enter --pid $$ --directory "$TEST_TEMP_DIR/project"
  assert_success
  run pitchfork status project/external
  assert_success
  assert_output --partial "running"
  run pitchfork project leave --pid $$ --directory "$TEST_TEMP_DIR/project"
  assert_success
}

@test "registered settings and rewritten files are reloaded by the supervisor" {
  cat >> "$EXTRA" <<'TOML'
[settings.general]
ready_delay = "0s"
TOML
  run pitchfork config add "$EXTRA"
  assert_success
  run pitchfork settings get general.ready_delay
  assert_success
  assert_output --partial "0"
  run pitchfork start external
  assert_success
  cat > "$EXTRA" <<'TOML'
[daemons.external]
run = "echo replacement; exec sleep 60"
ready_delay = 0
[daemons.external.hooks]
on_stop = "echo hook-ran > hook-result"
TOML
  cd "$TEST_TEMP_DIR/other"
  run pitchfork restart project/external
  assert_success
  wait_for_logs "project/external" "replacement" 10
  run pitchfork stop project/external
  assert_success
  for _ in {1..50}; do
    [[ -f "$TEST_TEMP_DIR/project/hook-result" ]] && break
    sleep 0.1
  done
  assert_file_exists "$TEST_TEMP_DIR/project/hook-result"
}

@test "explicit attachment namespace works without an ordinary config and validates dot-config namespaces" {
  run pitchfork config add "$EXTRA" --namespace generated
  assert_success
  run pitchfork start generated/external
  assert_success
  run pitchfork config remove "$EXTRA"
  assert_success
  mkdir -p .config
  echo 'namespace = "native"' > .config/pitchfork.toml
  run pitchfork config add "$EXTRA" --namespace conflicting
  assert_failure
  run pitchfork config add "$EXTRA"
  assert_success
  run pitchfork config list --json
  assert_success
  assert_output --partial '"namespace": "native"'
}

@test "ancestor attachments do not replace a nested project's namespace" {
  run pitchfork config add "$EXTRA"
  assert_success
  cat > sub/pitchfork.toml <<'TOML'
namespace = "nested"
[daemons.external]
run = "pwd; exec sleep 60"
ready_delay = 0
TOML
  cd sub
  run pitchfork start external
  assert_success
  run pitchfork status nested/external
  assert_success
  assert_output --partial "running"
  wait_for_logs "nested/external" "/project/sub" 10
  run pitchfork start project/external
  assert_success
  wait_for_logs "project/external" "/project" 10
}

@test "environment attachments inherit the nearest ordinary project from a subdirectory" {
  cat > pitchfork.toml <<'TOML'
namespace = "ordinary"
[daemons.external]
run = "false"
TOML
  cd sub
  export PITCHFORK_CONFIG="$EXTRA"
  run pitchfork start external
  assert_success
  run pitchfork status ordinary/external
  assert_success
  assert_output --partial "running"
  wait_for_logs "ordinary/external" "/project" 10
}
