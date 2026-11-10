#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# Poll `pitchfork status` until it shows the expected activity.
_wait_for_activity() {
  local name="$1" needle="$2" timeout_secs="${3:-10}"
  for _ in $(seq 1 "$((timeout_secs * 5))"); do
    if pitchfork status "$name" 2>/dev/null | grep -q "Activity: $needle"; then
      return 0
    fi
    sleep 0.2
  done
  echo "Timed out waiting for '$name' to report: $needle" >&2
  pitchfork status "$name" >&2 || true
  return 1
}

# A report ends with ST, not a newline, so none is printed here: the status has
# to show up while the daemon is still quietly running.
@test "status shows a daemon's OSC 7501 report" {
  create_pitchfork_toml <<'EOF'
[daemons.reporter]
run = '''printf '\033]7501;state=working:progress=40:msg=U3luY2luZw==\033\\'; sleep 30'''
EOF

  run pitchfork start reporter
  assert_success

  _wait_for_activity reporter "working 40% Syncing"

  run pitchfork list
  assert_output --partial "working 40% Syncing"

  pitchfork stop reporter || true
}

@test "status shows a report from a pty daemon" {
  skip_on_windows "pty daemons are not supported on Windows"
  create_pitchfork_toml <<'EOF'
[daemons.pty_reporter]
run = '''printf '\033]7501;state=blocked:kind=question:msg=U3luY2luZw==\007'; sleep 30'''
pty = true
EOF

  run pitchfork start pty_reporter
  assert_success

  _wait_for_activity pty_reporter "blocked Syncing"

  pitchfork stop pty_reporter || true
}

@test "a finished report outlives the process and a live one does not" {
  create_pitchfork_toml <<'EOF'
[daemons.finisher]
run = '''printf '\033]7501;state=done:msg=QWxsIGRvbmU=\033\\'; sleep 2'''
ready_delay = 0

[daemons.dropper]
run = '''printf '\033]7501;state=working\033\\'; sleep 2'''
ready_delay = 0
EOF

  pitchfork start finisher
  pitchfork start dropper
  _wait_for_activity finisher "done All done"
  _wait_for_activity dropper "working"

  # Both exit after two seconds.
  for _ in $(seq 1 50); do
    pitchfork status dropper 2>/dev/null | grep -q "Activity:" || break
    sleep 0.2
  done

  run pitchfork status dropper
  refute_output --partial "Activity:"
  run pitchfork status finisher
  assert_output --partial "Activity: done All done"
}
