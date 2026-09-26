#!/usr/bin/env bats

setup() {
  bats_require_minimum_version 1.5.0
  load test_helper/common_setup
  export PITCHFORK_INTERVAL=1s
  export PITCHFORK_CRON_CHECK_INTERVAL=1s
  _common_setup
}

teardown() {
  _common_teardown
}

# ============================================================================
# Cron retrigger tests with failing tasks
# ============================================================================

# bats test_tags=slow
@test "cron finish retrigger with failing task runs at least twice" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_finish_fail]
run = 'bash "$fail_script" 0'
retry = 0

[daemons.cron_finish_fail.cron]
schedule = "* * * * * *"
retrigger = "finish"
immediate = true
EOF

  # The task fails immediately, so starting it reports failure. What this test
  # is about is what the cron schedule does next.
  run pitchfork start cron_finish_fail
  assert_failure

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_finish_fail --raw 2>/dev/null | grep -c "Failed after 0!" || true)
    [[ "$count" -ge 2 ]] && break
    sleep 2
  done

  [[ "$count" -ge 2 ]]
}

# bats test_tags=slow
@test "cron always retrigger with failing task runs at least twice" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_always_fail]
run = 'bash "$fail_script" 0'
retry = 0

[daemons.cron_always_fail.cron]
schedule = "* * * * * *"
retrigger = "always"
immediate = true
EOF

  # The task fails immediately, so starting it reports failure. What this test
  # is about is what the cron schedule does next.
  run pitchfork start cron_always_fail
  assert_failure

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_always_fail --raw 2>/dev/null | grep -c "Failed after 0!" || true)
    [[ "$count" -ge 2 ]] && break
    sleep 2
  done

  [[ "$count" -ge 2 ]]
}

# bats test_tags=slow
@test "cron success retrigger with failing task runs only once" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_success_fail]
run = 'bash "$fail_script" 0'
retry = 0

[daemons.cron_success_fail.cron]
schedule = "* * * * * *"
retrigger = "success"
immediate = true
EOF

  # The task fails immediately, so starting it reports failure. What this test
  # is about is what the cron schedule does next.
  run pitchfork start cron_success_fail
  assert_failure

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_success_fail --raw 2>/dev/null | grep -c "Failed after 0!" || true)
    [[ "$count" -ge 1 ]] && break
    sleep 2
  done

  [[ "$count" -eq 1 ]]
}

# bats test_tags=slow
@test "cron fail retrigger with failing task runs at least twice" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_fail_fail]
run = 'bash "$fail_script" 0'
retry = 0

[daemons.cron_fail_fail.cron]
schedule = "* * * * * *"
retrigger = "fail"
immediate = true
EOF

  # The task fails immediately, so starting it reports failure. What this test
  # is about is what the cron schedule does next.
  run pitchfork start cron_fail_fail
  assert_failure

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_fail_fail --raw 2>/dev/null | grep -c "Failed after 0!" || true)
    [[ "$count" -ge 2 ]] && break
    sleep 2
  done

  [[ "$count" -ge 2 ]]
}

# ============================================================================
# Cron retrigger tests with long-running tasks
# ============================================================================

# bats test_tags=slow
@test "cron finish retrigger with long-running task starts only once" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local slowly_output_script
  slowly_output_script="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_finish_long]
run = 'bash "$slowly_output_script" 2 999'
retry = 0

[daemons.cron_finish_long.cron]
schedule = "* * * * * *"
retrigger = "finish"
immediate = true
EOF

  run pitchfork start cron_finish_long
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_finish_long --raw 2>/dev/null | grep -c "Output 1/999" || true)
    [[ "$count" -ge 1 ]] && break
    sleep 2
  done

  [[ "$count" -eq 1 ]]
}

# bats test_tags=slow
@test "cron always retrigger with long-running task restarts at least twice" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local slowly_output_script
  slowly_output_script="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_always_long]
run = 'bash "$slowly_output_script" 2 999'
retry = 0

[daemons.cron_always_long.cron]
schedule = "* * * * * *"
retrigger = "always"
immediate = true
EOF

  # No assertion on the exit status: `retrigger = "always"` force-restarts the
  # daemon every second, so it can be replaced while `start` is still waiting
  # for it and either outcome is legitimate.
  run pitchfork start cron_always_long

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_always_long --raw 2>/dev/null | grep -c "Output 1/999" || true)
    [[ "$count" -ge 2 ]] && break
    sleep 2
  done

  [[ "$count" -ge 2 ]]
}

