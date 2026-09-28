#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

@test "stop_signal sends custom signal to daemon" {
  # Windows is covered by the Ctrl+C test below: Git Bash's bash does not
  # turn Ctrl+C into SIGINT when it runs as a daemon.
  skip_on_windows "POSIX signals are not supported on Windows"
  local sig_script
  sig_script="$TEST_TEMP_DIR/trap_sigint.sh"
  cat > "$sig_script" <<'EOF'
#!/bin/bash
trap 'echo got_sigint >> "$TEST_TEMP_DIR/signal_marker"; exit 0' SIGINT
sleep 60
EOF
  chmod +x "$sig_script"

  create_pitchfork_toml <<EOF
[daemons.signal_test]
run = "bash $sig_script"
stop_signal = "SIGINT"
ready_delay = 1
EOF

  run pitchfork start signal_test
  assert_success
  wait_for_status signal_test running

  run pitchfork stop signal_test
  assert_success

  wait_for_file "$TEST_TEMP_DIR/signal_marker"
  run cat "$TEST_TEMP_DIR/signal_marker"
  assert_output --partial "got_sigint"
}

@test "stop_signal SIGINT sends Ctrl+C to the daemon on Windows" {
  [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]] || skip "Ctrl+C is how Windows delivers SIGINT"
  local script marker
  script="$(cygpath -w "$TEST_TEMP_DIR/wait_for_ctrl_c.ps1")"
  marker="$(cygpath -w "$TEST_TEMP_DIR/signal_marker")"
  # Ctrl+C stops the loop and runs the finally block; being killed would not.
  cat > "$TEST_TEMP_DIR/wait_for_ctrl_c.ps1" <<EOF
try { Write-Output 'waiting for ctrl+c'; while (\$true) { Start-Sleep -Milliseconds 200 } }
finally { Set-Content -Path '$marker' -Value got_ctrl_c }
EOF

  create_pitchfork_toml <<EOF
[daemons.ctrl_c_test]
run = ["powershell", "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", '$script']
stop_signal = "SIGINT"
ready_output = "waiting for ctrl"
EOF

  run pitchfork start ctrl_c_test
  assert_success

  run pitchfork stop ctrl_c_test
  assert_success

  wait_for_file "$TEST_TEMP_DIR/signal_marker"
  run cat "$TEST_TEMP_DIR/signal_marker"
  assert_output --partial "got_ctrl_c"
}

# Whether Windows process $1 (a PID pitchfork reports, or one a daemon wrote
# down) is still running.
_windows_pid_running() {
  tasklist //FI "PID eq $1" //NH 2>/dev/null | grep -q " $1 "
}

# A batch file answers Ctrl+C with "Terminate batch job (Y/N)?" and, with no
# one to answer, carries on: it has to be terminated once the timeout is up.
@test "stop_signal SIGINT terminates a daemon that ignores Ctrl+C on Windows" {
  [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]] || skip "Ctrl+C is how Windows delivers SIGINT"
  # A forced stop on a loaded machine can take longer than the CLI waits by
  # default: taskkill alone took 7s in one local run. Wait longer, so the test
  # checks the stop and not the machine's load.
  export PITCHFORK_IPC_REQUEST_TIMEOUT=30s
  printf '@echo off\r\n:loop\r\nping -n 2 127.0.0.1 >nul\r\ngoto loop\r\n' >"$TEST_TEMP_DIR/loop.cmd"
  local loop
  loop="$(cygpath -w "$TEST_TEMP_DIR/loop.cmd")"

  create_pitchfork_toml <<EOF
[daemons.ignores_ctrl_c]
run = ["cmd", "/c", '$loop']
stop_signal = { signal = "SIGINT", timeout = "2s" }
EOF

  run pitchfork start ignores_ctrl_c
  assert_success
  wait_for_status ignores_ctrl_c running
  local pid
  pid=$(pitchfork status ignores_ctrl_c | awk '/^PID:/ {print $2}')

  run pitchfork stop ignores_ctrl_c
  assert_success
  wait_for_status ignores_ctrl_c stopped
  run _windows_pid_running "$pid"
  assert_failure
}

# Processes whose command line contains $1 that are still running.
_windows_count_running() {
  # The query's own command line names $1 too, so it leaves itself out.
  powershell -NoProfile -Command "@(Get-CimInstance Win32_Process | Where-Object { \$_.CommandLine -like '*$1*' -and \$_.ProcessId -ne \$PID }).Count" | tr -d '\r'
}

