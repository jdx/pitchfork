mod common;

use common::TestEnv;
use std::time::Duration;

/// Test that autostop is delayed when PITCHFORK_AUTOSTOP_DELAY is set
#[test]
fn test_autostop_delay() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a daemon with autostop enabled
    let toml_content = r#"
[daemons.delayed_stop]
run = "sleep 120"
auto = ["stop"]
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Create an alternate directory to "leave" to
    let other_dir = env.create_other_dir();

    // Use the actual test process PID so it's recognized as running
    let shell_pid = std::process::id().to_string();

    // First, register the shell in the project directory
    // This starts the supervisor with PITCHFORK_AUTOSTOP_DELAY=5
    println!("=== test_autostop_delay ===");
    println!("Project dir: {:?}", env.project_dir());
    println!("Other dir: {:?}", other_dir);
    println!("Shell PID: {}", shell_pid);

    let cd1_output = env.run_command_with_env(
        &["cd", "--shell-pid", &shell_pid],
        &[("PITCHFORK_AUTOSTOP_DELAY", "5")],
    );
    println!(
        "Initial cd stderr: {}",
        String::from_utf8_lossy(&cd1_output.stderr)
    );

    // Check state file after initial cd
    let state_contents = std::fs::read_to_string(env.state_file_path()).unwrap_or_default();
    println!("State after initial cd:\n{}", state_contents);

    // Start the daemon with shell_pid association
    let output = env.run_command(&["start", "delayed_stop", "--shell-pid", &shell_pid]);
    println!("Start stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "Start command should succeed");

    // Verify daemon is running
    let status = env.get_daemon_status("delayed_stop");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should be running"
    );

    // Simulate "cd" to another directory (leaving the project dir)
    // This should schedule the autostop with delay
    let cd_output = env.run_command_in_dir(&["cd", "--shell-pid", &shell_pid], &other_dir);
    println!(
        "Leave cd stderr: {}",
        String::from_utf8_lossy(&cd_output.stderr)
    );

    // Immediately check - daemon should still be running (within delay period)
    env.sleep(Duration::from_secs(1));
    let status = env.get_daemon_status("delayed_stop");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should still be running within delay period"
    );

    // Wait for the delay to pass plus buffer for the 10s refresh interval
    // The interval timer only refreshes if elapsed() > 10s since last refresh
    // So we need to wait more than 10 seconds after the cd command
    env.sleep(Duration::from_secs(25));

    // Force a refresh by running list
    let _ = env.run_command(&["list"]);
    env.sleep(Duration::from_secs(1));

    // Now the daemon should be stopped
    let status = env.get_daemon_status("delayed_stop");
    assert!(
        status.as_deref() != Some("running"),
        "Daemon should be stopped after delay, got: {:?}",
        status
    );

    // Clean up
    let _ = env.run_command(&["stop", "delayed_stop"]);
}

/// Test that returning to the directory cancels the pending autostop
#[test]
fn test_autostop_cancel_on_return() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a daemon with autostop enabled
    let toml_content = r#"
[daemons.cancel_stop]
run = "sleep 120"
auto = ["stop"]
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Create an alternate directory
    let other_dir = env.create_other_dir();

    // Use the actual test process PID so it's recognized as running
    let shell_pid = std::process::id().to_string();

    // First, register the shell in the project directory
    // This starts the supervisor with PITCHFORK_AUTOSTOP_DELAY=10
    let _ = env.run_command_with_env(
        &["cd", "--shell-pid", &shell_pid],
        &[("PITCHFORK_AUTOSTOP_DELAY", "10")],
    );

    // Start the daemon
    let output = env.run_command(&["start", "cancel_stop", "--shell-pid", &shell_pid]);
    assert!(output.status.success(), "Start command should succeed");

    // Verify daemon is running
    let status = env.get_daemon_status("cancel_stop");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should be running"
    );

    // Simulate "cd" to another directory (leaving the project dir)
    let _ = env.run_command_in_dir(&["cd", "--shell-pid", &shell_pid], &other_dir);

    // Wait briefly
    env.sleep(Duration::from_secs(2));

    // Daemon should still be running
    let status = env.get_daemon_status("cancel_stop");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should still be running within delay"
    );

    // Return to the project directory - this should cancel the pending autostop
    let _ = env.run_command(&["cd", "--shell-pid", &shell_pid]);

    // Wait longer than the original delay plus refresh interval
    env.sleep(Duration::from_secs(20));

    // Force a refresh
    let _ = env.run_command(&["list"]);
    env.sleep(Duration::from_secs(1));

    // Daemon should STILL be running because we returned to the directory
    let status = env.get_daemon_status("cancel_stop");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should still be running after returning to directory"
    );

    // Clean up
    let _ = env.run_command(&["stop", "cancel_stop"]);
}