# bats test_tags=slow
@test "cron success retrigger with long-running task starts only once" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local slowly_output_script
  slowly_output_script="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_success_long]
run = 'bash "$slowly_output_script" 2 999'
retry = 0

[daemons.cron_success_long.cron]
schedule = "* * * * * *"
retrigger = "success"
immediate = true
EOF

  run pitchfork start cron_success_long
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_success_long --raw 2>/dev/null | grep -c "Output 1/999" || true)
    [[ "$count" -ge 1 ]] && break
    sleep 2
  done

  [[ "$count" -eq 1 ]]
}

# bats test_tags=slow
@test "cron fail retrigger with long-running task starts only once" {
  [[ -n "${RUN_SLOW:-}" ]] || skip "Slow test, set RUN_SLOW=1 to run"

  local slowly_output_script
  slowly_output_script="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.cron_fail_long]
run = 'bash "$slowly_output_script" 2 999'
retry = 0

[daemons.cron_fail_long.cron]
schedule = "* * * * * *"
retrigger = "fail"
immediate = true
EOF

  run pitchfork start cron_fail_long
  assert_success

  local count=0
  for _ in $(seq 1 65); do
    count=$(pitchfork logs cron_fail_long --raw 2>/dev/null | grep -c "Output 1/999" || true)
    [[ "$count" -ge 1 ]] && break
    sleep 2
  done

  [[ "$count" -eq 1 ]]
}

# ============================================================================
# Fast cron immediate/retry tests
# ============================================================================

@test "cron immediate=false does not fire on first watcher tick" {
  create_pitchfork_toml <<EOF
[daemons.cron_no_immediate]
run = "echo fired"

[daemons.cron_no_immediate.cron]
schedule = "0 0 1 1 *"
retrigger = "always"
immediate = false
EOF

  run pitchfork start cron_no_immediate
  assert_success

  # The manual start runs the daemon once; with immediate=false the cron watcher
  # should not trigger any additional runs for this far-future schedule.
  sleep 3

  local count
  count=$(pitchfork logs cron_no_immediate --raw 2>/dev/null | grep -c "fired" || true)
  [[ "$count" -eq 1 ]]
}

@test "cron immediate=true fires on start" {
  create_pitchfork_toml <<EOF
[daemons.cron_immediate]
run = "echo immediate_fired"

[daemons.cron_immediate.cron]
schedule = "*/5 * * * * *"
retrigger = "always"
immediate = true
EOF

  run pitchfork start cron_immediate
  assert_success

  # immediate=true should trigger the cron watcher on its first check because a
  # scheduled time falls within the 10-second look-back window. The manual
  # start logs the first line; poll until the cron-triggered second line
  # arrives — asserting a count immediately races the cron tick and the log
  # flush on loaded CI runners.
  wait_for_logs cron_immediate "immediate_fired" 5

  local count
  count=0
  for _ in $(seq 1 15); do
    count=$(pitchfork logs cron_immediate --raw 2>/dev/null | grep -c "immediate_fired" || true)
    [[ "$count" -ge 2 ]] && break
    sleep 1
  done
  [[ "$count" -ge 2 ]]
}

@test "cron finish retrigger does not restart a running daemon" {
  create_pitchfork_toml <<EOF
[daemons.cron_finish_running]
run = "echo started && sleep 10"

[daemons.cron_finish_running.cron]
schedule = "* * * * * *"
retrigger = "finish"
immediate = true
EOF

  run pitchfork start cron_finish_running
  assert_success

  # With retrigger=finish, the daemon should not be restarted while it is still
  # running, even though the schedule fires every second.
  sleep 3

  local count
  count=$(pitchfork logs cron_finish_running --raw 2>/dev/null | grep -c "started" || true)
  [[ "$count" -eq 1 ]]
}

@test "retry=true retries indefinitely" {
  local fail_script
  fail_script="$(script_path fail.sh)"

  create_pitchfork_toml <<EOF
[daemons.retry_infinite]
run = 'bash "$fail_script" 0'
retry = true
EOF

  # Infinite retry causes the start client to block forever, so run it in the
  # background and observe several retry attempts.
  pitchfork start retry_infinite &
  local start_pid=$!
  sleep 10
  kill "$start_pid" 2>/dev/null || true
  wait "$start_pid" 2>/dev/null || true

  local count
  count=$(pitchfork logs retry_infinite --raw 2>/dev/null | grep -c "Failed after 0!" || true)
  [[ "$count" -ge 3 ]]

  # The daemon should still be retrying, not in a final stopped state.
  local status
  status=$(get_daemon_status retry_infinite || true)
  [[ "$status" != "stopped" ]]
}

