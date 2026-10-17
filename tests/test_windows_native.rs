//! Native-shell regressions. Child processes have no Git Bash on PATH and use
//! isolated state/config directories and a binary installed in a Unicode path
//! containing spaces. Node.js and a POSIX shell are not required.
#![cfg(windows)]

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(45);

fn output_with_deadline(mut command: Command, deadline: Duration) -> std::io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut command = tokio::process::Command::from(command);
            command.kill_on_drop(true);
            // output() drains both streams while waiting. Dropping the future
            // at the deadline kills the child rather than waiting on full pipes.
            tokio::time::timeout(deadline, command.output())
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "native command timed out")
                })?
        })
}

fn text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn on_path(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("{name} must be installed to run native Windows tests"))
}

fn toml_string(value: impl AsRef<Path>) -> String {
    toml::Value::String(value.as_ref().to_string_lossy().into_owned()).to_string()
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + DEADLINE;
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "file was not created: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

// Retain process identities so shutdown checks cannot observe reused PIDs.
struct RunningProcess(OwnedHandle);

impl RunningProcess {
    fn open(pid: u32) -> Self {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        };

        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        assert!(
            !handle.is_null(),
            "pid {pid}: {}",
            std::io::Error::last_os_error()
        );
        let process = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        assert!(!process.exited(), "pid {pid} already exited");
        process
    }

    fn exited(&self) -> bool {
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => true,
            WAIT_TIMEOUT => false,
            result => panic!(
                "process wait failed: {result:#x}: {}",
                std::io::Error::last_os_error()
            ),
        }
    }

    fn exit_code(&self) -> u32 {
        use windows_sys::Win32::System::Threading::GetExitCodeProcess;

        assert!(self.exited(), "process is still running");
        let mut code = 0;
        assert_ne!(
            unsafe { GetExitCodeProcess(self.0.as_raw_handle(), &mut code) },
            0
        );
        code
    }
}

struct NativeTest {
    _temp: tempfile::TempDir,
    root: PathBuf,
    binary: PathBuf,
    system: PathBuf,
    powershell: PathBuf,
    mise: Option<PathBuf>,
}

impl NativeTest {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Pitchfork space λ");
        std::fs::create_dir_all(root.join("config")).unwrap();
        let binary = root.join("pitchfork.exe");
        std::fs::copy(env!("CARGO_BIN_EXE_pitchfork"), &binary).unwrap();
        let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        let powershell = system.join("WindowsPowerShell/v1.0/powershell.exe");
        Self {
            _temp: temp,
            root,
            binary,
            system,
            powershell,
            mise: None,
        }
    }

    fn with_mise(mut self) -> Self {
        self.mise = Some(on_path("mise.exe"));
        self
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.binary);
        // Do not inherit a developer's shell override, live supervisor, or
        // mise project selection. Keep Windows' other process environment.
        for (name, _) in std::env::vars_os() {
            let name_text = name.to_string_lossy().to_ascii_uppercase();
            if name_text.starts_with("PITCHFORK_") || name_text.starts_with("MISE_") {
                cmd.env_remove(name);
            }
        }
        cmd.args(args)
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .current_dir(&self.root)
            .env("PITCHFORK_STATE_DIR", self.root.join("state"))
            .env("PITCHFORK_CONFIG_DIR", self.root.join("config"))
            .env("PITCHFORK_LOGS_DIR", self.root.join("logs"))
            .env("PITCHFORK_PROXY_ENABLE", "false")
            .env("PITCHFORK_WEB_AUTO_START", "false")
            // An invalid explicit path stops automatic boot repair before it
            // can read or rewrite the developer's real startup registration.
            .env(
                "PITCHFORK_BOOT_EXECUTABLE",
                self.root.join("no-boot-registration.exe"),
            )
            .env(
                "PATH",
                std::env::join_paths([self.system.as_path(), self.powershell.parent().unwrap()])
                    .unwrap(),
            );
        if let Some(mise) = &self.mise {
            cmd.env("PITCHFORK_MISE_BIN", mise)
                .env("MISE_TRUSTED_CONFIG_PATHS", &self.root);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        output_with_deadline(self.command(args), DEADLINE)
            .unwrap_or_else(|error| panic!("{args:?}: {error}\n{}", self.supervisor_log()))
    }

    fn success(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}\n{}",
            text(&output),
            self.supervisor_log()
        );
        output
    }

    fn supervisor_log(&self) -> String {
        std::fs::read_to_string(self.root.join("logs/pitchfork/pitchfork.log"))
            .unwrap_or_else(|error| error.to_string())
    }

    fn status(&self, id: &str) -> serde_json::Value {
        let output = self.success(&["status", id, "--json"]);
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("invalid status JSON: {error}: {}", text(&output)))
    }

    fn config(&self, contents: &str) {
        std::fs::write(
            self.root.join("pitchfork.toml"),
            format!("namespace = 'native'\n{contents}"),
        )
        .unwrap();
    }
}

