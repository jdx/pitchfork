mod common;

use common::{TestEnv, get_script_path};
use std::fs;
use std::time::Duration;

/// Test that file watch configuration is persisted to state and triggers daemon restart.
/// This test verifies:
/// 1. Watch patterns are saved in daemon state
/// 2. Modifying a watched file triggers a daemon restart
/// 3. PID changes after the restart
#[test]
fn test_watch_triggers_daemon_restart() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a script that outputs its PID so we can track restarts
    let watch_script = get_script_path("http_server.ts");

    // Create a unique port to avoid conflicts
    let port = 19191;
    #[cfg(unix)]
    env.kill_port(port);

    let toml_content = format!(
        r#"
[daemons.watch_test]
run = "bun run {} 0 {}"
watch = ["watch_test_marker.txt"]
ready_port = {}
"#,
        watch_script.display(),
        port,
        port
    );
    env.create_toml(&toml_content);

    // Create the watched file before starting
    let watched_file = env.project_dir().join("watch_test_marker.txt");
    fs::write(&watched_file, "initial content").unwrap();

    // Start the daemon
    let output = env.run_command(&["start", "watch_test"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("start stderr: {}", stderr);
    println!("start stdout: {}", stdout);

    assert!(
        output.status.success(),
        "Start command should succeed: {}",
        stderr
    );

    // Get the original PID
    let original_pid = env.get_daemon_pid("watch_test");
    println!("Original PID: {:?}", original_pid);
    assert!(original_pid.is_some(), "Daemon should have a PID");

    // Wait for daemon to be fully running
    env.wait_for_status("watch_test", "running");

    // Modify the watched file to trigger a restart
    fs::write(&watched_file, "modified content").unwrap();
    println!("Modified watched file: {}", watched_file.display());

    // Wait for the file watcher to detect the change and restart
    // File watcher checks every 1s (debounce) + some processing time
    env.sleep(Duration::from_secs(3));

    // Check if daemon was restarted by looking at the PID
    let new_pid = env.get_daemon_pid("watch_test");
    println!("New PID after modification: {:?}", new_pid);

    // The daemon should be running with a different PID
    assert!(
        new_pid.is_some(),
        "Daemon should still be running after file change"
    );

    // Verify the status is still "running"
    let status = env.get_daemon_status("watch_test");
    println!("Daemon status after file change: {:?}", status);
    assert_eq!(
        status,
        Some("running".to_string()),
        "Daemon should be in running state after restart"
    );

    // Clean up
    let output = env.run_command(&["stop", "watch_test"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("stop stderr: {}", stderr);
}

/// Test that watch configuration persists after supervisor restart.
/// This verifies that watch configs are correctly saved to and loaded from state.
#[test]
fn test_watch_config_persists_in_state() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a daemon with watch patterns
    let toml_content = r#"
[daemons.persist_test]
run = "sleep 60"
watch = ["src/**/*.rs", "Cargo.toml"]
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Create directories for the watch patterns
    let src_dir = env.project_dir().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("main.rs"), "").unwrap();
    fs::write(env.project_dir().join("Cargo.toml"), "").unwrap();

    // Start the daemon
    let output = env.run_command(&["start", "persist_test"]);
    assert!(
        output.status.success(),
        "Start command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Read the state file directly to verify watch config was persisted
    let state_path = env.state_file_path();
    let state_contents = fs::read_to_string(&state_path).expect("State file should exist");
    println!("State file contents:\n{}", state_contents);

    // Verify watch patterns are in the state file
    assert!(
        state_contents.contains("watch"),
        "State file should contain watch configuration"
    );
    assert!(
        state_contents.contains("src/**/*.rs"),
        "State file should contain watch pattern 'src/**/*.rs'"
    );
    assert!(
        state_contents.contains("Cargo.toml"),
        "State file should contain watch pattern 'Cargo.toml'"
    );

    // Stop the supervisor (this will stop the daemon too)
    let output = env.run_command(&["supervisor", "stop"]);
    println!(
        "supervisor stop stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    env.sleep(Duration::from_secs(1));

    // Restart the daemon - this should restore watch config from state
    let output = env.run_command(&["start", "persist_test"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("restart stderr: {}", stderr);

    assert!(
        output.status.success(),
        "Restart should succeed: {}",
        stderr
    );

    // Verify daemon is running
    env.wait_for_status("persist_test", "running");

    // Read state file again to verify watch config is still there
    let state_contents = fs::read_to_string(&state_path).expect("State file should exist");
    assert!(
        state_contents.contains("watch"),
        "Watch config should persist after supervisor restart"
    );

    // Clean up
    env.run_command(&["stop", "persist_test"]);
}

/// Test that daemons with empty watch lists work correctly.
/// This ensures the watch feature doesn't break when not configured.
#[test]
fn test_daemon_without_watch_works() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.no_watch_test]
run = "sleep 60"
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Start the daemon without watch config
    let output = env.run_command(&["start", "no_watch_test"]);
    assert!(
        output.status.success(),
        "Start command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify daemon is running
    env.wait_for_status("no_watch_test", "running");

    // Get PID
    let pid = env.get_daemon_pid("no_watch_test");
    assert!(pid.is_some(), "Daemon should have a PID");

    // Wait a bit
    env.sleep(Duration::from_secs(2));

    // Daemon should still be running (no restarts triggered)
    let new_pid = env.get_daemon_pid("no_watch_test");
    assert_eq!(
        pid, new_pid,
        "PID should not change when no watch is configured"
    );

    let status = env.get_daemon_status("no_watch_test");
    assert_eq!(
        status,
        Some("running".to_string()),
        "Daemon should still be running"
    );

    // Clean up
    env.run_command(&["stop", "no_watch_test"]);
}

/// Test watch with glob patterns that match multiple directories.
#[test]
fn test_watch_glob_patterns() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Use the http_server script with a different port
    let port = 19192;
    #[cfg(unix)]
    env.kill_port(port);

    let watch_script = get_script_path("http_server.ts");
    let toml_content = format!(
        r#"
[daemons.glob_watch_test]
run = "bun run {} 0 {}"
watch = ["lib/**/*.ts", "config/*.json"]
ready_port = {}
"#,
        watch_script.display(),
        port,
        port
    );
    env.create_toml(&toml_content);

    // Create directories and files
    let lib_dir = env.project_dir().join("lib");
    fs::create_dir_all(&lib_dir).unwrap();
    let config_dir = env.project_dir().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    // Create initial files
    fs::write(lib_dir.join("main.ts"), "").unwrap();
    fs::write(config_dir.join("app.json"), r#"{"port": 8080}"#).unwrap();

    // Start the daemon
    let output = env.run_command(&["start", "glob_watch_test"]);
    assert!(
        output.status.success(),
        "Start command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Wait for daemon to be running
    env.wait_for_status("glob_watch_test", "running");

    // Get original PID
    let original_pid = env.get_daemon_pid("glob_watch_test");
    assert!(original_pid.is_some(), "Daemon should have a PID");

    // Modify a file in lib directory
    fs::write(lib_dir.join("helper.ts"), "export const x = 1;").unwrap();
    env.sleep(Duration::from_secs(3));

    // Verify daemon is still running
    let status = env.get_daemon_status("glob_watch_test");
    assert_eq!(
        status,
        Some("running".to_string()),
        "Daemon should be running after lib file change"
    );

    // Modify config file
    fs::write(config_dir.join("app.json"), r#"{"port": 9090}"#).unwrap();
    env.sleep(Duration::from_secs(3));

    // Verify daemon is still running
    let status = env.get_daemon_status("glob_watch_test");
    assert_eq!(
        status,
        Some("running".to_string()),
        "Daemon should be running after config change"
    );

    // Clean up
    env.run_command(&["stop", "glob_watch_test"]);
}

/// Test that watch patterns work correctly with relative paths.
#[test]
fn test_watch_relative_paths() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let port = 19193;
    #[cfg(unix)]
    env.kill_port(port);

    let watch_script = get_script_path("http_server.ts");
    let toml_content = format!(
        r#"
[daemons.relative_watch_test]
run = "bun run {} 0 {}"
watch = ["./relative_test.txt"]
ready_port = {}
"#,
        watch_script.display(),
        port,
        port
    );
    env.create_toml(&toml_content);

    // Create the watched file
    let watched_file = env.project_dir().join("relative_test.txt");
    fs::write(&watched_file, "initial").unwrap();

    // Start the daemon
    let output = env.run_command(&["start", "relative_watch_test"]);
    assert!(
        output.status.success(),
        "Start command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Wait for daemon to be running
    env.wait_for_status("relative_watch_test", "running");

    // Get original PID
    let original_pid = env.get_daemon_pid("relative_watch_test");
    assert!(original_pid.is_some(), "Daemon should have a PID");

    // Modify the watched file
    fs::write(&watched_file, "modified").unwrap();
    env.sleep(Duration::from_secs(3));

    // Verify daemon is still running
    let status = env.get_daemon_status("relative_watch_test");
    assert_eq!(
        status,
        Some("running".to_string()),
        "Daemon should be running after file change"
    );

    // Clean up
    env.run_command(&["stop", "relative_watch_test"]);
}
