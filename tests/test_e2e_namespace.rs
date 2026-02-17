mod common;

use common::TestEnv;
use std::fs;
use std::time::Duration;

// ============================================================================
// Namespace Qualified ID Tests
// ============================================================================

/// Test that daemons from different directories are properly namespaced
/// and their logs are stored in separate paths.
#[test]
fn test_namespace_log_separation() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create project directory with a daemon named "api"
    let project_a = env.project_dir().join("project-a");
    fs::create_dir_all(&project_a).unwrap();
    fs::write(
        project_a.join("pitchfork.toml"),
        r#"
[daemons.api]
run = "echo 'Hello from project-a' && sleep 10"
"#,
    )
    .unwrap();

    // Create another project directory with a daemon also named "api"
    let project_b = env.project_dir().join("project-b");
    fs::create_dir_all(&project_b).unwrap();
    fs::write(
        project_b.join("pitchfork.toml"),
        r#"
[daemons.api]
run = "echo 'Hello from project-b' && sleep 10"
"#,
    )
    .unwrap();

    // Start daemon from project-a
    let output_a = env.run_command_in_dir(&["start", "api"], &project_a);
    println!(
        "project-a start stdout: {}",
        String::from_utf8_lossy(&output_a.stdout)
    );
    println!(
        "project-a start stderr: {}",
        String::from_utf8_lossy(&output_a.stderr)
    );
    assert!(
        output_a.status.success(),
        "Failed to start api in project-a"
    );

    env.sleep(Duration::from_secs(1));

    // Start daemon from project-b
    let output_b = env.run_command_in_dir(&["start", "api"], &project_b);
    println!(
        "project-b start stdout: {}",
        String::from_utf8_lossy(&output_b.stdout)
    );
    println!(
        "project-b start stderr: {}",
        String::from_utf8_lossy(&output_b.stderr)
    );
    assert!(
        output_b.status.success(),
        "Failed to start api in project-b"
    );

    env.sleep(Duration::from_secs(1));

    // Check that logs are stored in separate directories
    let logs_dir = env
        .home_dir()
        .join(".local")
        .join("state")
        .join("pitchfork")
        .join("logs");

    // project-a's api should be in project-a--api/
    let log_a = logs_dir.join("project-a--api").join("project-a--api.log");
    assert!(
        log_a.exists(),
        "Log file for project-a/api should exist at {:?}",
        log_a
    );
    let log_a_content = fs::read_to_string(&log_a).unwrap();
    assert!(
        log_a_content.contains("Hello from project-a"),
        "project-a log should contain its message, got: {log_a_content}"
    );

    // project-b's api should be in project-b--api/
    let log_b = logs_dir.join("project-b--api").join("project-b--api.log");
    assert!(
        log_b.exists(),
        "Log file for project-b/api should exist at {:?}",
        log_b
    );
    let log_b_content = fs::read_to_string(&log_b).unwrap();
    assert!(
        log_b_content.contains("Hello from project-b"),
        "project-b log should contain its message, got: {log_b_content}"
    );

    // Cleanup
    let _ = env.run_command_in_dir(&["stop", "api"], &project_a);
    let _ = env.run_command_in_dir(&["stop", "api"], &project_b);
}

/// Test that qualified IDs work correctly with start/stop/status commands
#[test]
fn test_qualified_id_operations() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a project directory
    let project = env.project_dir().join("myproject");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("pitchfork.toml"),
        r#"
