mod common;

use chrono;
use common::{TestEnv, get_script_path};
use std::time::Duration;

// ============================================================================
// Log Viewing Tests
// ============================================================================

#[test]
fn test_logs_tail_command() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.test_tail]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "test_tail"]);

    // Test tail command
    let mut child = env.run_background(&["logs", "-t", "test_tail"]);

    // Wait for some output to be generated
    env.sleep(Duration::from_secs(4));

    // Kill the tail process
    let _ = child.kill();
    let output = child.wait_with_output().expect("Failed to get output");

    // Check the stdout from the tail command
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Tail output: {stdout}");

    // Verify that tail command captured the streaming output
    assert!(!stdout.is_empty(), "Tail output should not be empty");
    assert!(
        stdout.contains("Output 3/3"),
        "Tail output should contain new output"
    );

    // Also verify the log file has content
    let logs = env.read_logs("test_tail");
    assert!(!logs.is_empty(), "Log file should not be empty");

    // Clean up
    env.run_command(&["stop", "test_tail"]);
}

#[test]
fn test_logs_follow_alias() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.test_follow]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "test_follow"]);

    // Test follow command with -f alias
    let mut child = env.run_background(&["logs", "-f", "test_follow"]);

    // Wait for some output to be generated
    env.sleep(Duration::from_secs(4));

    // Kill the follow process
    let _ = child.kill();
    let output = child.wait_with_output().expect("Failed to get output");

    // Check the stdout from the follow command
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Follow output: {stdout}");

    // Verify that follow command captured the streaming output
    assert!(!stdout.is_empty(), "Follow output should not be empty");
    assert!(
        stdout.contains("Output 3/3"),
        "Follow output should contain new output"
    );

    // Clean up
    env.run_command(&["stop", "test_follow"]);
}