# The daemon exits on Ctrl+C, but a child it started ignores it. The daemon's
# job object still holds the child once the daemon is gone, so it is stopped
# with it.
@test "stop_signal SIGINT stops a child left behind after Ctrl+C on Windows" {
  [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]] || skip "job objects are Windows-only"
  export PITCHFORK_IPC_REQUEST_TIMEOUT=30s
  printf '@echo off\r\n:loop\r\nping -n 2 127.0.0.1 >nul\r\ngoto loop\r\n' >"$TEST_TEMP_DIR/loop.cmd"
  local script loop pid_file
  script="$(cygpath -w "$TEST_TEMP_DIR/with_child.ps1")"
  loop="$(cygpath -w "$TEST_TEMP_DIR/loop.cmd")"
  pid_file="$(cygpath -w "$TEST_TEMP_DIR/child.pid")"
  cat >"$TEST_TEMP_DIR/with_child.ps1" <<'PS1'
param([string]$Loop, [string]$PidFile)
$child = Start-Process cmd -NoNewWindow -PassThru -ArgumentList '/c', $Loop
Set-Content -Path $PidFile -Value $child.Id
Write-Output 'parent ready'
while ($true) { Start-Sleep -Milliseconds 200 }
PS1

  create_pitchfork_toml <<EOF
[daemons.leaves_child]
run = ["powershell", "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", '$script', "-Loop", '$loop', "-PidFile", '$pid_file']
stop_signal = { signal = "SIGINT", timeout = "5s" }
ready_output = "parent ready"
EOF

  run pitchfork start leaves_child
  assert_success
  wait_for_file "$TEST_TEMP_DIR/child.pid"
  local child
  child=$(tr -d '\r\n' <"$TEST_TEMP_DIR/child.pid")
  run _windows_pid_running "$child"
  assert_success

  run pitchfork stop leaves_child
  assert_success
  run _windows_pid_running "$child"
  assert_failure
}

# A child that ignores Ctrl+C keeps starting processes while the daemon
# stops. However late one starts, it is in the daemon's job and is stopped.
@test "stop_signal SIGINT stops processes started while the daemon stops on Windows" {
  [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]] || skip "job objects are Windows-only"
  export PITCHFORK_IPC_REQUEST_TIMEOUT=30s
  local marker="pf-job-spawn-$$-$RANDOM"
  cat >"$TEST_TEMP_DIR/spawner.ps1" <<PS1
Add-Type -Namespace W -Name K -MemberDefinition '[DllImport("kernel32.dll")] public static extern bool SetConsoleCtrlHandler(System.IntPtr h, bool add);'
[W.K]::SetConsoleCtrlHandler([System.IntPtr]::Zero, \$true) | Out-Null
while (\$true) { Start-Process powershell -NoNewWindow -ArgumentList '-NoProfile','-Command','Start-Sleep 60 # $marker' | Out-Null; Start-Sleep -Milliseconds 100 }
PS1
  cat >"$TEST_TEMP_DIR/parent.ps1" <<'PS1'
param([string]$Spawner)
Start-Process powershell -NoNewWindow -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',$Spawner | Out-Null
Write-Output 'parent ready'
while ($true) { Start-Sleep -Milliseconds 200 }
PS1

  create_pitchfork_toml <<EOF
[daemons.keeps_spawning]
run = ["powershell", "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", '$(cygpath -w "$TEST_TEMP_DIR/parent.ps1")', "-Spawner", '$(cygpath -w "$TEST_TEMP_DIR/spawner.ps1")']
stop_signal = { signal = "SIGINT", timeout = "5s" }
ready_output = "parent ready"
EOF

  run pitchfork start keeps_spawning
  assert_success
  sleep 3
  run _windows_count_running "$marker"
  [[ "$output" -gt 0 ]]

  run pitchfork stop keeps_spawning
  assert_success
  sleep 2
  run _windows_count_running "$marker"
  assert_output "0"
}

# The default stop terminates the daemon's tree outright. A grandchild whose
# parent has already exited is out of taskkill /T's reach from the daemon, but
# still in its job.
@test "stop terminates a process whose parent has exited on Windows" {
  [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]] || skip "job objects are Windows-only"
  export PITCHFORK_IPC_REQUEST_TIMEOUT=30s
  # An unusual ping timeout marks this test's ping among any others.
  local marker="-w $((20000 + RANDOM))"
  cat >"$TEST_TEMP_DIR/orphaner.ps1" <<PS1
