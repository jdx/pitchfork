mod common;

use common::{TestEnv, get_script_path};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

fn run_command_exits_before_marker(
    env: &TestEnv,
    args: &[&str],
    marker: &Path,
    timeout: Duration,
    message: &str,
) -> std::process::Output {
    let mut child = env.run_background(args);
    let deadline = std::time::Instant::now() + timeout;

    loop {
        if child.try_wait().unwrap().is_some() {
            assert!(!marker.exists(), "{message}");
            return child.wait_with_output().unwrap();
        }

        assert!(!marker.exists(), "{message}");
        assert!(
            std::time::Instant::now() < deadline,
            "pitchfork command did not exit before timeout: {message}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]

fn test_instant_fail_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");
    let toml_content = format!(
        r#"
[daemons.instant_fail]
run = "bun run {} 0"
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "instant_fail"]);
    let elapsed = start_time.elapsed();

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    println!("elapsed: {elapsed:?}");
    println!("exit code: {:?}", output.status.code());

    // Key test: daemon fails instantly (before 3s ready check)
    // start command should fail and return quickly (well before 3 seconds)
    assert!(
        !output.status.success(),
        "Start command should fail when daemon exits with code 1"
    );

    // Optional: verify logs
    let logs = env.wait_for_logs("instant_fail", "Failed after 0!", Duration::from_secs(5));
    assert!(
        logs.contains("Failed after 0!"),
        "Logs should contain 'Failed after 0!'"
    );

    let _ = env.run_command(&["stop", "instant_fail"]);
}

#[test]

fn test_two_second_fail_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");
    let toml_content = format!(
        r#"
[daemons.two_sec_fail]
run = "bun run {} 2"
retry = 0
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "two_sec_fail"]);
    let elapsed = start_time.elapsed();

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("elapsed: {elapsed:?}");
    println!("exit code: {:?}", output.status.code());

    // Key test: daemon fails after 2s (before 3s ready check)
    // start command should fail and return around 2s (before 3s)
    assert!(
        !output.status.success(),
        "Start command should fail when daemon exits with code 1 before ready check"
    );
    assert!(
        elapsed >= Duration::from_secs(2),
        "Start should wait at least 2s for daemon to fail, took {elapsed:?}"
    );

    // Optional: verify logs
    let logs = env.wait_for_logs("two_sec_fail", "Failed after 2!", Duration::from_secs(5));
    assert!(
        logs.contains("Failed after 2!"),
        "Logs should contain failure message"
    );

    let _ = env.run_command(&["stop", "two_sec_fail"]);
}

#[test]

fn test_four_second_fail_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");
    let toml_content = format!(
        r#"
[daemons.four_sec_fail]
run = "bun run {} 4"
retry = 0
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "four_sec_fail"]);
    let elapsed = start_time.elapsed();

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("elapsed: {elapsed:?}");
    println!("exit code: {:?}", output.status.code());

    // Key test: daemon fails after 4s, which is AFTER the 3s ready check
    // start command should succeed and return at ~3s (ready check passes)
    assert!(
        output.status.success(),
        "Start command should succeed when daemon passes 3s ready check even if it fails later"
    );
    assert!(
        elapsed >= Duration::from_secs(3),
        "Start should wait at least 3s for ready check, took {elapsed:?}"
    );

    let _ = env.run_command(&["stop", "four_sec_fail"]);
}

// ============================================================================
// CLI Commands Tests
// ============================================================================