impl Drop for NativeTest {
    fn drop(&mut self) {
        // Cleanup must not cause a second panic when a test already failed.
        let _ = output_with_deadline(self.command(&["supervisor", "stop"]), DEADLINE);
    }
}

#[test]
fn native_output_capture_drains_both_pipes_and_enforces_deadlines() {
    let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
    let powershell = system.join("WindowsPowerShell/v1.0/powershell.exe");
    let mut command = Command::new(&powershell);
    command
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::Out.Write(('o' * 1048576)); [Console]::Error.Write(('e' * 1048576))",
        ]);
    let output = output_with_deadline(command, DEADLINE).unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(output.stdout, vec![b'o'; 1048576]);
    assert_eq!(output.stderr, vec![b'e'; 1048576]);

    let mut command = Command::new(powershell);
    command
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 60",
        ]);
    let start = Instant::now();
    let error = output_with_deadline(command, Duration::from_millis(500)).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn shell_bridge_skips_settings_and_keeps_daemon_output_clean() {
    let test = NativeTest::new();
    let mut command = test.command(&["shell-exec", "--", "cmd", "/D", "/C", "echo APP_ONLY"]);
    // Either setting would normally produce a warning during CLI startup.
    command
        .env("PITCHFORK_INTERVAL", "APP_NOT_READY")
        .env("PITCHFORK_INTERVAL_SECS", "10s");
    let output = output_with_deadline(command, DEADLINE).unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "APP_ONLY");
    assert!(output.stderr.is_empty(), "{}", text(&output));
    assert!(!test.root.join("logs").exists());
}

