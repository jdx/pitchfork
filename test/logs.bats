#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# ============================================================================
# Tail / follow tests
# ============================================================================

@test "logs --tail streams newly appended output without pager" {
  skip_on_windows "PTY-based streaming test requires script(1)"
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.tail_test]
run = "bash $slowly_output 1 10"
ready_delay = 0
EOF

  pitchfork start tail_test
  wait_for_logs tail_test "Output 1/10" 10

  local output
  output="$(run_with_pty timeout 4 pitchfork logs tail_test --tail 2>&1)" || true

  [[ "$output" != *"(END)"* ]]
  [[ "$output" == *"Output "* ]]

  pitchfork stop tail_test
}

# ============================================================================
# Clear logs
# ============================================================================

@test "logs --clear removes logs for all daemons" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.clear_test_1]
run = "bash $slowly_output 1 3"
ready_delay = 0

[daemons.clear_test_2]
run = "bash $slowly_output 1 3"
ready_delay = 0
EOF

  pitchfork start clear_test_1
  pitchfork start clear_test_2
  wait_for_logs clear_test_1 "Output 1/3" 10
  wait_for_logs clear_test_2 "Output 1/3" 10

  [[ -n "$(read_logs clear_test_1)" ]]
  [[ -n "$(read_logs clear_test_2)" ]]

  run pitchfork logs --clear
  assert_success

  [[ -z "$(read_logs clear_test_1)" ]]
  [[ -z "$(read_logs clear_test_2)" ]]

  pitchfork stop clear_test_1
  pitchfork stop clear_test_2
}

# ============================================================================
# Time filter tests (--since / --until)
# ============================================================================

@test "logs --since with relative time shows recent logs" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.since_test]
run = "bash $slowly_output 1 5"
ready_delay = 0
EOF

  pitchfork start since_test
  wait_for_logs since_test "Output 1/5" 10

  run pitchfork logs since_test --since 3s --raw
  assert_success
  assert_output --partial "Output"

  pitchfork stop since_test
}

@test "logs --since and --until return logs in range" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.range_test]
run = "bash $slowly_output 1 5"
ready_delay = 0
EOF

  local start_time mid_time
  start_time="$(date +"%Y-%m-%d %H:%M:%S")"

  pitchfork start range_test
  wait_for_logs range_test "Output 2/5" 10
  mid_time="$(date +"%Y-%m-%d %H:%M:%S")"
  wait_for_logs range_test "Output 4/5" 10

  run pitchfork logs range_test --since "$start_time" --until "$mid_time" --raw
  assert_success

  local count
  count="$(grep -c "Output" <<< "$output" || true)"
  [[ "$count" -ge 1 ]]

  pitchfork stop range_test
}

@test "logs --since respects -n line limit" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.since_n_test]
run = "bash $slowly_output 1 10"
ready_delay = 0
EOF

  pitchfork start since_n_test
  wait_for_logs since_n_test "Output 10/10" 15

  run pitchfork logs since_n_test --since 10s -n 3 --raw
  assert_success

  local count
  count="$(grep -c "Output" <<< "$output" || true)"
  [[ "$count" -le 3 ]]

  pitchfork stop since_n_test
}

# ============================================================================
# Raw output tests
# ============================================================================

@test "logs --raw omits ANSI escape codes" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.raw_test]
run = "bash $slowly_output 1 3"
ready_delay = 0
EOF

  pitchfork start raw_test
  wait_for_logs raw_test "Output 1/3" 10

  run pitchfork logs raw_test --raw
  assert_success
  [[ -n "$output" ]]
  [[ "$output" != *$'\x1b['* ]]

  pitchfork stop raw_test
}

# ============================================================================
# Line limit tests
# ============================================================================

@test "logs -n limits output to last N lines" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.n_limit_test]
run = "bash $slowly_output 1 10"
ready_delay = 0
EOF

  pitchfork start n_limit_test
  wait_for_logs n_limit_test "Output 10/10" 15

  run pitchfork logs n_limit_test -n 5 --raw
  assert_success

  local count
  count="$(grep -c "Output" <<< "$output" || true)"
  [[ "$count" -le 5 ]]

  pitchfork stop n_limit_test
}