#[test]
fn test_list_command() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.test_list]
run = "sleep 10"
"#;
    env.create_toml(toml_content);

    // Start a daemon
    env.run_command(&["start", "test_list"]);

    // Run list command
    let output = env.run_command(&["list"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("List output: {stdout}");

    assert!(stdout.contains("test_list"), "List should show the daemon");
    assert!(output.status.success(), "List command should succeed");

    // Clean up
    env.run_command(&["stop", "test_list"]);
}

#[test]
fn test_list_shows_error_messages() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");
    let toml_content = format!(
        r#"
[daemons.list_error_test]
run = "bun run {} 0"
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    // Start the daemon (it will fail instantly)
    let _ = env.run_command(&["start", "list_error_test"]);

    // Give it a moment to register the failure
    env.sleep(Duration::from_millis(500));

    // Run list command and check for error message
    let output = env.run_command(&["list"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("List output: {stdout}");

    assert!(
        stdout.contains("list_error_test"),
        "List should show the daemon"
    );
    assert!(
        stdout.contains("exit code"),
        "List should show the exit code error message, got: {stdout}"
    );
    assert!(output.status.success(), "List command should succeed");
}

#[test]
fn test_list_shows_available_daemons() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.available_daemon]
run = "sleep 10"

[daemons.running_daemon]
run = "sleep 10"
"#;
    env.create_toml(toml_content);

    // Start only one daemon, leaving the other as "available"
    env.run_command(&["start", "running_daemon"]);
    env.sleep(Duration::from_millis(500));

    // Run list command
    let output = env.run_command(&["list"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("List output: {stdout}");

    // Check that both daemons are shown
    assert!(
        stdout.contains("available_daemon"),
        "List should show the available daemon"
    );
    assert!(
        stdout.contains("running_daemon"),
        "List should show the running daemon"
    );

    // Check that available daemon is marked as "available"
    assert!(
        stdout.contains("available_daemon") && stdout.contains("available"),
        "Available daemon should be marked as 'available', got: {stdout}"
    );

    // Check that running daemon is not marked as "available"
    let lines: Vec<&str> = stdout.lines().collect();
    let running_line = lines
        .iter()
        .find(|line| line.contains("running_daemon"))
        .expect("Should find running_daemon line");
    assert!(
        !running_line.contains("available"),
        "Running daemon should not be marked as 'available', got: {running_line}"
    );

    assert!(output.status.success(), "List command should succeed");

    // Clean up
    env.run_command(&["stop", "running_daemon"]);
}

#[test]

fn test_wait_command() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.test_wait]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon - should return immediately (ready_delay = 0)
    let start_time = std::time::Instant::now();
    let start_output = env.run_command(&["start", "test_wait"]);
    let start_elapsed = start_time.elapsed();

    println!("start elapsed: {start_elapsed:?}");
    assert!(
        start_output.status.success(),
        "Start command should succeed"
    );

    assert!(
        start_elapsed < Duration::from_secs(2),
        "Start should return quickly with ready_delay=0, took {start_elapsed:?}"
    );

    // Run wait command in background - it should wait for daemon to exit (~3s total)
    let wait_start = std::time::Instant::now();
    let wait_child = env.run_background(&["wait", "test_wait"]);

    // Wait for daemon to complete
    let output = wait_child
        .wait_with_output()
        .expect("Failed to get wait output");
    let wait_elapsed = wait_start.elapsed();

    println!("wait elapsed: {wait_elapsed:?}");
    println!("wait stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("wait stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Key test: wait should return when daemon exits (after ~3s)
    // slowly_output.ts outputs 3 times with 1s interval, so it runs for 3s
    assert!(
        output.status.success(),
        "Wait command should succeed when daemon completes"
    );
    assert!(
        wait_elapsed >= Duration::from_secs(2) && wait_elapsed < Duration::from_secs(6),
        "Wait should exit when daemon exits (~3s), took {wait_elapsed:?}"
    );

    // Clean up
    let _ = env.run_command(&["stop", "test_wait"]);
}

#[test]

fn test_status_command() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.test_status]
run = "sleep 10"
"#;
    env.create_toml(toml_content);

    // Start daemon
    env.run_command(&["start", "test_status"]);
    env.sleep(Duration::from_secs(1));

    // Check status
    let output = env.run_command(&["status", "test_status"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("Status output: {stdout}");

    assert!(output.status.success(), "Status command should succeed");

    // Clean up
    env.run_command(&["stop", "test_status"]);
}

// ============================================================================
// Retry Tests
// ============================================================================

#[test]

fn test_retry_zero() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");
    let toml_content = format!(
        r#"
[daemons.retry_zero]
run = "bun run {} 0"
retry = 0
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "retry_zero"]);
    let elapsed = start_time.elapsed();

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("elapsed: {elapsed:?}");
    println!("exit code: {:?}", output.status.code());

    // Key test: with retry=0, daemon fails immediately, start should fail quickly
    assert!(
        !output.status.success(),
        "Start command should fail when retry=0 and daemon fails"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "Start should return before ready check when daemon fails immediately, took {elapsed:?}"
    );

    // Optional: verify only one attempt
    let logs = env.wait_for_logs("retry_zero", "Failed after 0!", Duration::from_secs(5));
    let attempt_count = logs.matches("Failed after 0!").count();
    assert_eq!(attempt_count, 1, "Should only attempt once with retry=0");

    let _ = env.run_command(&["stop", "retry_zero"]);
}

#[test]

fn test_retry_three() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");
    let toml_content = format!(
        r#"
[daemons.retry_three]
run = "bun run {} 0"
ready_delay = 1  # wait 1s for ready check (daemon fails immediately)
retry = 3  # exponential backoff: 1s, 2s, 4s between retries
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "retry_three"]);
    let elapsed = start_time.elapsed();

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("elapsed: {elapsed:?}");
    println!("exit code: {:?}", output.status.code());

    // Key test: even with retry=3, if all attempts fail, start should ultimately fail
    // With exponential backoff and multiple retries, this should take some time
    assert!(
        !output.status.success(),
        "Start command should fail when all retry attempts fail"
    );
    // Each retry has exponential backoff, so it should take at least a few seconds
    assert!(
        elapsed >= Duration::from_secs(7), // 1 + 2 + 4
        "Start should take time for retries with backoff, took {elapsed:?}"
    );

    // Optional: verify multiple attempts were made
    let logs = env.wait_for_logs("retry_three", "Failed after 0!", Duration::from_secs(10));
    println!("Logs:\n{logs}");

    let attempt_count = logs.matches("Failed after 0!").count();

    assert!(
        attempt_count == 4,
        "Should not exceed 4 attempts (1 + 3 retries), got {attempt_count}"
    );

    let _ = env.run_command(&["stop", "retry_three"]);
}

#[test]

fn test_retry_success_on_third() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Generate a unique timestamp for this test run
    let test_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    let success_script = get_script_path("success_on_third.ts");
    let toml_content = format!(
        r#"
[daemons.retry_success]
run = "bun run {}"
ready_delay = 1  
retry = 2  # totally 3 attempts
"#,
        success_script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command_with_env(
        &["start", "retry_success"],
        &[("TEST_SUCCESS_ON_THIRD_TIMESTAMP", &test_timestamp)],
    );
    let elapsed = start_time.elapsed();

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("elapsed: {elapsed:?}");
    println!("exit code: {:?}", output.status.code());

    // Key test: with retry, daemon fails twice then succeeds on third attempt
    // First attempt fails immediately, wait 1s, second fails, wait 2s, third succeeds
    assert!(
        output.status.success(),
        "Start command should succeed when daemon eventually succeeds after retries"
    );

    // Optional: verify it did retry and eventually succeed
    let logs = env.wait_for_logs("retry_success", "Success!", Duration::from_secs(10));
    println!("Logs:\n{logs}");

    let attempt_count = logs.matches("Attempt").count();
    assert_eq!(
        attempt_count, 3,
        "Should attempt 3 times before succeeding, got {attempt_count}"
    );
    assert!(logs.contains("Success!"), "Should eventually succeed");

    let _ = env.run_command(&["stop", "retry_success"]);
}