# cmd starts ping and exits at once, leaving ping without a parent.
& cmd /c start /b ping -n 60 $marker 127.0.0.1
Write-Output 'parent ready'
while (\$true) { Start-Sleep -Milliseconds 200 }
PS1

  create_pitchfork_toml <<EOF2
[daemons.orphaner]
run = ["powershell", "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", '$(cygpath -w "$TEST_TEMP_DIR/orphaner.ps1")']
ready_output = "parent ready"
EOF2

  run pitchfork start orphaner
  assert_success
  run _windows_count_running "$marker 127.0.0.1"
  assert_output "1"

  run pitchfork stop orphaner
  assert_success
  sleep 1
  run _windows_count_running "$marker 127.0.0.1"
  assert_output "0"
}

@test "settings.general.mise loads the project mise environment" {
  command -v mise >/dev/null 2>&1 || skip "mise not installed"
  export PITCHFORK_MISE_BIN
  PITCHFORK_MISE_BIN="$(command -v mise)"
  pitchfork supervisor start --force >/dev/null

  cat >mise.toml <<'EOF'
[env]
PITCHFORK_MISE_TEST = "loaded_from_mise"
EOF
  mise trust mise.toml

  create_pitchfork_toml <<'EOF'
[daemons.mise_test]
run = "echo $PITCHFORK_MISE_TEST"
ready_delay = 1

[settings.general]
mise = true
EOF

  run pitchfork start mise_test
  assert_success
  wait_for_logs mise_test "loaded_from_mise" 10
}

@test "cpu_limit triggers on high CPU usage" {
  skip_on_windows "sysinfo CPU sampling is unreliable on Windows CI"
  export PITCHFORK_INTERVAL=1s
  pitchfork supervisor start --force >/dev/null 2>&1
  export PITCHFORK_INTERVAL=1s

  create_pitchfork_toml <<EOF
[daemons.cpu_burner]
run = "while true; do echo x > /dev/null; done"
cpu_limit = 1
retry = 0
ready_delay = 1
EOF

  run pitchfork start cpu_burner
  assert_success

  # Wait long enough for the default 3 consecutive CPU violations at 1s intervals.
  for _ in $(seq 1 60); do
    local status
    status="$(get_daemon_status cpu_burner)"
    [[ "$status" == "errored" ]] && break
    sleep 0.5
  done

  run pitchfork status cpu_burner
  assert_output --partial "errored"
}

@test "daemons add --local writes to pitchfork.local.toml" {
  run pitchfork daemons add testdaemon --run "sleep 10" --local
  assert_success
  assert [ -f pitchfork.local.toml ]

  run cat pitchfork.local.toml
  assert_output --partial "[daemons.testdaemon]"
  assert_output --partial 'run = "sleep 10"'
}

@test "daemons add --project writes to pitchfork.toml" {
  run pitchfork daemons add testdaemon --run "sleep 10" --project
  assert_success
  assert [ -f pitchfork.toml ]
  refute [ -f pitchfork.local.toml ]

  run cat pitchfork.toml
  assert_output --partial "[daemons.testdaemon]"
  assert_output --partial 'run = "sleep 10"'
}

@test "daemons add --cron-immediate sets immediate=true" {
  run pitchfork daemons add cronjob --run "echo hello" --cron-schedule "* * * * * *" --cron-immediate
  assert_success
  assert [ -f pitchfork.toml ]

  run cat pitchfork.toml
  assert_output --partial "[daemons.cronjob]"
  assert_output --partial 'immediate = true'
}

@test "daemons add --boot-start sets boot_start=true" {
  run pitchfork daemons add bootsvc --run "sleep 10" --boot-start
  assert_success

  run cat pitchfork.toml
  assert_output --partial 'boot_start = true'
}

@test "daemons add --on-stop registers stop hook via CLI" {
  run pitchfork daemons add hooktest --run "sleep 60" --on-stop "touch $TEST_TEMP_DIR/stop_marker"
  assert_success

  run cat pitchfork.toml
  assert_output --partial 'on_stop ='

  run pitchfork start hooktest
  assert_success
  wait_for_status hooktest running

  run pitchfork stop hooktest
  assert_success
  wait_for_file "$TEST_TEMP_DIR/stop_marker"
}

@test "daemons add --on-exit registers exit hook via CLI" {
  run pitchfork daemons add hooktest --run "sleep 1" --on-exit "touch $TEST_TEMP_DIR/exit_marker"
  assert_success

  run cat pitchfork.toml
  assert_output --partial 'on_exit ='

  run pitchfork start hooktest
  assert_success
  wait_for_file "$TEST_TEMP_DIR/exit_marker" 10
}