@test "logs without -n outputs directly to stdout in non-interactive mode" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.pager_test]
run = "bash $slowly_output 1 3"
ready_delay = 0
EOF

  pitchfork start pager_test
  wait_for_logs pager_test "Output 1/3" 10

  run pitchfork logs pager_test
  assert_success
  assert_output --partial "Output"
  [[ "$output" != *"(END)"* ]]

  pitchfork stop pager_test
}

# ============================================================================
# Multiple daemon tests
# ============================================================================

@test "logs for multiple daemons includes each daemon" {
  local slowly_output
  slowly_output="$(script_path slowly_output.sh)"

  create_pitchfork_toml <<EOF
[daemons.multi_log_1]
run = "bash $slowly_output 1 3"
ready_delay = 0

[daemons.multi_log_2]
run = "bash $slowly_output 1 3"
ready_delay = 0
EOF

  pitchfork start multi_log_1
  pitchfork start multi_log_2
  wait_for_logs multi_log_1 "Output 1/3" 10
  wait_for_logs multi_log_2 "Output 1/3" 10

  run pitchfork logs multi_log_1 multi_log_2
  assert_success
  [[ "$output" == *"multi_log_1"* ]]
  [[ "$output" == *"multi_log_2"* ]]

  pitchfork stop multi_log_1
  pitchfork stop multi_log_2
}

# ============================================================================
# Grep / regex filter tests
# ============================================================================

@test "logs --grep filters output by substring" {
  create_pitchfork_toml <<EOF
[daemons.grepper]
run = "echo apple; echo banana; echo cherry; sleep 60"
ready_output = "cherry"
EOF

  pitchfork start grepper
  wait_for_logs grepper "cherry" 10

  run pitchfork logs grepper --grep banana --raw
  assert_success
  assert_output --partial "banana"
  [[ "$output" != *"apple"* ]]
  [[ "$output" != *"cherry"* ]]

  pitchfork stop grepper
}

@test "logs --grep with multiple patterns (OR logic)" {
  create_pitchfork_toml <<EOF
[daemons.grepper]
run = "echo apple; echo banana; echo cherry; sleep 60"
ready_output = "cherry"
EOF

  pitchfork start grepper
  wait_for_logs grepper "cherry" 10

  run pitchfork logs grepper --grep apple --grep cherry --raw
  assert_success
  assert_output --partial "apple"
  assert_output --partial "cherry"
  [[ "$output" != *"banana"* ]]

  pitchfork stop grepper
}

@test "logs --grep with no matches returns empty" {
  create_pitchfork_toml <<EOF
[daemons.grepper]
run = "echo apple; echo banana; echo cherry; sleep 60"
ready_output = "cherry"
EOF

  pitchfork start grepper
  wait_for_logs grepper "cherry" 10

  PITCHFORK_LOG=error run pitchfork logs grepper --grep nonexistent --raw
  assert_success
  [[ -z "$output" ]]

  pitchfork stop grepper
}

@test "logs --regex filters output by regex pattern" {
  create_pitchfork_toml <<EOF
[daemons.regexer]
run = "echo port 3000; echo port 4000; echo no_port_here; sleep 60"
ready_output = "no_port_here"
EOF

  pitchfork start regexer
  wait_for_logs regexer "no_port_here" 10

  run pitchfork logs regexer --regex 'port [0-9]+' --raw
  assert_success
  assert_output --partial "port 3000"
  assert_output --partial "port 4000"
  [[ "$output" != *"no_port_here"* ]]

  pitchfork stop regexer
}

@test "logs --regex with invalid pattern gives error" {
  create_pitchfork_toml <<EOF
[daemons.regexer]
run = "echo port 3000; echo port 4000; echo no_port_here; sleep 60"
ready_output = "no_port_here"
EOF

  pitchfork start regexer
  wait_for_logs regexer "no_port_here" 10

  run pitchfork logs regexer --regex '[invalid(' --raw
  assert_failure
  [[ "$output" == *"regex"* || "$output" == *"invalid"* || "$output" == *"parse"* ]]

  pitchfork stop regexer
}

