mod common;

use common::TestEnv;
use std::net::TcpListener;
use std::time::Duration;

/// Test that pitchfork detects port conflicts and fails when auto_bump_port is disabled
#[test]
fn test_port_conflict_detection() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Bind to a specific port to create a conflict
    let port: u16 = 45678;
    let _listener = TcpListener::bind(("127.0.0.1", port)).expect("Failed to bind to test port");

    // Create a daemon that expects to use the same port
    let toml_content = format!(
        r#"
[daemons.port_conflict]
run = "python3 -m http.server {}"
port = {}
"#,
        port, port
    );
    env.create_toml(&toml_content);

    // Try to start the daemon - should fail due to port conflict
    let output = env.run_command(&["start", "port_conflict"]);

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    // The start command should fail
    assert!(
        !output.status.success(),
        "Start command should fail when port is in use"
    );

    // Error message should mention port conflict
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already in use") || stderr.contains("port") || stderr.contains("Port"),
        "Error message should indicate port conflict: {}",
        stderr
    );

    // Cleanup
    let _ = env.run_command(&["stop", "port_conflict"]);
}

/// Test that pitchfork auto-bumps to an available port when auto_bump_port is enabled
#[test]
fn test_port_auto_bump() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Bind to a specific port to create a conflict
    let port: u16 = 45679;
    let _listener = TcpListener::bind(("127.0.0.1", port)).expect("Failed to bind to test port");

    // Create the project directory first
    env.create_project_dir();

    // Create a script that uses the PORT environment variable
    let script_content = format!(
        r#"#!/bin/bash
python3 -c "
import http.server
import socketserver
import os
port = int(os.environ.get('PORT', {}))
with socketserver.TCPServer(('', port), http.server.SimpleHTTPRequestHandler) as httpd:
    print(f'Server running on port {port}')
    httpd.handle_request()
" &
sleep 1
echo "ready"
sleep 30
"#,
        port + 1
    );
    let script_path = env.project_dir().join("test_auto_bump.sh");
    std::fs::write(&script_path, script_content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
    }

    // Create a daemon that expects to use the same port but with auto_bump_port enabled
    let toml_content = format!(
        r#"
[daemons.port_bump]
run = "bash {}"
port = {}
auto_bump_port = true
ready_output = "ready"
"#,
        script_path.display(),
        port
    );
    env.create_toml(&toml_content);

    // Try to start the daemon - should succeed with auto-bump
    let output = env.run_command(&["start", "port_bump"]);

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    // The start command should succeed
    assert!(
        output.status.success(),
        "Start command should succeed with auto_bump_port enabled"
    );

    // Give the daemon a moment to start
    env.sleep(Duration::from_millis(500));

    // Check that the daemon is running
    let status_output = env.run_command(&["status", "port_bump"]);
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    println!("Status output: {}", status_stdout);

    assert!(
        status_stdout.contains("running") || status_stdout.contains("ready"),
        "Daemon should be running after port auto-bump"
    );

    // Cleanup
    let _ = env.run_command(&["stop", "port_bump"]);
}

/// Test that PORT environment variable is injected correctly
#[test]
fn test_port_env_injection() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let port: u16 = 45680;
    let marker_path = env.marker_path("port_test");

    // Create the project directory first
    env.create_project_dir();

    // Create a script that outputs the PORT environment variable
    let script_content = format!(
        r#"#!/bin/bash
echo "PORT=$PORT" > "{}"
# Keep the script running so pitchfork detects it as running
sleep 30
"#,
        marker_path.display()
    );
    let script_path = env.project_dir().join("test_port.sh");
    std::fs::write(&script_path, script_content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
    }

    // Create a daemon with port configured
    let toml_content = format!(
        r#"
[daemons.port_env]
run = "bash {}"
port = {}
"#,
        script_path.display(),
        port
    );
    env.create_toml(&toml_content);

    // Start the daemon
    let output = env.run_command(&["start", "port_env"]);

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success(), "Start command should succeed");

    // Give the daemon time to write the marker file
    env.sleep(Duration::from_millis(500));

    // Check that the PORT environment variable was set correctly
    let marker_content = std::fs::read_to_string(&marker_path).unwrap_or_default();
    println!("Marker content: {}", marker_content);

    assert!(
        marker_content.contains(&format!("PORT={}", port)),
        "PORT environment variable should be set to {}: got {}",
        port,
        marker_content
    );

    // Cleanup
    let _ = env.run_command(&["stop", "port_env"]);
}

/// Test CLI --expected-port and --auto-bump-port flags
#[test]
fn test_cli_port_flags() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let port: u16 = 45681;

    // Bind to the port to create a conflict
    let _listener = TcpListener::bind(("127.0.0.1", port)).expect("Failed to bind to test port");

    // Create a simple daemon
    let toml_content = r#"
[daemons.cli_port_test]
run = "python3 -m http.server 0"
"#;
    env.create_toml(toml_content);

    // Try to start with expected-port flag (should fail due to conflict)
    let output = env.run_command(&[
        "start",
        "cli_port_test",
        "--expected-port",
        &port.to_string(),
    ]);

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(
        !output.status.success(),
        "Start command should fail when --expected-port conflicts"
    );

    // Now try with --auto-bump-port flag (should succeed)
    let output = env.run_command(&[
        "start",
        "cli_port_test",
        "--expected-port",
        &port.to_string(),
        "--auto-bump-port",
    ]);

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Should succeed with auto-bump
    assert!(
        output.status.success(),
        "Start command should succeed with --auto-bump-port"
    );

    // Cleanup
    let _ = env.run_command(&["stop", "cli_port_test"]);
}

/// Test ready_port synchronization with resolved port
#[test]
fn test_ready_port_sync() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let port: u16 = 45682;

    // Create a daemon without explicit ready_port
    // The resolved port should be used as ready_port
    let toml_content = format!(
        r#"
[daemons.ready_sync]
run = "python3 -m http.server {}"
port = {}
"#,
        port, port
    );
    env.create_toml(&toml_content);

    // Start the daemon
    let output = env.run_command(&["start", "ready_sync"]);

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Should succeed
    assert!(output.status.success(), "Start command should succeed");

    // Give time for the daemon to become ready
    env.sleep(Duration::from_millis(1000));

    // Check status
    let status_output = env.run_command(&["status", "ready_sync"]);
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    println!("Status: {}", status_stdout);

    // The daemon should be ready (which means ready_port check passed)
    assert!(
        status_stdout.contains("ready") || status_stdout.contains("running"),
        "Daemon should be ready/running: {}",
        status_stdout
    );

    // Cleanup
    let _ = env.run_command(&["stop", "ready_sync"]);
}
