#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  bats_require_minimum_version 1.5.0
  export PITCHFORK_INTERVAL=200ms
  _common_setup
}

teardown() {
  _common_teardown
}

@test "removed worktrees stop their daemons even with live sessions and leftover caches" {
  git init --quiet "$TEST_TEMP_DIR/repo"
  git -C "$TEST_TEMP_DIR/repo" -c user.name=Test -c user.email=test@example.com commit --quiet --allow-empty -m initial
  local worktree="$TEST_TEMP_DIR/linked"
  git -C "$TEST_TEMP_DIR/repo" worktree add --quiet --detach "$worktree"
  cat >"$worktree/pitchfork.toml" <<EOF
namespace = "linked"
[daemons.worker]
run = "sleep 120"
ready_delay = 0
retry = true
EOF
  cd "$worktree"
  run pitchfork project enter --pid $$
  assert_success
  run pitchfork start worker
  assert_success
  local pid
  pid="$(get_daemon_pid linked/worker)"
  wait_for_status linked/worker running

  cd "$TEST_TEMP_DIR/repo"
  run pitchfork run control --delay 0 -- sleep 120
  assert_success
  git worktree remove --force "$worktree"
  mkdir -p "$worktree/app/.vite"

  wait_for_status linked/worker stopped 15
  run pitchfork status linked/worker
  refute_output --partial "errored"
  if pid_alive "$pid"; then
    echo "Removed worktree's process is still alive: $pid" >&2
    return 1
  fi
  wait_for_status global/control running
}

@test "worktree identity survives supervisor restart and deletion of the working directory" {
  git init --quiet "$TEST_TEMP_DIR/repo"
  git -C "$TEST_TEMP_DIR/repo" -c user.name=Test -c user.email=test@example.com commit --quiet --allow-empty -m initial
  local worktree="$TEST_TEMP_DIR/linked"
  git -C "$TEST_TEMP_DIR/repo" worktree add --quiet --detach "$worktree"
  echo 'namespace = "linked"' >"$worktree/pitchfork.toml"
  cd "$worktree"
  run pitchfork run worker --delay 0 -- sleep 120
  assert_success
  wait_for_status linked/worker running

  local supervisor_pid
  supervisor_pid="$(get_daemon_pid global/pitchfork)"
  [[ -n "$supervisor_pid" ]]
  kill_pid "$supervisor_pid"
  sleep 1
  pitchfork supervisor start >/dev/null 2>&1
  wait_for_status linked/worker running
  cd "$TEST_TEMP_DIR/repo"
  git worktree remove --force "$worktree"
  wait_for_status linked/worker stopped 15
}

@test "an exited daemon is not retried from a removed worktree's cache directory" {
  git init --quiet "$TEST_TEMP_DIR/repo"
  git -C "$TEST_TEMP_DIR/repo" -c user.name=Test -c user.email=test@example.com commit --quiet --allow-empty -m initial
  local worktree="$TEST_TEMP_DIR/linked"
  git -C "$TEST_TEMP_DIR/repo" worktree add --quiet --detach "$worktree"
  cat >"$worktree/pitchfork.toml" <<EOF
namespace = "linked"
[daemons.worker]
run = "exec sleep 120"
ready_delay = 0
retry = true
EOF
  cd "$worktree"
  run pitchfork start worker
  assert_success
  local daemon_pid supervisor_pid
  daemon_pid="$(get_daemon_pid linked/worker)"
  supervisor_pid="$(get_daemon_pid global/pitchfork)"
  [[ -n "$daemon_pid" && -n "$supervisor_pid" ]]
  kill_pid "$supervisor_pid"
  kill_pid "$daemon_pid"
  sleep 1
  cd "$TEST_TEMP_DIR/repo"
  git worktree remove --force "$worktree"
  mkdir -p "$worktree/.vite"
  pitchfork supervisor start >/dev/null 2>&1
  wait_for_status linked/worker stopped 15
  sleep 1
  wait_for_status linked/worker stopped
}