@test "logs --regex with no matches returns empty" {
  create_pitchfork_toml <<EOF
[daemons.regexer]
run = "echo port 3000; echo port 4000; echo no_port_here; sleep 60"
ready_output = "no_port_here"
EOF

  pitchfork start regexer
  wait_for_logs regexer "no_port_here" 10

  PITCHFORK_LOG=error run pitchfork logs regexer --regex 'nomatch[0-9]+' --raw
  assert_success
  [[ -z "$output" ]]

  pitchfork stop regexer
}

@test "logs --case-sensitive respects case" {
  create_pitchfork_toml <<EOF
[daemons.caser]
run = "echo Hello; echo hello; echo HELLO; sleep 60"
ready_output = "HELLO"
EOF

  pitchfork start caser
  wait_for_logs caser "HELLO" 10

  run pitchfork logs caser --grep Hello --case-sensitive --raw
  assert_success
  assert_output --partial "Hello"
  [[ "$output" != *"hello"* ]]
  [[ "$output" != *"HELLO"* ]]

  pitchfork stop caser
}

@test "logs without --case-sensitive is case-insensitive" {
  create_pitchfork_toml <<EOF
[daemons.caser]
run = "echo Hello; echo hello; echo HELLO; sleep 60"
ready_output = "HELLO"
EOF

  pitchfork start caser
  wait_for_logs caser "HELLO" 10

  run pitchfork logs caser --grep hello --raw
  assert_success
  assert_output --partial "Hello"
  assert_output --partial "hello"
  assert_output --partial "HELLO"

  pitchfork stop caser
}

# ============================================================================
# Timestamp tests
# ============================================================================

@test "logs --no-timestamp omits timestamp prefix" {
  create_pitchfork_toml <<EOF
[daemons.notime]
run = "echo testline; sleep 60"
ready_output = "testline"
EOF

  pitchfork start notime
  wait_for_logs notime "testline" 10

  run pitchfork logs notime --no-timestamp
  assert_success
  assert_output --partial "testline"
  [[ "$output" != *"20"* ]]

  pitchfork stop notime
}

# ============================================================================
# Clear / error / edge case tests
# ============================================================================

@test "logs --clear for single daemon preserves others" {
  create_pitchfork_toml <<EOF
[daemons.keeper]
run = "echo keep_me; sleep 60"
ready_output = "keep_me"

[daemons.clearer]
run = "echo clear_me; sleep 60"
ready_output = "clear_me"
EOF

  pitchfork start keeper
  pitchfork start clearer
  wait_for_logs keeper "keep_me" 10
  wait_for_logs clearer "clear_me" 10

  run pitchfork logs --clear clearer
  assert_success

  PITCHFORK_LOG=error run pitchfork logs clearer --raw
  assert_success
  [[ -z "$output" ]]

  run pitchfork logs keeper --raw
  assert_success
  assert_output --partial "keep_me"

  pitchfork stop keeper
  pitchfork stop clearer
}

@test "logs on nonexistent daemon gives error" {
  run pitchfork logs nonexistent_daemon --raw
  assert_failure
  [[ "$output" == *"nonexistent_daemon"* || "$output" == *"not found"* || "$output" == *"error"* ]]
}

@test "logs on daemon with no log output returns empty" {
  create_pitchfork_toml <<EOF
[daemons.unstarted]
run = "echo would_log; sleep 60"
ready_output = "would_log"
EOF

  # Daemon is configured but not started; no logs should exist
  PITCHFORK_LOG=error run pitchfork logs unstarted --raw
  assert_success
  [[ -z "$output" ]]
}

@test "logs without daemon argument lists available daemons" {
  create_pitchfork_toml <<EOF
[daemons.solo]
run = "echo hello_world; sleep 60"
ready_output = "hello_world"
EOF

  pitchfork start solo
  wait_for_logs solo "hello_world" 10

  run pitchfork logs --raw --no-timestamp
  assert_success
  assert_output --partial "hello_world"

  pitchfork stop solo
}

