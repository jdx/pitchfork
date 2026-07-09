#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# ============================================================================
# Group A: settings list/get
# ============================================================================

@test "settings list shows all settings" {
  run pitchfork settings list
  assert_success
  assert_output --partial "general.interval"
  assert_output --partial "general.autostop_delay"
  assert_output --partial "supervisor.watch_interval"
}

@test "settings list with --group filters by group" {
  run pitchfork settings list --group supervisor
  assert_success
  assert_output --partial "supervisor.watch_interval"
  refute_output --partial "autostop_delay"
}

@test "settings list --json outputs valid JSON" {
  run bash -c 'pitchfork settings list --json 2>/dev/null | python3 -m json.tool'
  assert_success
  assert_output --partial "general.interval"
  assert_output --partial "general.autostop_delay"
  assert_output --partial "supervisor.watch_interval"
}

@test "settings get returns value for known key" {
  run pitchfork settings get general.interval
  assert_success
  assert_output --partial "10s"
}

@test "settings get unknown key gives error" {
  run pitchfork settings get nonexistent.key
  assert_failure
  assert_output --partial "unknown"
}

@test "settings get similar key gives suggestion" {
  run pitchfork settings get general.interva
  assert_failure
  assert_output --partial "Did you mean"
}

# ============================================================================
# Group B: settings set
# ============================================================================

@test "settings set writes to project config by default" {
  run pitchfork settings set general.interval 5s
  assert_success
  assert_file_contains "pitchfork.toml" 'interval = "5s"'
  assert_file_contains "pitchfork.toml" "\[settings\.general\]"
}

@test "settings set --global writes to user config" {
  run pitchfork settings set general.interval 5s --global
  assert_success
  local global_config="$HOME/.config/pitchfork/config.toml"
  assert_file_contains "$global_config" 'interval = "5s"'
  assert_file_contains "$global_config" "\[settings\.general\]"
  assert [ ! -e pitchfork.toml ]
}

@test "settings set --local writes to pitchfork.local.toml" {
  run pitchfork settings set general.interval 5s --local
  assert_success
  assert_file_contains "pitchfork.local.toml" 'interval = "5s"'
  assert_file_contains "pitchfork.local.toml" "\[settings\.general\]"
  assert [ ! -e pitchfork.toml ]
}

@test "settings set --project explicitly writes to pitchfork.toml" {
  run pitchfork settings set general.interval 5s --project
  assert_success
  assert_file_contains "pitchfork.toml" 'interval = "5s"'
  assert_file_contains "pitchfork.toml" "\[settings\.general\]"
  assert [ ! -e pitchfork.local.toml ]
}

@test "settings set invalid bool value is rejected" {
  run pitchfork settings set general.mise notabool
  assert_failure
  assert_output --partial "invalid boolean"
}

@test "settings set invalid duration value is rejected" {
  run pitchfork settings set general.interval notaduration
  assert_failure
  assert_output --partial "invalid duration"
}

@test "settings set invalid integer value is rejected" {
  run pitchfork settings set supervisor.port_bump_attempts notanumber
  assert_failure
  assert_output --partial "invalid integer"
}

@test "settings set then get returns the new value" {
  run pitchfork settings set general.interval 7s
  assert_success
  run pitchfork settings get general.interval
  assert_success
  assert_output --partial "7s"
}

# ============================================================================
# Group C: config precedence
# ============================================================================

@test "project config overrides user config" {
  mkdir -p "$HOME/.config/pitchfork"
  cat > "$HOME/.config/pitchfork/config.toml" <<EOF
[settings.general]
interval = "5s"
EOF
  create_pitchfork_toml <<EOF
[settings.general]
interval = "3s"
EOF
  run pitchfork settings get general.interval
  assert_success
  assert_output --partial "3s"
}

@test "local config overrides project config" {
  create_pitchfork_toml <<EOF
[settings.general]
interval = "5s"
EOF
  cat > pitchfork.local.toml <<EOF
[settings.general]
interval = "2s"
EOF
  run pitchfork settings get general.interval
  assert_success
  assert_output --partial "2s"
}

@test "three-tier merge: user > project > local" {
  mkdir -p "$HOME/.config/pitchfork"
  cat > "$HOME/.config/pitchfork/config.toml" <<EOF
[settings.general]
interval = "9s"
EOF
  create_pitchfork_toml <<EOF
[settings.general]
interval = "5s"
EOF
  cat > pitchfork.local.toml <<EOF
[settings.general]
interval = "2s"
EOF
  run pitchfork settings get general.interval
  assert_success
  assert_output --partial "2s"
}

@test "settings from env var are reflected" {
  create_pitchfork_toml <<EOF
[settings.general]
interval = "5s"
EOF
  export PITCHFORK_INTERVAL=3s
  run pitchfork settings get general.interval
  unset PITCHFORK_INTERVAL
  assert_success
  assert_output --partial "3s"
}

# ============================================================================
# Group D: config validation
# ============================================================================

@test "invalid TOML syntax gives diagnostic error" {
  echo "[invalid toml [[" > pitchfork.toml
  run pitchfork list
  assert_failure
  assert_output --partial "parse"
}

@test "empty pitchfork.toml is valid" {
  echo "" > pitchfork.toml
  run pitchfork list
  assert_success
}

@test "daemon with double-dash name is rejected" {
  create_pitchfork_toml <<EOF
[daemons.bad--name]
run = "sleep 10"
EOF
  run pitchfork daemons
  assert_failure
  assert_output --partial "invalid daemon name"
  assert_output --partial "'--'"
}

@test "daemon name with spaces is rejected" {
  create_pitchfork_toml <<EOF
[daemons."bad name"]
run = "sleep 10"
EOF
  run pitchfork daemons
  assert_failure
  assert_output --partial "invalid daemon name"
  assert_output --partial "spaces"
}
