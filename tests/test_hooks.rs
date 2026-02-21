mod common;

use common::TestEnv;
use std::time::Duration;

/// Test that the on_ready hook fires when a daemon becomes ready
#[test]
fn test_hook_on_ready() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_ready");

    let toml_content = format!(
        r#"
[daemons.ready_hook_test]
run = "sleep 60"
ready_delay = 1

[daemons.ready_hook_test.hooks]
on_ready = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "ready_hook_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Wait for hook to execute (fire-and-forget, may take a moment)
    for _ in 0..20 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        marker.exists(),
        "on_ready hook should have created marker file"
    );

    env.run_command(&["stop", "ready_hook_test"]);
}

/// Test that on_fail hook fires when a daemon fails with no retries
#[test]
fn test_hook_on_fail_no_retry() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_fail");

    let toml_content = format!(
        r#"
[daemons.fail_hook_test]
run = "sh -c 'exit 42'"
retry = 0

[daemons.fail_hook_test.hooks]
on_fail = "sh -c 'echo $PITCHFORK_EXIT_CODE > {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "fail_hook_test"]);
    // Daemon fails, start command may fail
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("start stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Poll for marker file (hook fires async)
    for _ in 0..30 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        marker.exists(),
        "on_fail hook should have created marker file"
    );

    let content = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        content.trim(),
        "42",
        "on_fail hook should receive PITCHFORK_EXIT_CODE"
    );
}

/// Test that on_fail hook fires only after retries are exhausted
#[test]
fn test_hook_on_fail_after_retries() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_fail_retry");

    let toml_content = format!(
        r#"
[daemons.fail_retry_hook]
run = "sh -c 'exit 1'"
retry = 2

[daemons.fail_retry_hook.hooks]
on_fail = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    // Start with retry=2 and ready_delay so it uses wait_ready path
    let output = env.run_command(&["start", "fail_retry_hook", "--delay", "1"]);
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Background retries happen on 10s interval, so we need to wait
    // Use PITCHFORK_INTERVAL_SECS=1 to speed this up
    let output = env.run_command_with_env(
        &["start", "fail_retry_hook"],
        &[("PITCHFORK_INTERVAL_SECS", "1")],
    );
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Wait for retries to exhaust and on_fail to fire
    // retry=2 means 3 total attempts (initial + 2 retries), each on ~1s interval
    for _ in 0..50 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    assert!(
        marker.exists(),
        "on_fail hook should fire after retries exhausted"
    );
}

/// Test that on_retry hook fires for each retry attempt
#[test]
fn test_hook_on_retry() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_retry");

    let toml_content = format!(
        r#"
[daemons.retry_hook_test]
run = "sh -c 'exit 1'"
retry = 2

[daemons.retry_hook_test.hooks]
on_retry = "sh -c 'echo retry >> {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    // Start with ready_delay=1 so wait_ready retry path kicks in
    let output = env.run_command(&["start", "retry_hook_test", "--delay", "1"]);
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("start stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Wait for retries to complete
    for _ in 0..30 {
        if let Ok(content) = std::fs::read_to_string(&marker) {
            let lines: Vec<&str> = content.lines().collect();
            if lines.len() >= 2 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    assert!(
        marker.exists(),
        "on_retry hook should have created marker file"
    );
    let content = std::fs::read_to_string(&marker).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "Should have 2 retry hook invocations (retry=2), got: {:?}",
        lines
    );
}

/// Test that PITCHFORK_DAEMON_ID env var is available to daemon processes
#[test]
fn test_env_var_pitchfork_daemon_id() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("daemon_id");

    let toml_content = format!(
        r#"
[daemons.id_env_test]
run = "sh -c 'echo $PITCHFORK_DAEMON_ID > {} && sleep 60'"
ready_delay = 1
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "id_env_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    env.sleep(Duration::from_millis(500));

    assert!(marker.exists(), "Marker file should exist");
    let content = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        content.trim(),
        "id_env_test",
        "PITCHFORK_DAEMON_ID should be the daemon id"
    );

    env.run_command(&["stop", "id_env_test"]);
}

/// Test that PITCHFORK_RETRY_COUNT env var is available and incremented
#[test]
fn test_env_var_pitchfork_retry_count() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("retry_count");

    // Daemon writes retry count then fails - retries will overwrite with new count
    let toml_content = format!(
        r#"
[daemons.retry_count_test]
run = "sh -c 'echo $PITCHFORK_RETRY_COUNT > {} && exit 1'"
retry = 1
ready_delay = 1
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "retry_count_test"]);
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Wait for retry to happen and write "1" to the file
    for _ in 0..30 {
        if let Ok(content) = std::fs::read_to_string(&marker)
            && content.trim() == "1"
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    let content = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        content.trim(),
        "1",
        "After retry, PITCHFORK_RETRY_COUNT should be 1"
    );
}

/// Test that hook commands receive correct env vars
#[test]
fn test_hook_env_vars() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("hook_env");

    let toml_content = format!(
        r#"
[daemons.hook_env_test]
run = "sh -c 'exit 7'"
retry = 0

[daemons.hook_env_test.hooks]
on_fail = "sh -c 'echo $PITCHFORK_DAEMON_ID $PITCHFORK_EXIT_CODE > {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "hook_env_test"]);
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Poll for marker file
    for _ in 0..30 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    assert!(
        marker.exists(),
        "on_fail hook should have created marker file"
    );
    let content = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        content.trim(),
        "hook_env_test 7",
        "Hook should receive PITCHFORK_DAEMON_ID and PITCHFORK_EXIT_CODE"
    );
}
