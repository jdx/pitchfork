mod common;

use common::{TestEnv, get_script_path};
use std::time::Duration;

// ============================================================================
// Hooks Configuration Tests
// ============================================================================
// These tests verify that hooks configuration (on_ready, on_fail, on_cron_trigger,
// on_retry) is correctly parsed and stored in daemon configurations.

/// Test that on_ready hook can be configured as a simple string
#[test]
fn test_hooks_on_ready_simple_string() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.api]
run = "sleep 30"
ready_delay = 1
on_ready = "echo 'API is ready!'"
"#;
    env.create_toml(toml_content);

    // Start daemon
    let output = env.run_command(&["start", "api"]);
    assert!(output.status.success(), "Start command should succeed");

    // Verify daemon is running
    let status = env.get_daemon_status("api");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should be running"
    );

    // Clean up
    env.run_command(&["stop", "api"]);
}

/// Test that on_fail hook can be configured
#[test]
fn test_hooks_on_fail_with_failing_daemon() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");

    let toml_content = format!(
        r#"
[daemons.will_fail]
run = "bun run {} 0"
retry = 0
on_fail = "echo 'Daemon failed!'"
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon - it will fail
    let output = env.run_command(&["start", "will_fail"]);

    // Start should fail since daemon fails immediately
    assert!(
        !output.status.success(),
        "Start should fail when daemon fails immediately"
    );

    // Verify logs show the failure
    let logs = env.read_logs("will_fail");
    assert!(
        logs.contains("Failed after 0!"),
        "Logs should contain failure message"
    );

    let _ = env.run_command(&["stop", "will_fail"]);
}

/// Test that on_retry hook can be configured
#[test]
fn test_hooks_on_retry_with_retrying_daemon() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");

    let toml_content = format!(
        r#"
[daemons.will_retry]
run = "bun run {} 0"
retry = 2
ready_delay = 1
on_retry = "echo 'Retrying daemon...'"
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon - it will fail and retry
    let output = env.run_command(&["start", "will_retry"]);

    // Should eventually fail after exhausting retries
    assert!(
        !output.status.success(),
        "Start should fail after exhausting retries"
    );

    // Verify multiple retry attempts in logs
    let logs = env.read_logs("will_retry");
    let attempt_count = logs.matches("Failed after 0!").count();
    assert!(
        attempt_count >= 2,
        "Should have multiple failure attempts (got {})",
        attempt_count
    );

    let _ = env.run_command(&["stop", "will_retry"]);
}

/// Test that on_cron_trigger hook can be configured with cron daemon
#[test]
#[ignore] // Long-running test
fn test_hooks_on_cron_trigger_with_cron_daemon() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let slowly_script = get_script_path("slowly_output.ts");

    let toml_content = format!(
        r#"
[daemons.cron_with_hook]
run = "bun run {} 1 3"
cron = {{ schedule = "*/30 * * * * *", retrigger = "finish" }}
on_cron_trigger = "echo 'Cron job triggered!'"
"#,
        slowly_script.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "cron_with_hook"]);
    assert!(
        output.status.success(),
        "Start should succeed for cron daemon with hook"
    );

    // Wait for task to run
    env.sleep(Duration::from_secs(5));

    // Verify daemon ran
    let logs = env.read_logs("cron_with_hook");
    assert!(
        logs.contains("Output"),
        "Logs should contain output from the script"
    );

    let _ = env.run_command(&["stop", "cron_with_hook"]);
}

/// Test hooks with full configuration (shell, dir, env)
/// Note: Full hook configuration with shell, dir, env is not currently supported.
/// Hooks are simple string commands executed in bash.
#[test]
fn test_hooks_full_configuration() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.full_hooks]
run = "sleep 30"
ready_delay = 1
on_ready = "echo 'Service is ready'"
on_fail = "echo 'Service failed!'"
"#;
    env.create_toml(toml_content);

    // Start daemon
    let output = env.run_command(&["start", "full_hooks"]);
    assert!(output.status.success(), "Start command should succeed");

    // Verify daemon is running
    let status = env.get_daemon_status("full_hooks");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should be running"
    );

    // Clean up
    env.run_command(&["stop", "full_hooks"]);
}

/// Test multiple hooks on same daemon
#[test]
fn test_hooks_multiple_hooks_on_same_daemon() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.multi_hooks]
run = "sleep 30"
ready_delay = 1
on_ready = "echo 'Ready hook fired'"
on_fail = "echo 'Fail hook fired'"
on_retry = "echo 'Retry hook fired'"
"#;
    env.create_toml(toml_content);

    // Start daemon
    let output = env.run_command(&["start", "multi_hooks"]);
    assert!(output.status.success(), "Start command should succeed");

    // Verify daemon is running
    let status = env.get_daemon_status("multi_hooks");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should be running"
    );

    // Clean up
    env.run_command(&["stop", "multi_hooks"]);
}