[daemons.server]
run = "sleep 60"
"#,
    )
    .unwrap();

    // Start daemon from project directory
    let output = env.run_command_in_dir(&["start", "server"], &project);
    assert!(output.status.success(), "Failed to start server");

    env.sleep(Duration::from_secs(1));

    // Check status using qualified ID from a different directory
    let other_dir = env.create_other_dir();
    let status_output = env.run_command_in_dir(&["status", "myproject/server"], &other_dir);
    println!(
        "status stdout: {}",
        String::from_utf8_lossy(&status_output.stdout)
    );
    assert!(
        status_output.status.success(),
        "Status command with qualified ID should succeed"
    );
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    assert!(
        status_str.contains("running"),
        "Daemon should be running, got: {status_str}"
    );

    // Stop using qualified ID from a different directory
    let stop_output = env.run_command_in_dir(&["stop", "myproject/server"], &other_dir);
    assert!(
        stop_output.status.success(),
        "Stop command with qualified ID should succeed"
    );

    env.sleep(Duration::from_secs(1));

    // Verify it's stopped
    let status_output2 = env.run_command_in_dir(&["status", "myproject/server"], &other_dir);
    let status_str2 = String::from_utf8_lossy(&status_output2.stdout);
    assert!(
        status_str2.contains("stopped") || status_str2.contains("exited"),
        "Daemon should be stopped, got: {status_str2}"
    );
}

/// Test that short IDs work when in the correct directory
#[test]
fn test_short_id_in_correct_directory() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("shorttest");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("pitchfork.toml"),
        r#"
[daemons.myservice]
run = "sleep 30"
"#,
    )
    .unwrap();

    // Start with short ID
    let start_output = env.run_command_in_dir(&["start", "myservice"], &project);
    assert!(
        start_output.status.success(),
        "Start with short ID should work"
    );

    env.sleep(Duration::from_secs(1));

    // Status with short ID (in same directory)
    let status_output = env.run_command_in_dir(&["status", "myservice"], &project);
    assert!(
        status_output.status.success(),
        "Status with short ID should work"
    );
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    assert!(status_str.contains("running"), "Should be running");

    // Stop with short ID
    let stop_output = env.run_command_in_dir(&["stop", "myservice"], &project);
    assert!(
        stop_output.status.success(),
        "Stop with short ID should work"
    );
}

/// Test that list command shows proper namespaces when there are conflicts
#[test]
fn test_list_shows_namespace_on_conflict() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create two projects with same daemon name
    let project_x = env.project_dir().join("proj-x");
    fs::create_dir_all(&project_x).unwrap();
    fs::write(
        project_x.join("pitchfork.toml"),
        r#"
[daemons.web]
run = "sleep 60"
"#,
    )
    .unwrap();

    let project_y = env.project_dir().join("proj-y");
    fs::create_dir_all(&project_y).unwrap();
    fs::write(
        project_y.join("pitchfork.toml"),
        r#"
[daemons.web]
run = "sleep 60"
"#,
    )
    .unwrap();

    // Start both
    let _ = env.run_command_in_dir(&["start", "web"], &project_x);
    env.sleep(Duration::from_secs(1));
    let _ = env.run_command_in_dir(&["start", "web"], &project_y);
    env.sleep(Duration::from_secs(1));

    // List all daemons
    let list_output = env.run_command_in_dir(&["list"], &project_x);
    let list_str = String::from_utf8_lossy(&list_output.stdout);
    println!("list output: {list_str}");

    // Both should show qualified names since there's a conflict
    assert!(
        list_str.contains("proj-x/web") || list_str.contains("proj-x--web"),
        "List should show proj-x namespace for web daemon"
    );
    assert!(
        list_str.contains("proj-y/web") || list_str.contains("proj-y--web"),
        "List should show proj-y namespace for web daemon"
    );

    // Cleanup
    let _ = env.run_command_in_dir(&["stop", "web"], &project_x);
    let _ = env.run_command_in_dir(&["stop", "web"], &project_y);
}

/// Test that list command hides namespace when there's no conflict
#[test]
fn test_list_hides_namespace_without_conflict() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a single project with unique daemon names
    let project = env.project_dir().join("solo-project");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("pitchfork.toml"),
        r#"
[daemons.unique-api]
run = "sleep 60"