@test "logs --grep and --regex combined (OR logic)" {
  create_pitchfork_toml <<EOF
[daemons.combo]
run = "echo alpha; echo beta; echo gamma; sleep 60"
ready_output = "gamma"
EOF

  pitchfork start combo
  wait_for_logs combo "gamma" 10

  run pitchfork logs combo --grep alpha --regex 'beta' --raw
  assert_success
  assert_output --partial "alpha"
  assert_output --partial "beta"
  [[ "$output" != *"gamma"* ]]

  pitchfork stop combo
}

# ============================================================================
# Tail + filter tests
# ============================================================================

@test "logs --tail with --grep only streams matching lines" {
  create_pitchfork_toml <<EOF
[daemons.tailer]
run = "while true; do echo match_line; echo skip_line; sleep 1; done"
ready_output = "match_line"
EOF

  pitchfork start tailer
  wait_for_logs tailer "match_line" 10

  local output
  output="$(timeout 3 pitchfork logs tailer --tail --grep match_line --raw 2>&1)" || true
  [[ "$output" == *"match_line"* ]]
  [[ "$output" != *"skip_line"* ]]

  pitchfork stop tailer
}

# ============================================================================
# SSE tests
# ============================================================================

# TODO: The Rust e2e test `test_web_logs_sse_skips_existing_content_on_connect`
# started a web supervisor and consumed the SSE `/logs/project%2Fsse_connect/stream`
# endpoint. Converting it reliably to bash requires backgrounding the supervisor,
# parsing a dynamic port, and timing SSE chunks with curl. Skipping for now.

# ============================================================================
# Out-of-process capture (log sink)
# ============================================================================

@test "log capture survives a supervisor crash" {
  skip_on_windows "relies on SIGKILL semantics for the supervisor"

  # No SIGPIPE guard in the daemon: the whole point is that a plain writer
  # survives losing its supervisor, because the sink still holds the pipe.
  create_pitchfork_toml <<EOF
[daemons.crashlog]
run = "i=0; while true; do i=\$((i+1)); echo crashlog-\$i; sleep 0.3; done"
ready_delay = 1
EOF

  run pitchfork start crashlog
  assert_success
  wait_for_logs crashlog "crashlog-"

  local daemon_pid sup_pid before
  daemon_pid="$(get_daemon_pid crashlog)"
  sup_pid="$(grep -A 10 '\[daemons\."global/pitchfork"\]' "$PITCHFORK_STATE_DIR/state.toml" |
    grep -E '^pid = ' | head -1 | sed -E 's/.*= //')"
  [[ -n "$daemon_pid" && -n "$sup_pid" ]]
  before="$(read_logs crashlog | grep -c "crashlog-")"

  kill_pid "$sup_pid"
  sleep 3

  # The daemon outlives its supervisor, and its output keeps being recorded
  # even though no supervisor exists to record it.
  pid_alive "$daemon_pid"
  local after
  after="$(read_logs crashlog | grep -c "crashlog-")"
  [[ "$after" -gt "$before" ]] || {
    echo "capture stopped at the crash: before=$before after=$after" >&2
    return 1
  }

  kill_pid "$daemon_pid"
}

@test "ready_output is decided by the sink" {
  skip_on_windows "relies on pgrep to see the sink process"

  # Readiness must not arrive before the daemon says it is ready, and must
  # arrive once it does — with the supervisor never reading the output itself.
  create_pitchfork_toml <<EOF
[daemons.sinkready]
run = "sleep 2; echo SERVING on 8080; sleep 300"
ready_output = "SERVING"
EOF

  local start end
  start="$(date +%s)"
  run pitchfork start sinkready
  assert_success
  end="$(date +%s)"

  # A sink owns the stream, so the match came back over IPC rather than from
  # the supervisor reading the pipe.
  run pgrep -f "log-sink --daemon-id .*sinkready"
  assert_success

  # It waited for the line rather than returning early on a timer.
  [[ "$((end - start))" -ge 2 ]] || {
    echo "returned after $((end - start))s; readiness did not wait for the output" >&2
    return 1
  }

  run pitchfork status sinkready
  assert_output --partial "running"

  # The line that decided readiness is in the log store exactly once: the sink
  # stores it, and the copy relayed to the supervisor is for matching only.
  assert_equal "$(read_logs sinkready | grep -c "SERVING on 8080")" "1"

  pitchfork stop sinkready
}