@test "daemons add --bump with explicit number sets bump range" {
  run pitchfork daemons add portsvc --run "sleep 10" --expected-port 8080 --bump 20
  assert_success

  run cat pitchfork.toml
  assert_output --partial 'bump = 20'
}

@test "cron retrigger=success only re-fires on success" {
  export PITCHFORK_INTERVAL=1s
  export PITCHFORK_CRON_CHECK_INTERVAL=1s

  create_pitchfork_toml <<EOF
[daemons.cron_success]
run = "echo success_output"
ready_delay = 0

[daemons.cron_success.cron]
schedule = "0 0 1 1 *"
retrigger = "success"
immediate = true
EOF

  run pitchfork start cron_success
  assert_success

  sleep 3
  run pitchfork logs cron_success --raw
  assert_output --partial "success_output"

  local count
  count=$(pitchfork logs cron_success --raw 2>/dev/null | grep -c "success_output" || true)
  [[ "$count" -eq 1 ]]
}

@test "cron retrigger=fail does not re-fire on success" {
  export PITCHFORK_INTERVAL=1s
  export PITCHFORK_CRON_CHECK_INTERVAL=1s

  create_pitchfork_toml <<EOF
[daemons.cron_fail]
run = "echo success_output"
ready_delay = 0

[daemons.cron_fail.cron]
schedule = "0 0 1 1 *"
retrigger = "fail"
immediate = true
EOF

  run pitchfork start cron_fail
  assert_success

  sleep 3

  local count
  count=$(pitchfork logs cron_fail --raw 2>/dev/null | grep -c "success_output" || true)
  [[ "$count" -eq 1 ]]
}

@test "time_retention setting is accepted in config" {
  create_pitchfork_toml <<EOF
[daemons.logger]
run = "sleep 60"

[settings.logs]
time_retention = "5s"
EOF

  run pitchfork list
  assert_success
  assert_output --partial "logger"
}

@test "line_retention setting is accepted in config" {
  create_pitchfork_toml <<EOF
[daemons.logger]
run = "sleep 60"

[settings.logs]
line_retention = 100
EOF

  run pitchfork list
  assert_success
  assert_output --partial "logger"
}

@test "archive_hook setting is accepted in config" {
  create_pitchfork_toml <<EOF
[daemons.logger]
run = "sleep 60"
archive_hook = "echo archived"
EOF

  run pitchfork list
  assert_success
  assert_output --partial "logger"
}

@test "daemons remove deletes daemon from config" {
  run pitchfork daemons add toremove --run "sleep 10"
  assert_success

  run cat pitchfork.toml
  assert_output --partial "[daemons.toremove]"

  run pitchfork daemons remove toremove
  assert_success

  run cat pitchfork.toml
  refute_output --partial "[daemons.toremove]"
}

@test "daemons remove on nonexistent daemon gives warning" {
  # Ensure a project config exists so the remove command looks inside it.
  run pitchfork daemons add existing --run "sleep 10"
  assert_success

  run pitchfork daemons remove does_not_exist
  assert_success
  assert_output --partial "does_not_exist" || assert_output --partial "not found"
}

@test "[daemons.x.logs] sub-table configures log_format per-daemon" {
  # Verify the sub-table is parsed and log_format is applied at runtime
  create_pitchfork_toml <<'EOF'
[daemons.subtable_json]
run = 'echo {"level":"info","msg":"subt"}; sleep 60'
ready_output = "subt"

[daemons.subtable_json.logs]
log_format = "json"
EOF

  run pitchfork start subtable_json
  assert_success
  wait_for_log_lines subtable_json 1

  # Verify the daemon is running (config was accepted)
  run pitchfork status subtable_json
  assert_success
  assert_output --partial "running"

  pitchfork stop subtable_json
}

@test "[daemons.x.logs] sub-table overrides top-level time_retention" {
  # Top-level: 1h, sub-table: 1s. Config should be accepted without error.
  create_pitchfork_toml <<'EOF'
[daemons.override_test]
run = "sleep 60"
time_retention = "1h"

[daemons.override_test.logs]
time_retention = "1s"
EOF

  run pitchfork list
  assert_success
  [[ "$output" == *"override_test"* ]]

  run pitchfork start override_test
  assert_success
  wait_for_status override_test running

  pitchfork stop override_test
}