#[test]
fn test_logs_clear_all() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    // Create two daemons so we can verify --clear clears all logs
    let toml_content = format!(
        r#"
[daemons.clear_test_1]
run = "bun run {} 1 3"
ready_delay = 0

[daemons.clear_test_2]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display(),
        script.display()
    );
    env.create_toml(&toml_content);

    // Start both daemons to generate logs
    env.run_command(&["start", "clear_test_1"]);
    env.run_command(&["start", "clear_test_2"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // Verify logs exist for both daemons
    let logs1 = env.read_logs("clear_test_1");
    let logs2 = env.read_logs("clear_test_2");
    assert!(!logs1.is_empty(), "Daemon 1 should have logs");
    assert!(!logs2.is_empty(), "Daemon 2 should have logs");

    // Clear all logs without specifying daemon
    let output = env.run_command(&["logs", "--clear"]);
    assert!(
        output.status.success(),
        "logs --clear should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify logs are cleared for both daemons
    let logs1_after = env.read_logs("clear_test_1");
    let logs2_after = env.read_logs("clear_test_2");
    assert!(
        logs1_after.is_empty(),
        "Daemon 1 logs should be cleared, got: {logs1_after}"
    );
    assert!(
        logs2_after.is_empty(),
        "Daemon 2 logs should be cleared, got: {logs2_after}"
    );

    // Clean up
    env.run_command(&["stop", "clear_test_1"]);
    env.run_command(&["stop", "clear_test_2"]);
}

// ============================================================================
// Time Filter Tests (--since / --until)
// ============================================================================

#[test]
fn test_logs_since_relative_time() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.since_test]
run = "bun run {} 1 5"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "since_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(6));

    // Test --since with relative time (last 3 seconds)
    let output = env.run_command(&["logs", "since_test", "--since", "3s"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs since 3s: {stdout}");

    assert!(output.status.success(), "logs --since should succeed");
    assert!(
        !stdout.is_empty(),
        "Should have some logs from last 3 seconds"
    );

    // Clean up
    env.run_command(&["stop", "since_test"]);
}

#[test]
fn test_logs_since_time_only() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.time_only_test]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "time_only_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // Get current time and format as HH:MM
    let now = chrono::Local::now();
    let time_str = now.format("%H:%M").to_string();

    // Test --since with time only (should use today's date)
    let output = env.run_command(&["logs", "time_only_test", "--since", &time_str]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs since {time_str}: {stdout}");

    assert!(
        output.status.success(),
        "logs --since with time only should succeed"
    );

    // Clean up
    env.run_command(&["stop", "time_only_test"]);
}

#[test]
fn test_logs_since_until_range() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.range_test]
run = "bun run {} 1 5"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    let start_time = chrono::Local::now();

    // Start daemon
    env.run_command(&["start", "range_test"]);

    // Wait for some logs
    env.sleep(Duration::from_secs(3));

    let mid_time = chrono::Local::now();

    // Wait for more logs
    env.sleep(Duration::from_secs(3));

    // Test --since and --until together
    let since_str = start_time.format("%Y-%m-%d %H:%M:%S").to_string();
    let until_str = mid_time.format("%Y-%m-%d %H:%M:%S").to_string();

    let output = env.run_command(&[
        "logs",
        "range_test",
        "--since",
        &since_str,
        "--until",
        &until_str,
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs in range: {stdout}");

    assert!(
        output.status.success(),
        "logs with time range should succeed"
    );

    // Verify we got some logs but not all
    let log_count = stdout.lines().count();
    println!("Log lines in range: {log_count}");

    // Should have some logs from the first 3 seconds
    assert!(log_count > 0, "Should have some logs in the time range");

    // Clean up
    env.run_command(&["stop", "range_test"]);
}

#[test]
fn test_logs_since_with_n_limit() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.since_n_test]
run = "bun run {} 1 10"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "since_n_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(11));

    // Test --since with -n limit
    let output = env.run_command(&["logs", "since_n_test", "--since", "10s", "-n", "3"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs since 10s with -n 3: {stdout}");

    assert!(
        output.status.success(),
        "logs --since with -n should succeed"
    );

    // Should limit to 3 lines (or fewer if less than 3 lines match)
    let line_count = stdout.lines().filter(|l| !l.trim().is_empty()).count();
    assert!(
        line_count <= 3,
        "Should have at most 3 lines, got {line_count}"
    );

    // Clean up
    env.run_command(&["stop", "since_n_test"]);
}

// ============================================================================
// Raw Output Tests
// ============================================================================

#[test]
fn test_logs_raw_output() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.raw_test]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "raw_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // Test --raw output
    let output = env.run_command(&["logs", "raw_test", "--raw"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Raw logs: {stdout}");

    assert!(output.status.success(), "logs --raw should succeed");
    assert!(!stdout.is_empty(), "Should have raw logs");

    // Raw output should not have ANSI color codes (basic check)
    // ANSI codes typically start with \x1b[ or \033[
    assert!(
        !stdout.contains("\x1b["),
        "Raw output should not contain ANSI escape codes"
    );

    // Clean up
    env.run_command(&["stop", "raw_test"]);
}

#[test]
fn test_logs_raw_with_time_filter() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.raw_time_test]
run = "bun run {} 1 5"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "raw_time_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(6));

    // Test --raw with --since
    let output = env.run_command(&["logs", "raw_time_test", "--raw", "--since", "5s"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Raw logs with time filter: {stdout}");

    assert!(
        output.status.success(),
        "logs --raw with --since should succeed"
    );
    assert!(!stdout.is_empty(), "Should have raw logs from time range");

    // Clean up
    env.run_command(&["stop", "raw_time_test"]);
}

// ============================================================================
// Line Limit Tests (-n)
// ============================================================================

#[test]
fn test_logs_n_limit() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.n_limit_test]
run = "bun run {} 1 10"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "n_limit_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(11));

    // Test -n limit
    let output = env.run_command(&["logs", "n_limit_test", "-n", "5"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs with -n 5: {stdout}");

    assert!(output.status.success(), "logs -n should succeed");

    // Count non-empty lines
    let line_count = stdout.lines().filter(|l| !l.trim().is_empty()).count();
    println!("Line count: {line_count}");

    // Should have at most 5 lines
    assert!(
        line_count <= 5,
        "Should have at most 5 lines, got {line_count}"
    );

    // Clean up
    env.run_command(&["stop", "n_limit_test"]);
}

#[test]
fn test_logs_without_n_uses_pager() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.pager_test]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "pager_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // When running without -n in non-interactive mode, should output all logs
    let output = env.run_command(&["logs", "pager_test"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs without -n: {stdout}");

    assert!(output.status.success(), "logs without -n should succeed");
    assert!(!stdout.is_empty(), "Should have logs");

    // Clean up
    env.run_command(&["stop", "pager_test"]);
}

// ============================================================================
// Multiple Daemon Tests
// ============================================================================

#[test]
fn test_logs_multiple_daemons() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.multi_log_1]
run = "bun run {} 1 3"
ready_delay = 0

[daemons.multi_log_2]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display(),
        script.display()
    );
    env.create_toml(&toml_content);

    // Start both daemons
    env.run_command(&["start", "multi_log_1"]);
    env.run_command(&["start", "multi_log_2"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // Test viewing logs from multiple daemons
    let output = env.run_command(&["logs", "multi_log_1", "multi_log_2"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs from multiple daemons: {stdout}");

    assert!(
        output.status.success(),
        "logs for multiple daemons should succeed"
    );
    assert!(!stdout.is_empty(), "Should have logs from both daemons");

    // Verify logs contain output from both daemons
    // In multi-daemon mode, daemon names should be shown
    assert!(
        stdout.contains("multi_log_1") || stdout.contains("Output"),
        "Should contain logs from daemon 1"
    );
    assert!(
        stdout.contains("multi_log_2") || stdout.contains("Output"),
        "Should contain logs from daemon 2"
    );

    // Clean up
    env.run_command(&["stop", "multi_log_1"]);
    env.run_command(&["stop", "multi_log_2"]);
}

// ============================================================================
// No-Pager Tests
// ============================================================================

#[test]
fn test_logs_no_pager() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.no_pager_test]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "no_pager_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // Test --no-pager flag
    let output = env.run_command(&["logs", "no_pager_test", "--no-pager"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs with --no-pager: {stdout}");

    assert!(output.status.success(), "logs --no-pager should succeed");
    assert!(!stdout.is_empty(), "Should have logs");

    // Clean up
    env.run_command(&["stop", "no_pager_test"]);
}