@test "log capture for a ready_output daemon survives a supervisor crash" {
  skip_on_windows "relies on SIGKILL semantics for the supervisor"

  # `ready_output` used to force in-process capture, which put the supervisor
  # back in the data path for exactly these daemons.
  create_pitchfork_toml <<EOF
[daemons.readycrash]
run = "echo LISTENING; i=0; while true; do i=\$((i+1)); echo readycrash-\$i; sleep 0.3; done"
ready_output = "LISTENING"
EOF

  run pitchfork start readycrash
  assert_success
  wait_for_logs readycrash "readycrash-"

  local daemon_pid sup_pid before
  daemon_pid="$(get_daemon_pid readycrash)"
  sup_pid="$(grep -A 10 '\[daemons\."global/pitchfork"\]' "$PITCHFORK_STATE_DIR/state.toml" |
    grep -E '^pid = ' | head -1 | sed -E 's/.*= //')"
  [[ -n "$daemon_pid" && -n "$sup_pid" ]]
  before="$(read_logs readycrash | grep -c "readycrash-")"

  kill_pid "$sup_pid"
  sleep 3

  pid_alive "$daemon_pid"
  local after
  after="$(read_logs readycrash | grep -c "readycrash-")"
  [[ "$after" -gt "$before" ]] || {
    echo "capture stopped at the crash: before=$before after=$after" >&2
    return 1
  }

  kill_pid "$daemon_pid"
}

@test "a log sink that dies is replaced" {
  skip_on_windows "relies on SIGKILL semantics and pgrep"

  create_pitchfork_toml <<EOF
[daemons.sinkdies]
run = "i=0; while true; do i=\$((i+1)); echo sinkdies-\$i; sleep 0.3; done"
ready_delay = 1
EOF

  run pitchfork start sinkdies
  assert_success
  wait_for_logs sinkdies "sinkdies-"

  local daemon_pid first
  daemon_pid="$(get_daemon_pid sinkdies)"
  first="$(pgrep -f "log-sink --daemon-id .*/sinkdies" | head -1)"
  [[ -n "$first" ]] || {
    echo "no sink process found for the daemon" >&2
    return 1
  }

  kill_pid "$first"

  # A replacement must take over, or the pipe would fill and block the daemon.
  local second=""
  for _ in $(seq 1 50); do
    second="$(pgrep -f "log-sink --daemon-id .*/sinkdies" | head -1)"
    [[ -n "$second" && "$second" != "$first" ]] && break
    sleep 0.2
  done
  [[ -n "$second" && "$second" != "$first" ]] || {
    echo "sink was not replaced (was $first, now ${second:-none})" >&2
    return 1
  }
  pid_alive "$daemon_pid"

  # And capture resumes through the replacement.
  local before after
  before="$(read_logs sinkdies | grep -c "sinkdies-")"
  for _ in $(seq 1 50); do
    after="$(read_logs sinkdies | grep -c "sinkdies-")"
    [[ "$after" -gt "$before" ]] && break
    sleep 0.2
  done
  [[ "$after" -gt "$before" ]] || {
    echo "capture did not resume: before=$before after=$after" >&2
    return 1
  }

  pitchfork stop sinkdies
}

@test "start failure diagnostics include the daemon's output" {
  # collect_startup_logs reads the log store, so a failed start must not report
  # before the sink has written what the daemon printed.
  create_pitchfork_toml <<EOF
[daemons.diagfail]
run = "echo diagnostic-marker; exit 3"
ready_delay = 5
EOF

  run pitchfork start diagfail
  assert_output --partial "diagnostic-marker"
}