// ============================================================================
// Ready Check Tests
// ============================================================================

#[test]

fn test_ready_delay_custom() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.custom_delay]
run = "echo 'Starting' && sleep 10"
ready_delay = 1
"#;
    env.create_toml(toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "custom_delay"]);
    let elapsed = start_time.elapsed();

    println!("Start took: {elapsed:?}");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));

    // With ready_delay=1, should be ready in ~1 second, not 3
    assert!(
        elapsed < Duration::from_secs(3),
        "Should be ready in less than 3 seconds with ready_delay=1"
    );
    assert!(output.status.success(), "Start command should succeed");

    let _ = env.run_command(&["stop", "custom_delay"]);
}

#[test]

fn test_ready_output_pattern() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.ready_pattern]
run = "echo 'Starting...' && sleep 1 && echo 'Server is READY' && sleep 10"
ready_output = "READY"
"#;
    env.create_toml(toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "ready_pattern"]);
    let elapsed = start_time.elapsed();

    println!("Start took: {elapsed:?}");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Should be ready after ~1 second when "READY" appears, not wait for 3s delay
    assert!(
        elapsed < Duration::from_secs(3),
        "Should be ready when pattern matches, not wait full delay"
    );
    assert!(output.status.success(), "Start command should succeed");

    let logs = env.wait_for_logs("ready_pattern", "READY", Duration::from_secs(5));
    assert!(logs.contains("READY"), "Logs should contain READY message");

    let _ = env.run_command(&["stop", "ready_pattern"]);
}

#[test]

fn test_ready_output_regex() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.ready_regex]
run = "echo 'Starting server on port 8080' && sleep 1 && echo 'Listening on http://localhost:8080' && sleep 10"
ready_output = "Listening on http://.*:(\\d+)"
"#;
    env.create_toml(toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "ready_regex"]);
    let elapsed = start_time.elapsed();

    println!("Start took: {elapsed:?}");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Should match the regex pattern
    assert!(
        elapsed < Duration::from_secs(3),
        "Should be ready when regex matches"
    );
    assert!(output.status.success(), "Start command should succeed");

    let logs = env.wait_for_logs("ready_regex", "Listening on", Duration::from_secs(5));
    assert!(
        logs.contains("Listening on"),
        "Logs should contain the ready message"
    );

    let _ = env.run_command(&["stop", "ready_regex"]);
}

#[test]

fn test_ready_output_no_match_blocks() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.ready_no_match]
run = "bun run {} 1 3"
ready_output = "NEVER_APPEARS"
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "ready_no_match"]);
    let elapsed = start_time.elapsed();

    println!("Start took: {elapsed:?}");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("exit code: {:?}", output.status.code());

    // When ready_output is set but never matches, it blocks until daemon exits
    // slowly_output.ts outputs 5 times with 1s interval, so runs for ~5 seconds
    assert!(
        elapsed >= Duration::from_secs(3),
        "Should block until daemon exits (~3s), took {elapsed:?}"
    );

    // Check logs to verify daemon did run
    let logs = env.wait_for_logs("ready_no_match", "Output 3/3", Duration::from_secs(5));
    println!("Logs:\n{logs}");
    assert!(
        logs.contains("Output 3/3"),
        "Logs should show daemon ran to completion"
    );

    let _ = env.run_command(&["stop", "ready_no_match"]);
}

#[test]
fn test_fail_output_before_ready_fails_startup() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker_file = env.marker_path("fail_output_before_ready_marker");
    let marker_path = marker_file.display().to_string();

    let toml_content = r#"
[daemons.fail_output_before_ready]
run = "sh -c 'echo Starting && sleep 1 && echo migration failed && sleep 2 && touch {marker_path} && sleep 30'"
ready_output = "READY"
fail_output = "migration failed"
retry = 0
"#;
    env.create_toml(&toml_content.replace("{marker_path}", &marker_path));

    let output = run_command_exits_before_marker(
        &env,
        &["start", "fail_output_before_ready"],
        &marker_file,
        Duration::from_secs(30),
        "start returned before failure marker",
    );

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    println!("exit code: {:?}", output.status.code());

    assert!(
        !output.status.success(),
        "Start command should fail when fail_output matches before ready"
    );

    let logs = env.wait_for_logs(
        "fail_output_before_ready",
        "migration failed",
        Duration::from_secs(5),
    );
    assert!(
        logs.contains("migration failed"),
        "Logs should include the failure output"
    );
    env.wait_for_status("fail_output_before_ready", "errored");
}

#[test]
fn test_fail_output_wins_over_ready_output_on_same_line() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker_file = env.marker_path("fail_output_priority_marker");
    let marker_path = marker_file.display().to_string();

    let toml_content = r#"
[daemons.fail_output_priority]
run = "sh -c 'echo READY but migration failed && sleep 2 && touch {marker_path} && sleep 30'"
ready_output = "READY"
fail_output = "migration failed"
retry = 0
"#;
    env.create_toml(&toml_content.replace("{marker_path}", &marker_path));

    let output = run_command_exits_before_marker(
        &env,
        &["start", "fail_output_priority"],
        &marker_file,
        Duration::from_secs(30),
        "fail_output was not reported before marker file was created",
    );

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(
        !output.status.success(),
        "fail_output should take precedence over ready_output on the same line"
    );

    let logs = env.wait_for_logs(
        "fail_output_priority",
        "READY but migration failed",
        Duration::from_secs(5),
    );
    assert!(logs.contains("READY but migration failed"));
    env.wait_for_status("fail_output_priority", "errored");
}

