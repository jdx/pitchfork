mod common;

use common::{TestEnv, get_script_path};
use std::time::Duration;

/// Test that ANSI escape codes are preserved in log files by default.
#[test]
fn test_log_preserves_ansi_by_default() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("ansi_output.ts");
    let toml_content = format!(
        r#"
[daemons.ansi_test]
run = "sh -c 'bun run {} 32 green && sleep 60'"
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "ansi_test"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "start should succeed. stdout: {stdout}, stderr: {stderr}"
    );

    let logs = env.wait_for_logs("ansi_test", "\x1b[32m", Duration::from_secs(5));
    assert!(
        logs.contains("\x1b[32m"),
        "Log file should contain ANSI escape code \\x1b[32m (ANSI preserved by default). Got: {logs:?}"
    );
    assert!(
        logs.contains("green"),
        "Log file should contain the text 'green'. Got: {logs:?}"
    );

    let _ = env.run_command(&["stop", "ansi_test"]);
}

/// Test that `pty = true` allocates a pseudo-terminal for the daemon process.
#[test]
fn test_pty_true_creates_terminal() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    env.create_toml(
        r#"
[daemons.with_pty]
run = "sh -c 'if [ -t 0 ] && [ -t 1 ]; then echo HAS_TTY; else echo NO_TTY; fi && sleep 60'"
pty = true
"#,
    );

    let output = env.run_command(&["start", "with_pty"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "start should succeed. stdout: {stdout}, stderr: {stderr}"
    );

    let logs = env.wait_for_logs("with_pty", "HAS_TTY", Duration::from_secs(5));
    assert!(
        logs.contains("HAS_TTY"),
        "With pty = true, daemon should detect a TTY. Got: {logs:?}"
    );

    let _ = env.run_command(&["stop", "with_pty"]);
}

/// Test that `pty = false` (default) does NOT allocate a pseudo-terminal.
#[test]
fn test_pty_false_no_terminal() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    env.create_toml(
        r#"
[daemons.no_pty]
run = "sh -c 'if [ -t 0 ] && [ -t 1 ]; then echo HAS_TTY; else echo NO_TTY; fi && sleep 60'"
"#,
    );

    let output = env.run_command(&["start", "no_pty"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "start should succeed. stdout: {stdout}, stderr: {stderr}"
    );

    let logs = env.wait_for_logs("no_pty", "NO_TTY", Duration::from_secs(5));
    assert!(
        logs.contains("NO_TTY"),
        "Without pty (default), daemon should NOT detect a TTY. Got: {logs:?}"
    );

    let _ = env.run_command(&["stop", "no_pty"]);
}

/// Test that PTY mode preserves ANSI escape codes in log files.
#[test]
fn test_pty_preserves_ansi() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("ansi_output.ts");
    let toml_content = format!(
        r#"
[daemons.pty_ansi]
run = "sh -c 'bun run {} 31 red && sleep 60'"
pty = true
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "pty_ansi"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "start should succeed. stdout: {stdout}, stderr: {stderr}"
    );

    let logs = env.wait_for_logs("pty_ansi", "\x1b[31m", Duration::from_secs(5));
    assert!(
        logs.contains("\x1b[31m"),
        "Log should contain ANSI escape code \\x1b[31m (ANSI preserved in PTY mode). Got: {logs:?}"
    );
    assert!(
        logs.contains("red"),
        "Log should contain the text 'red'. Got: {logs:?}"
    );

    let _ = env.run_command(&["stop", "pty_ansi"]);
}