[daemons.unique-worker]
run = "sleep 60"
"#,
    )
    .unwrap();

    // Start daemons
    let _ = env.run_command_in_dir(&["start", "unique-api"], &project);
    env.sleep(Duration::from_secs(1));
    let _ = env.run_command_in_dir(&["start", "unique-worker"], &project);
    env.sleep(Duration::from_secs(1));

    // List all daemons
    let list_output = env.run_command_in_dir(&["list"], &project);
    let list_str = String::from_utf8_lossy(&list_output.stdout);
    println!("list output (no conflict): {list_str}");

    // Should show short names (no namespace) since there's no conflict
    assert!(
        list_str.contains("unique-api"),
        "List should contain unique-api"
    );
    assert!(
        list_str.contains("unique-worker"),
        "List should contain unique-worker"
    );
    // Should NOT contain the namespace prefix (no conflict)
    assert!(
        !list_str.contains("solo-project/"),
        "List should NOT show namespace when there's no conflict, got: {list_str}"
    );

    // Cleanup
    let _ = env.run_command_in_dir(&["stop", "unique-api"], &project);
    let _ = env.run_command_in_dir(&["stop", "unique-worker"], &project);
}

/// Test logs command works with qualified IDs
#[test]
fn test_logs_with_qualified_id() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("logtest");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("pitchfork.toml"),
        r#"
[daemons.logger]
run = "echo 'test log message' && sleep 30"
"#,
    )
    .unwrap();

    // Start daemon
    let _ = env.run_command_in_dir(&["start", "logger"], &project);
    env.sleep(Duration::from_secs(2));

    // Get logs using qualified ID from different directory
    let other_dir = env.create_other_dir();
    let logs_output = env.run_command_in_dir(&["logs", "logtest/logger", "-n", "10"], &other_dir);
    let logs_str = String::from_utf8_lossy(&logs_output.stdout);
    println!("logs output: {logs_str}");

    assert!(
        logs_str.contains("test log message"),
        "Logs should contain the test message, got: {logs_str}"
    );

    // Cleanup
    let _ = env.run_command_in_dir(&["stop", "logger"], &project);
}

/// Test that path encoding/decoding is consistent
#[test]
fn test_path_encoding_roundtrip() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Use a project name with dashes (but not double dash)
    let project = env.project_dir().join("my-cool-project");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("pitchfork.toml"),
        r#"
[daemons.my-service]
run = "echo 'encoding test' && sleep 30"
"#,
    )
    .unwrap();

    // Start daemon
    let output = env.run_command_in_dir(&["start", "my-service"], &project);
    assert!(output.status.success(), "Start should succeed");

    env.sleep(Duration::from_secs(2));

    // Verify log path uses correct encoding
    let logs_dir = env
        .home_dir()
        .join(".local")
        .join("state")
        .join("pitchfork")
        .join("logs");

    // my-cool-project/my-service -> my-cool-project--my-service
    let expected_dir = logs_dir.join("my-cool-project--my-service");
    let expected_log = expected_dir.join("my-cool-project--my-service.log");

    assert!(
        expected_dir.exists(),
        "Log directory should exist at {:?}",
        expected_dir
    );
    assert!(
        expected_log.exists(),
        "Log file should exist at {:?}",
        expected_log
    );

    let log_content = fs::read_to_string(&expected_log).unwrap();
    assert!(
        log_content.contains("encoding test"),
        "Log should contain test message"
    );

    // Cleanup
    let _ = env.run_command_in_dir(&["stop", "my-service"], &project);
}

// ============================================================================
// Helper trait extension for TestEnv
// ============================================================================

trait TestEnvExt {
    fn home_dir(&self) -> std::path::PathBuf;
}

impl TestEnvExt for TestEnv {
    fn home_dir(&self) -> std::path::PathBuf {
        // Access the home_dir through the public interface
        // We'll use the state file path to derive it
        self.state_file_path()
            .parent() // pitchfork/
            .unwrap()
            .parent() // state/
            .unwrap()
            .parent() // .local/
            .unwrap()
            .parent() // home/
            .unwrap()
            .to_path_buf()
    }
}