#[test]
fn test_fail_output_ignored_after_ready() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker_file = env.marker_path("fail_output_after_ready_runtime");
    let marker_path = marker_file.display().to_string();

    let toml_content = format!(
        r#"
[daemons.fail_output_after_ready]
run = "sh -c 'echo READY && sleep 1 && echo runtime failed && touch {marker_path} && sleep 30'"
ready_output = "READY"
fail_output = "runtime failed"
retry = 0
"#
    );
    env.create_toml(&toml_content);
    let output = run_command_exits_before_marker(
        &env,
        &["start", "fail_output_after_ready"],
        &marker_file,
        Duration::from_secs(30),
        "start command did not exit before runtime fail marker",
    );

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "Start command should succeed once ready_output matches"
    );

    let logs = env.wait_for_logs(
        "fail_output_after_ready",
        "runtime failed",
        Duration::from_secs(5),
    );
    assert!(logs.contains("runtime failed"));
    env.wait_for_status("fail_output_after_ready", "running");

    let _ = env.run_command(&["stop", "fail_output_after_ready"]);
}

#[test]
fn test_fail_output_triggers_retry() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.fail_output_retry]
run = "sh -c 'echo fatal boot && sleep 30'"
ready_output = "READY"
fail_output = "fatal boot"
retry = 1
"#;
    env.create_toml(toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "fail_output_retry"]);
    let elapsed = start_time.elapsed();

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    println!("elapsed: {elapsed:?}");

    assert!(
        !output.status.success(),
        "Start command should fail after fail_output exhausts retries"
    );
    assert!(
        elapsed >= Duration::from_secs(1),
        "Retry backoff should run after the first fail_output match"
    );

    let logs = env.wait_for_logs("fail_output_retry", "fatal boot", Duration::from_secs(5));
    let attempt_count = logs.matches("fatal boot").count();
    assert_eq!(
        attempt_count, 2,
        "Should run original attempt plus one retry, logs:\n{logs}"
    );
    env.wait_for_status("fail_output_retry", "errored");
}

#[test]
fn test_fail_output_retry_waits_for_stop_before_next_attempt() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    let project_dir = env.create_project_dir();

    let lock_dir = project_dir.join("fail_output_retry_lock");
    let script_path = project_dir.join("fail-output-retry.sh");
    let script = format!(
        r#"#!/bin/sh
set -eu

trap 'echo "[trap] term"; sleep 2; rmdir "{lock_dir}"; echo "[trap] done"; exit 0' HUP INT QUIT TERM USR1 USR2

if mkdir "{lock_dir}"; then
    echo "fatal boot"
    while :; do :; done
else
    echo "overlap detected"
    echo "fatal boot"
    while :; do :; done
fi
"#,
        lock_dir = lock_dir.display()
    );
    fs::write(&script_path, script).unwrap();
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&script_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).unwrap();
    }

    let toml_content = format!(
        r#"
[daemons.fail_output_retry_waits]
run = "{}"
ready_output = "READY"
fail_output = "fatal boot"
retry = 1
stop_signal = {{ signal = "SIGTERM", timeout = "5s" }}
env = {{ LOCKDIR = "{}" }}
"#,
        script_path.display(),
        lock_dir.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "fail_output_retry_waits"]);

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    println!("exit code: {:?}", output.status.code());

    assert!(
        !output.status.success(),
        "Start command should fail when all retry attempts are exhausted"
    );

    let logs = env.wait_for_logs(
        "fail_output_retry_waits",
        "fatal boot",
        Duration::from_secs(5),
    );
    let attempt_count = logs.matches("fatal boot").count();
    assert_eq!(
        attempt_count, 2,
        "Should run two attempts: one retry and one final, logs:\n{logs}"
    );
    assert!(
        !logs.contains("overlap detected"),
        "Retry should wait for previous attempt to stop before next start"
    );
    env.wait_for_status("fail_output_retry_waits", "errored");
}

#[test]
fn test_fail_output_reports_before_stop_timeout() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker_file = env.marker_path("fail_output_stop_timeout_cleanup");
    let marker_path = marker_file.display().to_string();

    let toml_content = format!(
        r#"
[daemons.fail_output_stop_timeout]
run = "sh -c 'trap \"sleep 2; touch {marker_path}; exit 0\" TERM; echo fatal boot; while :; do sleep 1; done'"
ready_output = "READY"
fail_output = "fatal boot"
retry = 0
stop_signal = {{ signal = "SIGTERM", timeout = "5s" }}
"#
    );
    env.create_toml(&toml_content);

    let output = run_command_exits_before_marker(
        &env,
        &["start", "fail_output_stop_timeout"],
        &marker_file,
        Duration::from_secs(30),
        "fail_output should be reported before cleanup marker is created",
    );

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(
        !output.status.success(),
        "Start command should fail when fail_output matches before ready"
    );

    let logs = env.wait_for_logs(
        "fail_output_stop_timeout",
        "fatal boot",
        Duration::from_secs(5),
    );
    assert!(logs.contains("fatal boot"));
    env.wait_for_status("fail_output_stop_timeout", "errored");
}