#[test]
fn quoted_scripts_run_in_native_shells_with_and_without_mise() {
    let mut shells = vec![
        "cmd /C".to_string(),
        "cmd /D /C".to_string(),
        "powershell -NoLogo -NoProfile -NonInteractive -Command".to_string(),
    ];
    // PowerShell 7 is available in CI; also exercise it when installed locally.
    if let Some(pwsh) = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("pwsh.exe"))
        .find(|path| path.is_file())
    {
        shells.push(format!(
            "'{}' -NoLogo -NoProfile -NonInteractive -Command",
            pwsh.display()
        ));
    }
    for shell in shells {
        for mise in [false, true] {
            let test = NativeTest::new();
            let test = if mise { test.with_mise() } else { test };
            let script = test.root.join("say hello.cmd");
            std::fs::write(
                &script,
                concat!(
                    "@echo off\r\n",
                    "echo argument=[%~1]\r\n",
                    "echo daemon=[%NATIVE_VALUE%]\r\n",
                    "echo mise=[%NATIVE_MISE_VALUE%]\r\n",
                    "exit /b 0\r\n",
                ),
            )
            .unwrap();
            std::fs::write(
                test.root.join("mise.toml"),
                "[env]\nNATIVE_MISE_VALUE = 'active'\n",
            )
            .unwrap();
            let run = format!("\"{}\" \"a b\"", script.display());
            let shell_setting = if shell == "cmd /C" {
                String::new()
            } else {
                format!("windows_shell = {}\n", toml::Value::String(shell.clone()))
            };
            test.config(&format!(
                r#"
[settings.general]
{shell_setting}mise = false

[daemons.demo]
run = {run}
oneshot = true
mise = {mise}
env = {{ NATIVE_VALUE = 'from daemon' }}
"#,
                run = toml::Value::String(run)
            ));
            test.success(&["start", "demo", "--quiet"]);
            let logs = text(&test.success(&["logs", "demo", "-n", "20"]));
            assert!(
                logs.contains("argument=[a b]"),
                "shell={shell} mise={mise}: {logs}"
            );
            assert!(logs.contains("daemon=[from daemon]"), "{logs}");
            let expected = if mise { "mise=[active]" } else { "mise=[]" };
            assert!(logs.contains(expected), "shell={shell} mise={mise}: {logs}");
            let status = test.status("demo");
            assert_eq!(status["status"], "completed", "{status}");
            assert!(status["pid"].is_null(), "{status}");
        }
    }
}

#[test]
fn mise_cmd_preserves_expansion_compound_commands_and_failure_codes() {
    let test = NativeTest::new().with_mise();
    let script = test.root.join("fail with code.cmd");
    std::fs::write(
        &script,
        "@echo off\r\necho argument=[%~1]\r\nexit /b 37\r\n",
    )
    .unwrap();
    std::fs::write(
        test.root.join("mise.toml"),
        r#"
[env]
NATIVE_MISE_VALUE = 'expanded value'

[vars]
greeting = 'from mise template'
"#,
    )
    .unwrap();
    let run = format!(
        "echo [{{{{ vars.greeting }}}}] && echo [%NATIVE_MISE_VALUE%] && \"{}\" \"a b\"",
        script.display()
    );
    test.config(&format!(
        r#"
[settings.general]
windows_shell = 'cmd /D /C'
mise = false

[daemons.demo]
run = {run}
oneshot = true
mise = true
"#,
        run = toml::Value::String(run)
    ));
    let start = test.run(&["start", "demo", "--quiet"]);
    assert!(!start.status.success(), "{}", text(&start));
    let wait = test.run(&["wait", "demo"]);
    assert_eq!(wait.status.code(), Some(37), "{}", text(&wait));
    let logs = text(&test.success(&["logs", "demo", "-n", "20"]));
    assert!(logs.contains("[expanded value]"), "{logs}");
    assert!(logs.contains("[from mise template]"), "{logs}");
    assert!(logs.contains("argument=[a b]"), "{logs}");
}

