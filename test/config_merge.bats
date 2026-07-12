#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

@test ".config/pitchfork.toml is discovered and merged" {
  mkdir -p .config
  cat > .config/pitchfork.toml <<EOF
[daemons.config_daemon]
run = "sleep 10"
EOF

  run pitchfork daemons
  assert_success
  assert_output --partial "config_daemon"
}

@test ".config/pitchfork.local.toml overrides .config/pitchfork.toml" {
  mkdir -p .config
  cat > .config/pitchfork.toml <<EOF
[daemons.override_test]
run = "sleep 10"
retry = 1
EOF
  cat > .config/pitchfork.local.toml <<EOF
[daemons.override_test]
run = "sleep 20"
retry = 5
EOF

  run pitchfork daemons --json
  assert_success
  assert_output --partial "sleep 20"

  run cat .config/pitchfork.local.toml
  assert_output --partial 'retry = 5'

  run cat .config/pitchfork.toml
  assert_output --partial 'retry = 1'
}

@test "pitchfork.toml overrides .config/pitchfork.toml" {
  mkdir -p .config
  cat > .config/pitchfork.toml <<EOF
[daemons.override_test]
run = "sleep 10"
EOF
  create_pitchfork_toml <<EOF
[daemons.override_test]
run = "sleep 30"
EOF

  run pitchfork daemons --json
  assert_success
  assert_output --partial "sleep 30"
}

@test "pitchfork.local.toml overrides pitchfork.toml" {
  create_pitchfork_toml <<EOF
[daemons.override_test]
run = "sleep 10"
EOF
  cat > pitchfork.local.toml <<EOF
[daemons.override_test]
run = "sleep 40"
EOF

  run pitchfork daemons --json
  assert_success
  assert_output --partial "sleep 40"
}

@test "configs from parent directory are discovered" {
  create_pitchfork_toml <<EOF
[daemons.parent_daemon]
run = "sleep 10"
EOF
  mkdir -p subdir
  cd subdir

  run pitchfork daemons
  assert_success
  assert_output --partial "parent_daemon"
}

@test "namespace collision between two directories with same name" {
  # Two discovered config files whose parent directories share the same basename
  # but live at different paths in the cwd hierarchy.
  mkdir -p proj/proj
  cat > proj/pitchfork.toml <<EOF
[daemons.a]
run = "sleep 10"
EOF
  cat > proj/proj/pitchfork.toml <<EOF
[daemons.b]
run = "sleep 10"
EOF
  cd proj/proj

  run pitchfork daemons
  assert_failure
  assert_output --partial "collision" || assert_output --partial "namespace"
}

@test "pitchfork.local.toml requires sibling pitchfork.toml" {
  cat > pitchfork.local.toml <<EOF
[daemons.local_only]
run = "sleep 10"
EOF

  run pitchfork list
  assert_success
  assert_output --partial "local_only"
}

@test "sibling local.toml with mismatched namespace is rejected" {
  create_pitchfork_toml <<EOF
namespace = "team1"

[daemons.a]
run = "sleep 10"
EOF
  cat > pitchfork.local.toml <<EOF
namespace = "team2"

[daemons.a]
run = "sleep 20"
EOF

  # `daemons` reads config directly without going through the supervisor,
  # so the parse error is surfaced in its output.
  run pitchfork daemons
  assert_failure
  assert_output --partial "namespace" || assert_output --partial "does not match"
}

@test "explicit namespace override works with valid directory name" {
  mkdir -p myproject
  cat > myproject/pitchfork.toml <<EOF
namespace = "custom-ns"

[daemons.mydaemon]
run = "sleep 10"
EOF
  cd myproject

  run pitchfork daemons
  assert_success
  assert_output --partial "custom-ns"
}

@test "[groups] section allows group-based operations" {
  create_pitchfork_toml <<EOF
[daemons.web1]
run = "sleep 60"
ready_delay = 1

[daemons.web2]
run = "sleep 60"
ready_delay = 1

[groups.web]
daemons = ["web1", "web2"]
EOF

  run pitchfork start --group web
  assert_success
  wait_for_status web1 running
  wait_for_status web2 running

  run pitchfork list
  assert_output --partial "web1"
  assert_output --partial "web2"
}

@test "start with unknown group gives clear error" {
  run pitchfork start --group nonexistent
  assert_failure
  assert_output --partial "nonexistent"
}

@test "groups section is preserved in config round-trip" {
  create_pitchfork_toml <<EOF
[daemons.worker]
run = "sleep 60"
ready_delay = 1

[groups.backend]
daemons = ["worker"]
EOF

  run pitchfork daemons
  assert_success
  assert_output --partial "worker"

  run pitchfork start --group backend
  assert_success
  wait_for_status worker running
}