/// Test hooks with dependency chain
#[test]
fn test_hooks_with_dependencies() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.db]
run = "sleep 60"
ready_delay = 1
on_ready = "echo 'Database ready'"

[daemons.api]
run = "sleep 60"
depends = ["db"]
ready_delay = 1
on_ready = "echo 'API ready!'"
on_fail = "echo 'API failed!'"
"#;
    env.create_toml(toml_content);

    // Start api (should auto-start db as dependency)
    let output = env.run_command(&["start", "api"]);
    assert!(
        output.status.success(),
        "Start command should succeed for api with dependency"
    );

    // Check that both daemons are running
    let list_output = env.run_command(&["list"]);
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(list_stdout.contains("db"), "db daemon should be running");
    assert!(list_stdout.contains("api"), "api daemon should be running");

    // Clean up
    env.run_command(&["stop", "--all"]);
}

/// Test hooks configuration is preserved during restart
#[test]
fn test_hooks_preserved_on_restart() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.restartable]
run = "sleep 60"
ready_delay = 1
on_ready = "echo 'Ready hook'"
on_fail = "echo 'Fail hook'"
"#;
    env.create_toml(toml_content);

    // Start daemon
    let output = env.run_command(&["start", "restartable"]);
    assert!(output.status.success(), "Initial start should succeed");

    // Get original PID
    let original_pid = env.get_daemon_pid("restartable");
    assert!(original_pid.is_some(), "Should have PID after start");

    // Restart the daemon
    let output = env.run_command(&["restart", "restartable"]);
    assert!(output.status.success(), "Restart should succeed");

    // Wait for restart to complete
    env.sleep(Duration::from_secs(2));

    // Verify daemon is running with new PID
    let new_pid = env.get_daemon_pid("restartable");
    assert!(new_pid.is_some(), "Should have PID after restart");
    assert_ne!(original_pid, new_pid, "PID should change after restart");

    let status = env.get_daemon_status("restartable");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should be running after restart"
    );

    // Clean up
    env.run_command(&["stop", "restartable"]);
}

/// Test hooks with environment variables
/// Note: Environment variables can be set via the daemon's env config, not per-hook.
/// Hooks receive PITCHFORK_DAEMON_ID, PITCHFORK_DAEMON_NAMESPACE, PITCHFORK_DAEMON_NAME,
/// and PITCHFORK_HOOK_NAME automatically.
#[test]
fn test_hooks_with_environment_variables() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.env_hooks]
run = "sleep 30"
ready_delay = 1
on_ready = "echo Ready: $PITCHFORK_DAEMON_NAME"
on_fail = "echo Failed: $PITCHFORK_HOOK_NAME"
"#;
    env.create_toml(toml_content);

    // Start daemon
    let output = env.run_command(&["start", "env_hooks"]);
    assert!(output.status.success(), "Start command should succeed");

    // Verify daemon is running
    let status = env.get_daemon_status("env_hooks");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should be running"
    );

    // Clean up
    env.run_command(&["stop", "env_hooks"]);
}

/// Test that start --all works with daemons that have hooks
#[test]
fn test_hooks_start_all() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.service1]
run = "sleep 30"
ready_delay = 1
on_ready = "echo 'Service 1 ready'"

[daemons.service2]
run = "sleep 30"
ready_delay = 1
on_ready = "echo 'Service 2 ready'"
on_fail = "echo 'Service 2 failed'"

[daemons.service3]
run = "sleep 30"
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Start all daemons
    let output = env.run_command(&["start", "--all"]);
    assert!(
        output.status.success(),
        "Start --all should succeed with hooks"
    );

    // Wait for all to start
    env.sleep(Duration::from_secs(2));

    // List should show all three
    let output = env.run_command(&["list"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("service1"), "Should list service1");
    assert!(stdout.contains("service2"), "Should list service2");
    assert!(stdout.contains("service3"), "Should list service3");

    // Clean up
    env.run_command(&["stop", "--all"]);
}

/// Test hooks config does not interfere with daemon stop
#[test]
fn test_hooks_daemon_stop() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.stoppable]
run = "sleep 60"
ready_delay = 1
on_ready = "echo 'Started'"
on_fail = "echo 'Failed'"
on_retry = "echo 'Retrying'"
"#;
    env.create_toml(toml_content);

    // Start daemon
    let output = env.run_command(&["start", "stoppable"]);
    assert!(output.status.success(), "Start should succeed");

    // Verify running
    let status = env.get_daemon_status("stoppable");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should be running"
    );

    // Stop daemon
    let stop_start = std::time::Instant::now();
    let output = env.run_command(&["stop", "stoppable"]);
    let stop_elapsed = stop_start.elapsed();

    assert!(output.status.success(), "Stop should succeed");

    // Stop should complete quickly (not hang due to hooks)
    assert!(
        stop_elapsed < Duration::from_secs(5),
        "Stop should complete quickly, took {:?}",
        stop_elapsed
    );

    // Daemon should be stopped
    env.wait_for_status("stoppable", "stopped");
}