#[test]
fn test_fail_output_persisted_in_state_after_start() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.fail_output_state_persist]
run = "sh -c 'echo READY && sleep 30'"
ready_output = "READY"
fail_output = "fatal boot"
retry = 0
"#;
    env.create_toml(toml_content);

    let output = env.run_command(&["start", "fail_output_state_persist"]);
    assert!(
        output.status.success(),
        "Start command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    env.wait_for_status("fail_output_state_persist", "running");

    let state_contents = std::fs::read_to_string(env.state_file_path()).unwrap();
    assert!(
        state_contents.contains("fail_output = \"fatal boot\""),
        "State file should include fail_output config after start"
    );

    let output = env.run_command(&["stop", "fail_output_state_persist"]);
    assert!(
        output.status.success(),
        "Stop command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_fail_output_removed_from_config_clears_state_after_start() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_with_fail_output = r#"
[daemons.fail_output_state_clear]
run = "sh -c 'echo READY && sleep 30'"
ready_output = "READY"
fail_output = "fatal boot"
retry = 0
"#;
    env.create_toml(toml_with_fail_output);

    let output = env.run_command(&["start", "fail_output_state_clear"]);
    assert!(
        output.status.success(),
        "Initial start command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    env.wait_for_status("fail_output_state_clear", "running");

    let output = env.run_command(&["stop", "fail_output_state_clear"]);
    assert!(
        output.status.success(),
        "Stop command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let toml_without_fail_output = r#"
[daemons.fail_output_state_clear]
run = "sh -c 'echo READY && sleep 30'"
ready_output = "READY"
retry = 0
"#;
    env.create_toml(toml_without_fail_output);

    let output = env.run_command(&["start", "fail_output_state_clear"]);
    assert!(
        output.status.success(),
        "Restart after removing fail_output should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    env.wait_for_status("fail_output_state_clear", "running");

    let state_contents = std::fs::read_to_string(env.state_file_path()).unwrap();
    assert!(
        !state_contents.contains("fail_output = \"fatal boot\""),
        "State file should clear removed fail_output config, got:\n{state_contents}"
    );

    let _ = env.run_command(&["stop", "fail_output_state_clear"]);
}

#[test]
fn test_run_fail_output_override() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    let _ = env.create_project_dir();

    let marker_file = env.marker_path("fail_output_run_override");
    let marker_path = marker_file.display().to_string();
    let script = format!(
        "echo Output 1/5; sleep 1; echo Output 2/5; sleep 2; touch {marker_path}; sleep 30"
    );

    let output = run_command_exits_before_marker(
        &env,
        &[
            "run",
            "fail_output_run",
            "--output",
            "NEVER_APPEARS",
            "--fail-output",
            "Output 2/5",
            "--",
            "sh",
            "-c",
            script.as_str(),
        ],
        &marker_file,
        Duration::from_secs(30),
        "run command should fail before marker is created",
    );

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(
        !output.status.success(),
        "run --fail-output should fail when the output pattern appears"
    );

    let logs = env.wait_for_logs(
        "global/fail_output_run",
        "Output 2/5",
        Duration::from_secs(5),
    );
    assert!(logs.contains("Output 2/5"));
    env.wait_for_status("global/fail_output_run", "errored");
}

#[test]

fn test_ready_both_delay_and_output() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.ready_both]
run = "echo 'READY NOW' && sleep 10"
ready_output = "READY"
ready_delay = 5
"#;
    env.create_toml(toml_content);

    let start_time = std::time::Instant::now();
    let _output = env.run_command(&["start", "ready_both"]);
    let elapsed = start_time.elapsed();

    println!("Start took: {elapsed:?}");

    // When both are specified, whichever happens first should trigger ready
    // The output pattern should match first (~0.5s) before the delay (5s)
    assert!(
        elapsed < Duration::from_secs(2),
        "Should be ready when pattern matches, not wait for delay"
    );

    let _ = env.run_command(&["stop", "ready_both"]);
}

// ============================================================================
// Integration Tests - Multiple Features Combined
// ============================================================================

#[test]

fn test_multiple_daemons() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.daemon1]
run = "echo 'Daemon 1' && sleep 10"

[daemons.daemon2]
run = "echo 'Daemon 2' && sleep 10"

[daemons.daemon3]
run = "echo 'Daemon 3' && sleep 10"
"#;
    env.create_toml(toml_content);

    // Start all daemons
    env.run_command(&["start", "--all"]);
    env.sleep(Duration::from_secs(4));

    // List should show all three
    let output = env.run_command(&["list"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("List: {stdout}");

    assert!(stdout.contains("daemon1"), "Should list daemon1");
    assert!(stdout.contains("daemon2"), "Should list daemon2");
    assert!(stdout.contains("daemon3"), "Should list daemon3");

    // Stop all
    env.run_command(&["stop", "--all"]);
}

#[test]

fn test_retry_with_ready_check() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");
    let toml_content = format!(
        r#"
[daemons.retry_ready]
run = "bun run {} 0"
retry = 2
ready_delay = 1
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "retry_ready"]);
    let elapsed = start_time.elapsed();

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("elapsed: {elapsed:?}");
    println!("exit code: {:?}", output.status.code());

    env.sleep(Duration::from_secs(3));

    // Key test: with retry=2 and ready_delay=1, should retry multiple times
    // Each attempt fails quickly, then retries with backoff
    assert!(
        !output.status.success(),
        "Start command should fail when all retry attempts fail"
    );

    // Optional: verify multiple attempts
    let logs = env.wait_for_logs("retry_ready", "Failed after 0!", Duration::from_secs(5));
    println!("Logs:\n{logs}");

    let attempt_count = logs.matches("Failed after 0!").count();
    assert!(
        attempt_count > 1,
        "Should retry multiple times, got {attempt_count}"
    );

    let _ = env.run_command(&["stop", "retry_ready"]);
}

// ============================================================================
// Stop Command Tests
// ============================================================================