@test "retry with ready_output re-checks on each attempt" {
  local success_script
  success_script="$(script_path success_on_third.sh)"

  create_pitchfork_toml <<EOF
[daemons.retry_ready_output]
run = 'bash "$success_script"'
ready_output = "READY"
retry = 2

[daemons.retry_ready_output.env]
TEST_SUCCESS_ON_THIRD_TIMESTAMP = "$BATS_TEST_NAME"
EOF

  run pitchfork start retry_ready_output
  assert_success

  wait_for_logs retry_ready_output "Success!" 15
  wait_for_logs retry_ready_output "Attempt 3" 15

  # The daemon command exits after success, so it ends in stopped status.
  # The important verification is that start succeeded and the third attempt
  # produced the ready/success output despite the non-matching ready_output
  # pattern being rechecked on each attempt.
  wait_for_status retry_ready_output stopped
}


# ============================================================================
# Schedule timing in `pitchfork status`
#
# A cron daemon reads `stopped` between runs, which says nothing about whether
# the schedule is still live. These cover the lines that answer that.
# ============================================================================

@test "status shows the schedule and next run for a daemon that has never run" {
  create_pitchfork_toml <<EOF
[daemons.cron_status_never]
run = "echo fired"

[daemons.cron_status_never.cron]
schedule = "0 0 3 * * *"
retrigger = "finish"
immediate = false
EOF

  run pitchfork status cron_status_never
  assert_success
  assert_output --partial "Cron: 0 0 3 * * *"
  assert_output --partial "Last run: never"
  assert_output --partial "Next run: "
  # The next run is a real timestamp with a relative hint, not a placeholder.
  assert_output --regexp "Next run: [0-9]{4}-[0-9]{2}-[0-9]{2} 03:00:00 \(in "
}

@test "status --json carries the schedule and next run" {
  create_pitchfork_toml <<EOF
[daemons.cron_status_json]
run = "echo fired"

[daemons.cron_status_json.cron]
schedule = "0 0 3 * * *"
retrigger = "finish"
immediate = false
EOF

  # --separate-stderr: the supervisor autostart notice goes to stderr and
  # would otherwise be parsed as part of the document.
  run --separate-stderr pitchfork status cron_status_json --json
  assert_success

  local parsed
  parsed=$(python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["cron_schedule"])
# A schedule that has not come due has no last run: the key is omitted rather
# than reported as a null-ish value.
print("cron_last_run" in d)
print(d["cron_next_run"])
' <<<"$output")

  [[ "$(sed -n 1p <<<"$parsed")" == "0 0 3 * * *" ]]
  [[ "$(sed -n 2p <<<"$parsed")" == "False" ]]
  [[ "$(sed -n 3p <<<"$parsed")" == *T03:00:00* ]]
}

@test "status reports the last run once the schedule fires" {
  create_pitchfork_toml <<EOF
[daemons.cron_status_ran]
run = "echo cron_ran"
retry = 0

[daemons.cron_status_ran.cron]
schedule = "* * * * * *"
retrigger = "always"
immediate = true
EOF

  run pitchfork start cron_status_ran
  assert_success
  wait_for_logs cron_status_ran "cron_ran" 15

  # Poll: the manual start is not a cron run, so `Last run` stays `never`
  # until the watcher itself triggers the daemon.
  local ok=0
  for _ in $(seq 1 20); do
    if pitchfork status cron_status_ran | grep -qE "Last run: [0-9]{4}-"; then
      ok=1
      break
    fi
    sleep 1
  done
  [[ "$ok" -eq 1 ]]

  # grep, not assert_output --regexp: `$` there anchors to the end of the
  # whole multi-line output, so it cannot pin what follows on one line.
  run bash -c 'pitchfork status cron_status_ran 2>/dev/null | grep -cE "^Last run: [0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2} \([^)]*ago\)$"'
  assert_success
  assert_output "1"
}

