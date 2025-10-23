mod common;

use common::{get_script_path, TestEnv};
use std::time::Duration;

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
    println!("elapsed: {:?}", elapsed);
    println!("exit code: {:?}", output.status.code());

    // Key test: daemon fails instantly (before 3s ready check)
    // start command should fail and return quickly (well before 3 seconds)
    assert!(
        !output.status.success(),
        "Start command should fail when daemon exits with code 1"
    );

    // Optional: verify logs
    let logs = env.read_logs("instant_fail");
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
    println!("elapsed: {:?}", elapsed);
    println!("exit code: {:?}", output.status.code());

    // Key test: daemon fails after 2s (before 3s ready check)
    // start command should fail and return around 2s (before 3s)
    assert!(
        !output.status.success(),
        "Start command should fail when daemon exits with code 1 before ready check"
    );
    assert!(
        elapsed >= Duration::from_secs(2),
        "Start should wait at least 2s for daemon to fail, took {:?}",
        elapsed
    );

    // Optional: verify logs
    let logs = env.read_logs("two_sec_fail");
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
    println!("elapsed: {:?}", elapsed);
    println!("exit code: {:?}", output.status.code());

    // Key test: daemon fails after 4s, which is AFTER the 3s ready check
    // start command should succeed and return at ~3s (ready check passes)
    assert!(
        output.status.success(),
        "Start command should succeed when daemon passes 3s ready check even if it fails later"
    );
    assert!(
        elapsed >= Duration::from_secs(3),
        "Start should wait at least 3s for ready check, took {:?}",
        elapsed
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
    println!("List output: {}", stdout);

    assert!(stdout.contains("test_list"), "List should show the daemon");
    assert!(output.status.success(), "List command should succeed");

    // Clean up
    env.run_command(&["stop", "test_list"]);
}

// will be fixed later, logs command is working now
// #[test]

// fn test_logs_command() {
//     let env = TestEnv::new();
//     env.ensure_binary_exists().unwrap();

//     let toml_content = r#"
// [daemons.test_logs]
// run = "sleep 1 && echo 'Test log message' && sleep 5"
// "#;
//     env.create_toml(toml_content);

//     // Start daemon
//     env.run_command(&["start", "test_logs"]);

//     // Check logs
//     let output = env.run_command(&["logs", "test_logs"]);
//     let stdout = String::from_utf8_lossy(&output.stdout);
//     println!("Logs output: {}", stdout);

//     assert!(
//         stdout.contains("Test log message"),
//         "Logs should contain the message"
//     );
//     assert!(output.status.success(), "Logs command should succeed");

//     // Clean up
//     env.run_command(&["stop", "test_logs"]);
// }

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

    println!("Tail output: {}", stdout);

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

    println!("start elapsed: {:?}", start_elapsed);
    assert!(
        start_output.status.success(),
        "Start command should succeed"
    );

    assert!(
        start_elapsed < Duration::from_secs(2),
        "Start should return quickly with ready_delay=0, took {:?}",
        start_elapsed
    );

    // Run wait command in background - it should wait for daemon to exit (~3s total)
    let wait_start = std::time::Instant::now();
    let wait_child = env.run_background(&["wait", "test_wait"]);

    // Wait for daemon to complete
    let output = wait_child
        .wait_with_output()
        .expect("Failed to get wait output");
    let wait_elapsed = wait_start.elapsed();

    println!("wait elapsed: {:?}", wait_elapsed);
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
        "Wait should exit when daemon exits (~3s), took {:?}",
        wait_elapsed
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
    println!("Status output: {}", stdout);

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
    println!("elapsed: {:?}", elapsed);
    println!("exit code: {:?}", output.status.code());

    // Key test: with retry=0, daemon fails immediately, start should fail quickly
    assert!(
        !output.status.success(),
        "Start command should fail when retry=0 and daemon fails"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "Start should return before ready check when daemon fails immediately, took {:?}",
        elapsed
    );

    // Optional: verify only one attempt
    let logs = env.read_logs("retry_zero");
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
    println!("elapsed: {:?}", elapsed);
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
        "Start should take time for retries with backoff, took {:?}",
        elapsed
    );

    // Optional: verify multiple attempts were made
    let logs = env.read_logs("retry_three");
    println!("Logs:\n{}", logs);

    let attempt_count = logs.matches("Failed after 0!").count();

    assert!(
        attempt_count == 4,
        "Should not exceed 4 attempts (1 + 3 retries), got {}",
        attempt_count
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
    println!("elapsed: {:?}", elapsed);
    println!("exit code: {:?}", output.status.code());

    // Key test: with retry, daemon fails twice then succeeds on third attempt
    // First attempt fails immediately, wait 1s, second fails, wait 2s, third succeeds
    assert!(
        output.status.success(),
        "Start command should succeed when daemon eventually succeeds after retries"
    );

    // Optional: verify it did retry and eventually succeed
    let logs = env.read_logs("retry_success");
    println!("Logs:\n{}", logs);

    let attempt_count = logs.matches("Attempt").count();
    assert_eq!(
        attempt_count, 3,
        "Should attempt 3 times before succeeding, got {}",
        attempt_count
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

    println!("Start took: {:?}", elapsed);
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

    println!("Start took: {:?}", elapsed);
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Should be ready after ~1 second when "READY" appears, not wait for 3s delay
    assert!(
        elapsed < Duration::from_secs(3),
        "Should be ready when pattern matches, not wait full delay"
    );
    assert!(output.status.success(), "Start command should succeed");

    let logs = env.read_logs("ready_pattern");
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

    println!("Start took: {:?}", elapsed);
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Should match the regex pattern
    assert!(
        elapsed < Duration::from_secs(3),
        "Should be ready when regex matches"
    );
    assert!(output.status.success(), "Start command should succeed");

    let logs = env.read_logs("ready_regex");
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

    println!("Start took: {:?}", elapsed);
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("exit code: {:?}", output.status.code());

    // When ready_output is set but never matches, it blocks until daemon exits
    // slowly_output.ts outputs 5 times with 1s interval, so runs for ~5 seconds
    assert!(
        elapsed >= Duration::from_secs(3),
        "Should block until daemon exits (~3s), took {:?}",
        elapsed
    );

    // Check logs to verify daemon did run
    let logs = env.read_logs("ready_no_match");
    println!("Logs:\n{}", logs);
    assert!(
        logs.contains("Output 3/3"),
        "Logs should show daemon ran to completion"
    );

    let _ = env.run_command(&["stop", "ready_no_match"]);
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

    println!("Start took: {:?}", elapsed);

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
    println!("List: {}", stdout);

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
    println!("elapsed: {:?}", elapsed);
    println!("exit code: {:?}", output.status.code());

    env.sleep(Duration::from_secs(3));

    // Key test: with retry=2 and ready_delay=1, should retry multiple times
    // Each attempt fails quickly, then retries with backoff
    assert!(
        !output.status.success(),
        "Start command should fail when all retry attempts fail"
    );

    // Optional: verify multiple attempts
    let logs = env.read_logs("retry_ready");
    println!("Logs:\n{}", logs);

    let attempt_count = logs.matches("Failed after 0!").count();
    assert!(
        attempt_count > 1,
        "Should retry multiple times, got {}",
        attempt_count
    );

    let _ = env.run_command(&["stop", "retry_ready"]);
}