fn assert_graceful_ctrl_c_shutdown(cleanup_delay: Duration) {
    const SHUTDOWN_EXIT_CODE: u32 = 42;

    let test = NativeTest::new().with_mise();
    let marker = test.root.join("graceful shutdown.txt");
    let child_pid = test.root.join("graceful child.pid");
    std::fs::write(
        test.root.join("mise.toml"),
        "[env]\nNATIVE_MISE_VALUE = 'active'\n",
    )
    .unwrap();
    // Both the application and its wrappers must return the code chosen after
    // cleanup, rather than a forced termination's code, even when cleanup waits.
    let run = format!(
        "\"{}\" -NoLogo -NoProfile -NonInteractive -Command \"try {{ [IO.File]::WriteAllText('{}', $PID.ToString()); Write-Output READY; while ($true) {{ Start-Sleep -Milliseconds 100 }} }} finally {{ [Threading.Thread]::Sleep({}); [IO.File]::WriteAllText('{}', 'SIGINT'); [Environment]::Exit({SHUTDOWN_EXIT_CODE}) }}\"",
        test.powershell.display(),
        child_pid.display(),
        cleanup_delay.as_millis(),
        marker.display()
    );
    test.config(&format!(
        r#"
[settings.general]
log_file_level = 'debug'

[daemons.demo]
run = {run}
mise = true
ready_output = '^READY$'
stop_signal = 'SIGINT'
"#,
        run = toml::Value::String(run)
    ));
    test.success(&["start", "demo", "--quiet"]);
    let pid: u32 = test.status("demo")["pid"]
        .as_u64()
        .unwrap()
        .try_into()
        .unwrap();
    let parent = RunningProcess::open(pid);
    let child = RunningProcess::open(std::fs::read_to_string(child_pid).unwrap().parse().unwrap());
    let stopped = test.success(&["stop", "demo"]);
    let log = test.supervisor_log();
    let marker = std::fs::read_to_string(marker);
    assert_eq!(
        marker.as_deref().ok(),
        Some("SIGINT"),
        "shutdown marker: {marker:?}\nstop output:\n{}\nsupervisor log:\n{}",
        text(&stopped),
        log,
    );
    assert!(
        parent.exited() && child.exited(),
        "{}\n{log}",
        text(&stopped)
    );
    assert_eq!(child.exit_code(), SHUTDOWN_EXIT_CODE, "{log}");
    assert_eq!(parent.exit_code(), SHUTDOWN_EXIT_CODE, "{log}");
}

#[test]
fn mise_cmd_bridge_preserves_graceful_ctrl_c_shutdown() {
    assert_graceful_ctrl_c_shutdown(Duration::ZERO);
}

#[test]
fn mise_cmd_bridge_waits_for_delayed_ctrl_c_shutdown() {
    assert_graceful_ctrl_c_shutdown(Duration::from_secs(1));
}

#[test]
fn mise_cmd_bridge_forces_tree_shutdown_after_ctrl_c_timeout() {
    let test = NativeTest::new().with_mise();
    let script = test.root.join("ignore ctrl c.ps1");
    let child_pid = test.root.join("child.pid");
    let receipt = test.root.join("ctrl c received.txt");
    std::fs::write(
        &script,
        r#"
param([string]$ChildPid, [string]$Receipt)
$ErrorActionPreference = 'Stop'
# The native callback runs outside the PowerShell runspace and only sets a flag.
Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;

public static class NativeConsole {
    private delegate bool Handler(uint signal);
    private static readonly Handler handler = Handle;
    public static volatile bool Received;

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetConsoleCtrlHandler(Handler handler, bool add);

    public static void Ignore(bool ignore) {
        if (!SetConsoleCtrlHandler(null, ignore))
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    }

    public static void Observe() {
        Ignore(false);
        if (!SetConsoleCtrlHandler(handler, true))
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    }

    private static bool Handle(uint signal) {
        if (signal != 0) return false;
        Received = true;
        return true;
    }
}
'@
# Only the child inherits the ignore flag. The parent observes Ctrl+C without exiting.
[NativeConsole]::Ignore($true)
$child = Start-Process powershell -NoNewWindow -PassThru -ArgumentList '-NoLogo', '-NoProfile', '-NonInteractive', '-Command', 'while ($true) { Start-Sleep -Milliseconds 100 }'
[NativeConsole]::Observe()
[IO.File]::WriteAllText($ChildPid, $child.Id.ToString())
Write-Output READY
while (![NativeConsole]::Received) { Start-Sleep -Milliseconds 25 }
[IO.File]::WriteAllText($Receipt, 'SIGINT')
while ($true) { Start-Sleep -Milliseconds 100 }
"#,
    )
    .unwrap();
    let run = format!(
        "\"{}\" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\" -ChildPid \"{}\" -Receipt \"{}\"",
        test.powershell.display(),
        script.display(),
        child_pid.display(),
        receipt.display(),
    );
    test.config(&format!(
        r#"
[settings.general]
log_file_level = 'debug'

[daemons.demo]
run = {run}
mise = true
ready_output = '^READY$'
stop_signal = 'SIGINT'
"#,
        run = toml::Value::String(run),
    ));
    test.success(&["start", "demo", "--quiet"]);
    let status = test.status("demo");
    let pid: u32 = status["pid"].as_u64().unwrap().try_into().unwrap();
    let parent = RunningProcess::open(pid);
    let child = RunningProcess::open(std::fs::read_to_string(child_pid).unwrap().parse().unwrap());
    let started = Instant::now();
    let stopped = test.success(&["stop", "demo"]);
    let elapsed = started.elapsed();
    let log = test.supervisor_log();
    assert!(
        elapsed >= Duration::from_secs(5),
        "stopped after {elapsed:?}\n{log}"
    );
    let receipt = std::fs::read_to_string(receipt);
    assert_eq!(
        receipt.as_deref().ok(),
        Some("SIGINT"),
        "Ctrl+C receipt: {receipt:?}\n{}\n{log}",
        text(&stopped)
    );
    assert!(
        parent.exited() && child.exited(),
        "{}\n{log}",
        text(&stopped)
    );
    assert_eq!(test.status("demo")["status"], "stopped");
}