#[test]
fn test_ready_http_check() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Clean up any stray processes on port 18081
    #[cfg(unix)]
    env.kill_port(18081);

    let script = get_script_path("http_server.ts");
    // Server starts listening after 1 second delay
    let toml_content = format!(
        r#"
[daemons.http_test]
run = "bun run {} 1 18081"
ready_http = "http://localhost:18081/health"
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "http_test"]);
    let elapsed = start_time.elapsed();

    println!("Start took: {elapsed:?}");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success(), "Start command should succeed");

    // Should take at least 1 second (the delay before server starts)
    assert!(
        elapsed >= Duration::from_secs(1),
        "Should wait for HTTP server to be ready"
    );

    // Should not take too long (less than 10 seconds)
    assert!(
        elapsed < Duration::from_secs(10),
        "Should not take too long to detect ready state"
    );

    let logs = env.wait_for_logs("http_test", "Server listening", Duration::from_secs(5));
    println!("Logs: {logs}");
    assert!(
        logs.contains("Server listening"),
        "Logs should contain server start message, got: {logs}"
    );

    // Clean up
    env.run_command(&["stop", "http_test"]);
}

#[test]
fn test_ready_http_check_with_status() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    #[cfg(unix)]
    env.kill_port(18084);

    let script = get_script_path("http_server.ts");
    let toml_content = format!(
        r#"
[daemons.http_status_test]
run = "bun run {} 1 18084 401"
ready_http = {{ url = "http://localhost:18084/health", status = [401] }}
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "http_status_test"]);
    let elapsed = start_time.elapsed();

    println!("Start took: {elapsed:?}");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success(), "Start command should succeed");
    assert!(
        elapsed >= Duration::from_secs(1),
        "Should wait for HTTP server to be ready"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "Should not take too long to detect ready state"
    );

    env.run_command(&["stop", "http_status_test"]);
}

#[test]
fn test_ready_port_check() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Clean up any stray processes on port 18082
    #[cfg(unix)]
    env.kill_port(18082);

    let script = get_script_path("http_server.ts");
    // Server starts listening after 1 second delay
    let toml_content = format!(
        r#"
[daemons.port_test]
run = "bun run {} 1 18082"
ready_port = 18082
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "port_test"]);
    let elapsed = start_time.elapsed();

    println!("Start took: {elapsed:?}");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success(), "Start command should succeed");

    // Should take at least 1 second (the delay before server starts listening)
    assert!(
        elapsed >= Duration::from_secs(1),
        "Should wait for port to be listening"
    );

    // Should not take too long (less than 10 seconds)
    assert!(
        elapsed < Duration::from_secs(10),
        "Should not take too long to detect ready state"
    );

    let logs = env.wait_for_logs("port_test", "Server listening", Duration::from_secs(5));
    println!("Logs: {logs}");
    assert!(
        logs.contains("Server listening"),
        "Logs should contain server start message, got: {logs}"
    );

    // Clean up
    env.run_command(&["stop", "port_test"]);
}

/// Test ready_cmd check - waits for a command to return exit code 0
#[test]
fn test_ready_cmd_check() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a unique marker file path for this test
    let marker_file = env.marker_path("ready");
    let marker_path = marker_file.display().to_string();

    // Daemon creates a marker file after 1 second delay, then sleeps
    let toml_content = format!(
        r#"
[daemons.cmd_test]
run = "bash -c 'echo Starting; sleep 1; touch {marker_path}; echo Ready; sleep 60'"
ready_cmd = "test -f {marker_path}"
"#
    );
    env.create_toml(&toml_content);

    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "cmd_test"]);
    let elapsed = start_time.elapsed();

    println!("Start took: {elapsed:?}");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success(), "Start command should succeed");

    // Should take at least 1 second (the delay before file is created)
    assert!(
        elapsed >= Duration::from_secs(1),
        "Should wait for ready command to succeed"
    );

    // Should not take too long (less than 10 seconds)
    assert!(
        elapsed < Duration::from_secs(10),
        "Should not take too long to detect ready state"
    );

    // Verify the marker file exists
    assert!(marker_file.exists(), "Marker file should have been created");

    let logs = env.wait_for_logs("cmd_test", "Ready", Duration::from_secs(5));
    println!("Logs: {logs}");
    assert!(
        logs.contains("Starting"),
        "Logs should contain start message, got: {logs}"
    );
    assert!(
        logs.contains("Ready"),
        "Logs should contain ready message, got: {logs}"
    );

    // Clean up
    env.run_command(&["stop", "cmd_test"]);
}

// ============================================================================
// Dir and Env Config Tests
// ============================================================================

/// Test that `dir` config field sets the working directory for the daemon
#[test]
fn test_daemon_dir_relative() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a subdirectory in the project
    let project_dir = env.project_dir();
    let subdir = project_dir.join("mysubdir");
    std::fs::create_dir_all(&subdir).unwrap();

    let marker = env.marker_path("dir_test");

    // The daemon writes its $PWD to a marker file
    let toml_content = format!(
        r#"
[daemons.dir_test]
run = "bash -c 'pwd > \"{}\" && sleep 60'"
dir = "mysubdir"
ready_delay = 1
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "dir_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Wait a moment for the file to be written
    env.sleep(Duration::from_millis(500));

    // Verify the daemon ran in the correct directory
    let pwd = std::fs::read_to_string(&marker).unwrap();
    let pwd = pwd.trim();
    // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
    let expected = subdir.canonicalize().unwrap();
    assert_eq!(
        pwd,
        expected.to_str().unwrap(),
        "Daemon should run in the subdirectory"
    );

    env.run_command(&["stop", "dir_test"]);
}