# `last_exit_success` is the daemon's last exit, not necessarily the exit of
# the run `Last run` names: a later manual start replaces it, and it is not
# cleared while a new run is in flight. The line therefore carries a timestamp
# only, and the outcome stays on the `Status:` line where it is attributed to
# the run it actually describes.
@test "the last run line carries no exit outcome" {
  create_pitchfork_toml <<EOF
[daemons.cron_status_noverdict]
run = "sleep 30"
retry = 0

[daemons.cron_status_noverdict.cron]
schedule = "* * * * * *"
retrigger = "finish"
immediate = true
EOF

  # Not started by hand: `retrigger = "finish"` would decline every scheduled
  # tick while that run is up, so the watcher would never own a run of its
  # own. `immediate = true` lets its first tick start the daemon instead.
  #
  # Both conditions are polled together: `last_cron_run` is persisted at the
  # spawn, before the `running` status is, so waiting on the timestamp alone
  # would race the status upsert that follows it.
  local ok=0
  local snap
  for _ in $(seq 1 20); do
    snap=$(pitchfork status cron_status_noverdict 2>/dev/null)
    if grep -qE "^Last run: [0-9]{4}-" <<<"$snap" && grep -qE "^Status: running" <<<"$snap"; then
      ok=1
      break
    fi
    sleep 1
  done
  [[ "$ok" -eq 1 ]]

  run pitchfork status cron_status_noverdict
  assert_success
  # The outcome lives here, on the run it actually describes.
  assert_output --partial "Status: running"

  # And the timestamp line ends at the relative hint: nothing is appended.
  run bash -c 'pitchfork status cron_status_noverdict 2>/dev/null | grep -cE "^Last run: [0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2} \([^)]*\)$"'
  assert_success
  assert_output "1"

  run --separate-stderr pitchfork status cron_status_noverdict --json
  assert_success
  local has_success
  has_success=$(python3 -c '
import json, sys
print("cron_last_success" in json.load(sys.stdin))
' <<<"$output")
  [[ "$has_success" == "False" ]]
}

# A run is a run whether or not it succeeded, and a job that fails instantly is
# exactly the one whose timing a user needs. The spawn is what counts: a
# process that exits before its PID can be read reports the same response as a
# start that never spawned at all, so the record cannot be driven off that.
@test "status records the last run for a cron job that fails immediately" {
  create_pitchfork_toml <<EOF
[daemons.cron_status_fails]
run = "exit 7"
retry = 0

[daemons.cron_status_fails.cron]
schedule = "* * * * * *"
retrigger = "always"
immediate = true
EOF

  local ok=0
  for _ in $(seq 1 25); do
    if pitchfork status cron_status_fails 2>/dev/null | grep -qE "^Last run: [0-9]{4}-"; then
      ok=1
      break
    fi
    sleep 1
  done
  [[ "$ok" -eq 1 ]]
}

@test "a non-cron daemon's status has no schedule lines" {
  create_pitchfork_toml <<EOF
[daemons.plain_daemon]
run = "echo hi"
EOF

  run pitchfork status plain_daemon
  assert_success
  refute_output --partial "Cron:"
  refute_output --partial "Last run:"
  refute_output --partial "Next run:"
}

# A cron daemon that has never been started is registered by the supervisor
# straight from config, so it has to be rendered there as `pitchfork start`
# would render it.
@test "a config-only cron daemon gets its templates and the top-level env" {
  create_pitchfork_toml <<'EOF2'
[env]
TOP = "top-level"

[daemons.anchor]
run = "sleep 30"

[daemons.cron_rendered]
run = 'echo "name={{ name }} top=$TOP"'
cron = "* * * * * *"
EOF2

  # Starts the supervisor; `cron_rendered` itself is never started by hand.
  run pitchfork start anchor
  assert_success

  wait_for_logs cron_rendered "name=cron_rendered top=top-level" 20
}

# Wait until `pitchfork status` of $1 shows the cron schedule $2, or shows none
# when $2 is empty: the watcher has applied the config.
_wait_for_cron_schedule() {
  local id="$1" schedule="$2" line
  for _ in $(seq 1 50); do
    line=$(pitchfork status "$id" 2>/dev/null | grep "^Cron:" || true)
    if [[ -z "$schedule" && -z "$line" ]] || [[ -n "$schedule" && "$line" == "Cron: $schedule" ]]; then
      return 0
    fi
    sleep 0.2
  done
  echo "Timed out waiting for the cron schedule of $id to become '$schedule' (last: '$line')" >&2
  return 1
}

# Wait until $1 is not running, so a run the old schedule started has ended,
# then give its last output time to reach the log store.
_wait_for_cron_run_to_end() {
  local id="$1"
  for _ in $(seq 1 50); do
    if ! pitchfork status "$id" 2>/dev/null | grep "^Status:" | grep -q running; then
      sleep 1
      return 0
    fi
    sleep 0.2
  done
  echo "Timed out waiting for $id to stop running" >&2
  return 1
}

# A daemon started by hand keeps the schedule it was started with in state,
# so removing `cron` from config has to stop it being fired from there.
@test "a started cron daemon stops firing once its schedule leaves config" {
  create_pitchfork_toml <<'EOF2'
[daemons.cron_removed]
run = "echo removed_tick"
cron = "* * * * * *"
EOF2

  run pitchfork start cron_removed
  assert_success
  wait_for_logs cron_removed "removed_tick" 10

  create_pitchfork_toml <<'EOF2'
[daemons.cron_removed]
run = "echo removed_tick"
EOF2

  # Count once the watcher has dropped the schedule and any run it had
  # started is over; no run may follow.
  _wait_for_cron_schedule cron_removed ""
  _wait_for_cron_run_to_end cron_removed
  local before after
  before=$(pitchfork logs cron_removed --raw 2>/dev/null | grep -c "removed_tick" || true)
  sleep 4
  after=$(pitchfork logs cron_removed --raw 2>/dev/null | grep -c "removed_tick" || true)
  [[ "$after" -eq "$before" ]]
}

# The schedule stored at start follows config: a new expression takes effect
# without restarting the daemon, and so does restoring the old one.
@test "a started cron daemon follows its schedule as config changes" {
  create_pitchfork_toml <<'EOF2'
[daemons.cron_changed]
run = "echo changed_tick"
cron = "* * * * * *"
EOF2

  run pitchfork start cron_changed
  assert_success
  wait_for_logs cron_changed "changed_tick" 10

  # Far in the future: no further run may follow.
  create_pitchfork_toml <<'EOF2'
[daemons.cron_changed]
run = "echo changed_tick"
cron = "0 0 0 1 1 *"
EOF2
  _wait_for_cron_schedule cron_changed "0 0 0 1 1 *"
  _wait_for_cron_run_to_end cron_changed
  local before after
  before=$(pitchfork logs cron_changed --raw 2>/dev/null | grep -c "changed_tick" || true)
  sleep 4
  after=$(pitchfork logs cron_changed --raw 2>/dev/null | grep -c "changed_tick" || true)
  [[ "$after" -eq "$before" ]]

  # Back to every second: runs resume.
  create_pitchfork_toml <<'EOF2'
[daemons.cron_changed]
run = "echo changed_tick"
cron = "* * * * * *"
EOF2
  local resumed=0
  for _ in $(seq 1 15); do
    resumed=$(pitchfork logs cron_changed --raw 2>/dev/null | grep -c "changed_tick" || true)
    [[ "$resumed" -gt "$after" ]] && break
    sleep 1
  done
  [[ "$resumed" -gt "$after" ]]
}

# The `cron_immediate` line stored in state for the daemon `cron_immediate`.
_stored_cron_immediate() {
  awk '/^\[daemons\..*\/cron_immediate"\]$/ { found = 1; next } /^\[/ { found = 0 } found' \
    "$PITCHFORK_STATE_DIR/state.toml" | grep "^cron_immediate ="
}

# `immediate` is part of the stored schedule too, and follows config even when
# it is the only setting that changed.
@test "a started cron daemon picks up a change to immediate alone" {
  create_pitchfork_toml <<'EOF2'
[daemons.cron_immediate]
run = "echo immediate_tick"
cron = { schedule = "0 0 0 1 1 *", immediate = false }
EOF2

  run pitchfork start cron_immediate
  assert_success
  _stored_cron_immediate | grep -q "^cron_immediate = false"

  create_pitchfork_toml <<'EOF2'
[daemons.cron_immediate]
run = "echo immediate_tick"
cron = { schedule = "0 0 0 1 1 *", immediate = true }
EOF2

  local synced=false
  for _ in $(seq 1 50); do
    if _stored_cron_immediate | grep -q "^cron_immediate = true"; then
      synced=true
      break
    fi
    sleep 0.2
  done
  [[ "$synced" == true ]]
}