#[test]
fn unsupported_windows_options_fail_before_spawning_a_daemon() {
    for (settings, daemon, expected) in [
        ("", "pty = true", "pty = true is not supported on Windows"),
        (
            "",
            "user = 'missing-account'",
            "user switching is not supported on Windows",
        ),
        (
            "[settings.supervisor]\nuser = 'missing-account'\n",
            "",
            "user switching is not supported on Windows",
        ),
    ] {
        let test = NativeTest::new();
        let marker = test.root.join("must-not-exist");
        let run = format!("echo spawned > \"{}\"", marker.display());
        test.config(&format!(
            r#"
{settings}
[daemons.demo]
run = {run}
oneshot = true
{daemon}
"#,
            run = toml::Value::String(run)
        ));
        let output = test.run(&["start", "demo", "--quiet"]);
        assert!(!output.status.success(), "{}", text(&output));
        assert!(text(&output).contains(expected), "{}", text(&output));
        assert!(
            !marker.exists(),
            "unsupported settings must fail before spawning"
        );
    }
}

#[test]
fn unsupported_windows_settings_stop_retries_and_preserve_a_running_daemon() {
    let test = NativeTest::new();
    let exit_marker = test.root.join("exit now");
    let retry_marker = test.root.join("retry hooks.txt");
    let run = format!(
        "Write-Output READY; while (!(Test-Path -LiteralPath '{}')) {{ Start-Sleep -Milliseconds 25 }}; exit 7",
        exit_marker.display()
    );
    let retry_hook = format!(
        "[IO.File]::AppendAllText('{}', 'R')",
        retry_marker.display()
    );
    test.config(&format!(
        r#"
[settings.general]
windows_shell = 'powershell -NoLogo -NoProfile -NonInteractive -Command'
mise = false
interval = '100ms'

[daemons.demo]
run = [{powershell}, '-NoLogo', '-NoProfile', '-NonInteractive', '-Command', {run}]
ready_output = 'READY'
retry = 1

[daemons.demo.hooks]
on_retry = {retry_hook}
"#,
        powershell = toml_string(&test.powershell),
        run = toml::Value::String(run),
        retry_hook = toml::Value::String(retry_hook)
    ));
    test.success(&["start", "demo", "--quiet"]);
    let status = test.status("demo");
    assert_eq!(status["status"], "running", "{status}");
    let original_pid = status["pid"].as_u64().expect("running daemon has a PID");
    test.success(&[
        "settings",
        "set",
        "supervisor.user",
        "missing-account",
        "--global",
    ]);
    let replacement = test.run(&["start", "demo", "--force", "--quiet"]);
    assert!(!replacement.status.success(), "{}", text(&replacement));
    assert!(text(&replacement).contains("user switching is not supported on Windows"));
    let status = test.status("demo");
    assert_eq!(status["status"], "running", "{status}");
    assert_eq!(status["pid"], original_pid, "{status}");

    std::fs::write(exit_marker, "exit").unwrap();
    let deadline = Instant::now() + DEADLINE;
    loop {
        let status = test.status("demo");
        if status["status"] == "failed" {
            assert!(status["pid"].is_null(), "{status}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "retry did not record its validation failure: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Shutdown waits for retry hooks and flushes the detailed failure to disk.
    test.success(&["supervisor", "stop"]);
    let state: toml::Value =
        toml::from_str(&std::fs::read_to_string(test.root.join("state/state.toml")).unwrap())
            .unwrap();
    let daemon = &state["daemons"]["native/demo"];
    assert!(
        daemon["status"]["failed"]
            .as_str()
            .unwrap()
            .contains("user switching is not supported on Windows")
    );
    assert_eq!(daemon["retry_count"].as_integer(), Some(1));
    assert_eq!(std::fs::read_to_string(retry_marker).unwrap(), "R");
}

#[test]
fn native_powershell_daemon_runs_readiness_health_and_lifecycle_hooks() {
    let test = NativeTest::new();
    let marker = test.root.join("probe ready.txt");
    let ready_hook = test.root.join("ready hook.txt");
    let stopped = test.root.join("stop hook.txt");
    let ready = format!("Test-Path -LiteralPath '{}'", marker.display());
    let probe = format!("if ({ready}) {{ exit 0 }} else {{ exit 1 }}");
    let run = format!(
        "[IO.File]::WriteAllText('{}', 'ready'); Write-Output READY; while ($true) {{ Start-Sleep -Milliseconds 100 }}",
        marker.display()
    );
    test.config(&format!(
        r#"
[settings.general]
windows_shell = 'powershell -NoLogo -NoProfile -NonInteractive -Command'
mise = false

[daemons.demo]
run = [{powershell}, '-NoLogo', '-NoProfile', '-NonInteractive', '-Command', {run}]
ready_cmd = {{ run = {probe}, timeout = '20s' }}
health_cmd = {{ run = {probe}, interval = '500ms', timeout = '10s', retries = 1 }}
pty = false
user = ''

[daemons.demo.hooks]
on_ready = {on_ready}
on_stop = {on_stop}
"#,
        powershell = toml_string(&test.powershell),
        run = toml::Value::String(run),
        probe = toml::Value::String(probe),
        on_ready = toml::Value::String(format!(
            "[IO.File]::WriteAllText('{}', 'ready')",
            ready_hook.display()
        )),
        on_stop = toml::Value::String(format!(
            "[IO.File]::WriteAllText('{}', 'stopped')",
            stopped.display()
        ))
    ));
    test.success(&["start", "demo", "--quiet"]);
    assert!(marker.is_file());
    wait_for_file(&ready_hook);
    // Remove the previous process's marker before starting its replacement.
    std::fs::remove_file(&marker).unwrap();
    test.success(&["restart", "demo", "--quiet"]);
    std::fs::remove_file(&marker).unwrap();
    let deadline = Instant::now() + DEADLINE;
    loop {
        let status = test.status("demo");
        if status["status"] == "errored" {
            assert!(status["pid"].is_null(), "{status}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "health failure did not record an errored daemon: {status}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // Start again so the explicit stop exercises the on_stop hook as well.
    test.success(&["start", "demo", "--quiet"]);
    if stopped.exists() {
        std::fs::remove_file(&stopped).unwrap();
    }
    test.success(&["stop", "demo"]);
    wait_for_file(&stopped);
    let logs = text(&test.success(&["logs", "demo", "-n", "50"]));
    assert!(logs.contains("READY"), "{logs}");
}