/// Test that `dir` config field works with absolute paths
#[test]
fn test_daemon_dir_absolute() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create an absolute directory outside the project
    let abs_dir = env.create_other_dir();

    let marker = env.marker_path("dir_abs_test");

    let toml_content = format!(
        r#"
[daemons.dir_abs_test]
run = "bash -c 'pwd > \"{}\" && sleep 60'"
dir = "{}"
ready_delay = 1
"#,
        marker.display(),
        abs_dir.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "dir_abs_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    env.sleep(Duration::from_millis(500));

    let pwd = std::fs::read_to_string(&marker).unwrap();
    let pwd = pwd.trim();
    // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
    let expected = abs_dir.canonicalize().unwrap();
    assert_eq!(
        pwd,
        expected.to_str().unwrap(),
        "Daemon should run in the absolute directory"
    );

    env.run_command(&["stop", "dir_abs_test"]);
}

/// Test that `env` config field sets environment variables for the daemon
#[test]
fn test_daemon_env_vars() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("env_test");

    let toml_content = format!(
        r#"
[daemons.env_test]
run = "bash -c 'echo $MY_TEST_VAR > \"{}\" && sleep 60'"
ready_delay = 1

[daemons.env_test.env]
MY_TEST_VAR = "hello_from_pitchfork"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "env_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    env.sleep(Duration::from_millis(500));

    let value = std::fs::read_to_string(&marker).unwrap();
    let value = value.trim();
    assert_eq!(
        value, "hello_from_pitchfork",
        "Daemon should see the configured env var"
    );

    env.run_command(&["stop", "env_test"]);
}

/// Test that multiple env vars are all available to the daemon
#[test]
fn test_daemon_multiple_env_vars() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("multi_env_test");

    let toml_content = format!(
        r#"
[daemons.multi_env_test]
run = "bash -c 'echo $VAR_A:$VAR_B:$VAR_C > \"{}\" && sleep 60'"
ready_delay = 1
env = {{ VAR_A = "alpha", VAR_B = "beta", VAR_C = "gamma" }}
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "multi_env_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    env.sleep(Duration::from_millis(500));

    let value = std::fs::read_to_string(&marker).unwrap();
    let value = value.trim();
    assert_eq!(
        value, "alpha:beta:gamma",
        "All env vars should be available"
    );

    env.run_command(&["stop", "multi_env_test"]);
}

/// Test that `dir` and `env` work together
#[test]
fn test_daemon_dir_and_env_combined() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project_dir = env.project_dir();
    let subdir = project_dir.join("combined_test_dir");
    std::fs::create_dir_all(&subdir).unwrap();

    let marker = env.marker_path("combined_test");

    let toml_content = format!(
        r#"
[daemons.combined_test]
run = "bash -c 'echo $MY_PORT:$(pwd) > \"{}\" && sleep 60'"
dir = "combined_test_dir"
ready_delay = 1

[daemons.combined_test.env]
MY_PORT = "8080"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "combined_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    env.sleep(Duration::from_millis(500));

    let value = std::fs::read_to_string(&marker).unwrap();
    let value = value.trim();
    // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
    let canonical_subdir = subdir.canonicalize().unwrap();
    let expected = format!("8080:{}", canonical_subdir.display());
    assert_eq!(value, expected, "Both dir and env should work together");

    env.run_command(&["stop", "combined_test"]);
}

// ============================================================================
// Stop Command Tests
// ============================================================================

/// Test that stop command correctly transitions daemon from running to stopped
/// This test verifies the fix for the bug where daemon would get stuck in "stopping" status
/// due to a race condition between sysinfo's process.wait() and tokio's child.wait()
#[test]
fn test_stop_transitions_to_stopped() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.stop_test]
run = "sleep 60"
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Start the daemon
    let output = env.run_command(&["start", "stop_test"]);
    assert!(output.status.success(), "Start command should succeed");

    // Verify daemon is running
    let status = env.get_daemon_status("stop_test");
    println!("Status after start: {status:?}");
    assert_eq!(
        status,
        Some("running".to_string()),
        "Daemon should be running after start"
    );

    // Stop the daemon
    let stop_start = std::time::Instant::now();
    let output = env.run_command(&["stop", "stop_test"]);
    let stop_elapsed = stop_start.elapsed();

    println!("Stop took: {stop_elapsed:?}");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success(), "Stop command should succeed");

    // Key test: Stop should complete quickly (not hang)
    assert!(
        stop_elapsed < Duration::from_secs(5),
        "Stop should complete quickly, took {stop_elapsed:?}"
    );

    // Key test: Daemon status should be "stopped", NOT "stopping"
    env.wait_for_status("stop_test", "stopped");
}

/// Test stop command with a daemon that has child processes
#[test]
fn test_stop_kills_child_processes() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Use bash to spawn a child process
    let toml_content = r#"
[daemons.stop_children_test]
run = "bash -c 'sleep 60 & sleep 60 & wait'"
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Start the daemon
    let output = env.run_command(&["start", "stop_children_test"]);
    assert!(
        output.status.success(),
        "Start command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify daemon is running
    let status = env.get_daemon_status("stop_children_test");
    assert_eq!(
        status,
        Some("running".to_string()),
        "Daemon should be running"
    );

    // Stop the daemon
    let stop_start = std::time::Instant::now();
    let output = env.run_command(&["stop", "stop_children_test"]);
    let stop_elapsed = stop_start.elapsed();

    println!("Stop took: {stop_elapsed:?}");

    assert!(output.status.success(), "Stop command should succeed");

    // Stop should complete in reasonable time even with child processes
    assert!(
        stop_elapsed < Duration::from_secs(10),
        "Stop should complete in reasonable time, took {stop_elapsed:?}"
    );

    // Daemon should be stopped
    env.wait_for_status("stop_children_test", "stopped");
}