/// Test that autostop happens immediately when PITCHFORK_AUTOSTOP_DELAY=0
#[test]
fn test_autostop_immediate_with_zero_delay() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a daemon with autostop enabled
    let toml_content = r#"
[daemons.immediate_stop]
run = "sleep 120"
auto = ["stop"]
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Create an alternate directory
    let other_dir = env.create_other_dir();

    // Use the actual test process PID so it's recognized as running
    let shell_pid = std::process::id().to_string();

    // First, register the shell in the project directory
    // This starts the supervisor with PITCHFORK_AUTOSTOP_DELAY=0
    println!("Project dir: {:?}", env.project_dir());
    println!("Other dir: {:?}", other_dir);

    let cd_output = env.run_command_with_env(
        &["cd", "--shell-pid", &shell_pid],
        &[("PITCHFORK_AUTOSTOP_DELAY", "0")],
    );
    println!(
        "Initial cd output: {}",
        String::from_utf8_lossy(&cd_output.stderr)
    );

    // Check state file after initial cd
    let state_file = env.state_file_path();
    let state_after_cd = std::fs::read_to_string(&state_file).unwrap_or_default();
    println!("State file after initial cd:\n{}", state_after_cd);

    // Start the daemon
    let output = env.run_command(&["start", "immediate_stop", "--shell-pid", &shell_pid]);
    println!("Start stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("Start stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "Start command should succeed");

    // Verify daemon is running and check its status details
    let status_output = env.run_command(&["status", "immediate_stop"]);
    println!(
        "Status before cd: {}",
        String::from_utf8_lossy(&status_output.stdout)
    );

    // Check the state file to see daemon config
    let state_file = env.state_file_path();
    let state_contents = std::fs::read_to_string(&state_file).unwrap_or_default();
    println!("State file:\n{}", state_contents);

    let status = env.get_daemon_status("immediate_stop");
    assert_eq!(
        status.as_deref(),
        Some("running"),
        "Daemon should be running"
    );

    // Simulate "cd" to another directory - should stop immediately
    let cd_output2 = env.run_command_in_dir(&["cd", "--shell-pid", &shell_pid], &other_dir);
    println!(
        "Leave cd stdout: {}",
        String::from_utf8_lossy(&cd_output2.stdout)
    );
    println!(
        "Leave cd stderr: {}",
        String::from_utf8_lossy(&cd_output2.stderr)
    );

    // Wait briefly for the stop to process
    env.sleep(Duration::from_secs(2));

    // Force a refresh
    let list_output = env.run_command(&["list"]);
    println!(
        "List output: {}",
        String::from_utf8_lossy(&list_output.stdout)
    );
    env.sleep(Duration::from_secs(1));

    // Check status again
    let status_output2 = env.run_command(&["status", "immediate_stop"]);
    println!(
        "Status after cd: {}",
        String::from_utf8_lossy(&status_output2.stdout)
    );

    // Daemon should be stopped immediately
    let status = env.get_daemon_status("immediate_stop");
    assert!(
        status.as_deref() != Some("running"),
        "Daemon should be stopped immediately with PITCHFORK_AUTOSTOP_DELAY=0, got: {:?}",
        status
    );

    // Clean up
    let _ = env.run_command(&["stop", "immediate_stop"]);
}