/// Test stopping an already stopped daemon
#[test]
fn test_stop_already_stopped() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.already_stopped_test]
run = "sleep 60"
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Start and stop the daemon
    let output = env.run_command(&["start", "already_stopped_test"]);
    assert!(output.status.success(), "Start should succeed");

    let output = env.run_command(&["stop", "already_stopped_test"]);
    assert!(output.status.success(), "First stop should succeed");

    // Wait for the supervisor to fully persist the stopped state.
    // Under CI load, the state file may not be fully written yet.
    env.wait_for_status("already_stopped_test", "stopped");

    // Try to stop again
    let output = env.run_command(&["stop", "already_stopped_test"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("Second stop stdout: {stdout}");

    // Should handle gracefully (either succeed or indicate already stopped)
    // The daemon should still be in stopped state
    env.wait_for_status("already_stopped_test", "stopped");
}

// ============================================================================
// Ad-hoc Daemon Tests
// ============================================================================

/// Test that ad-hoc daemons (started via `pitchfork run`) can now be restarted
/// because their startup command is saved in the state file.
#[test]
fn test_adhoc_daemon_can_be_restarted() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create an empty toml (ad-hoc daemon won't be in it)
    env.create_toml("");

    // Start an ad-hoc daemon using `pitchfork run`
    let output = env.run_command(&["run", "adhoc_test", "--delay", "1", "--", "sleep", "60"]);
    println!("run stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("run stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "Ad-hoc run should succeed");

    // Verify daemon is running
    let status = env.get_daemon_status("adhoc_test");
    println!("Status after run: {status:?}");
    assert_eq!(
        status,
        Some("running".to_string()),
        "Ad-hoc daemon should be running"
    );

    // Get the original PID
    let original_pid = env.get_daemon_pid("adhoc_test");
    println!("Original PID: {original_pid:?}");

    // Restart the ad-hoc daemon - it should now succeed
    let output = env.run_command(&["restart", "adhoc_test"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("restart stdout: {stdout}");
    println!("restart stderr: {stderr}");

    // Give some time for restart to complete
    env.sleep(Duration::from_secs(2));

    // The ad-hoc daemon should be running with a new PID
    let status = env.get_daemon_status("adhoc_test");
    println!("Status after restart: {status:?}");
    assert_eq!(
        status,
        Some("running".to_string()),
        "Ad-hoc daemon should be running after restart"
    );

    // Verify PID changed (daemon was actually restarted)
    let new_pid = env.get_daemon_pid("adhoc_test");
    println!("New PID after restart: {new_pid:?}");
    assert_ne!(original_pid, new_pid, "PID should change after restart");

    // Clean up
    env.run_command(&["stop", "adhoc_test"]);
}

/// Test that `restart --all` also restarts ad-hoc daemons
/// (since they now have saved commands)
#[test]
fn test_restart_all_includes_adhoc_daemons() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a config with one daemon
    let toml_content = r#"
[daemons.config_daemon]
run = "sleep 60"
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Start the config-based daemon
    let output = env.run_command(&["start", "config_daemon"]);
    assert!(
        output.status.success(),
        "Config daemon start should succeed"
    );

    // Start an ad-hoc daemon
    let output = env.run_command(&["run", "adhoc_daemon", "--delay", "1", "--", "sleep", "60"]);
    assert!(output.status.success(), "Ad-hoc run should succeed");

    // Verify both are running
    assert_eq!(
        env.get_daemon_status("config_daemon"),
        Some("running".to_string()),
        "Config daemon should be running"
    );
    assert_eq!(
        env.get_daemon_status("adhoc_daemon"),
        Some("running".to_string()),
        "Ad-hoc daemon should be running"
    );

    // Get original PIDs
    let config_pid = env.get_daemon_pid("config_daemon");
    let adhoc_pid = env.get_daemon_pid("adhoc_daemon");
    println!("Original config_daemon PID: {config_pid:?}");
    println!("Original adhoc_daemon PID: {adhoc_pid:?}");

    // Restart all daemons
    let output = env.run_command(&["restart", "--all"]);
    println!(
        "restart --all stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    println!(
        "restart --all stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Give some time for restart to complete
    env.sleep(Duration::from_secs(2));

    // Config daemon should still be running (was restarted)
    let status = env.get_daemon_status("config_daemon");
    println!("Config daemon status after restart --all: {status:?}");
    assert_eq!(
        status,
        Some("running".to_string()),
        "Config daemon should be running after restart --all"
    );

    // Ad-hoc daemon should also be running (was restarted with saved command)
    let status = env.get_daemon_status("adhoc_daemon");
    println!("Ad-hoc daemon status after restart --all: {status:?}");
    assert_eq!(
        status,
        Some("running".to_string()),
        "Ad-hoc daemon should be running after restart --all"
    );

    // Verify both PIDs changed (both were restarted)
    let new_config_pid = env.get_daemon_pid("config_daemon");
    let new_adhoc_pid = env.get_daemon_pid("adhoc_daemon");
    println!("New config_daemon PID: {new_config_pid:?}");
    println!("New adhoc_daemon PID: {new_adhoc_pid:?}");

    assert_ne!(
        config_pid, new_config_pid,
        "Config daemon PID should change after restart"
    );
    assert_ne!(
        adhoc_pid, new_adhoc_pid,
        "Ad-hoc daemon PID should change after restart"
    );

    // Clean up
    env.run_command(&["stop", "--all"]);
}
