mod common;

use common::TestEnv;
use std::time::Duration;

/// Test that pre_start gate blocks daemon start until it succeeds
#[test]
fn test_gate_pre_start_passes() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("pre_start_pass");

    let toml_content = format!(
        r#"
[daemons.pre_start_pass_test]
run = "sleep 60"
ready_delay = 1

[daemons.pre_start_pass_test.gates]
pre_start = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "pre_start_pass_test"]);
    assert!(
        output.status.success(),
        "Start should succeed when pre_start gate passes: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "pre_start gate should have created marker file"
    );

    env.run_command(&["stop", "pre_start_pass_test"]);
}

/// Test that pre_start gate failure prevents daemon from starting
#[test]
fn test_gate_pre_start_fails_blocks_start() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.pre_start_fail_test]
run = "sleep 60"
ready_delay = 1

[daemons.pre_start_fail_test.gates]
pre_start = "exit 1"
"#
    .to_string();
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "pre_start_fail_test"]);
    assert!(
        !output.status.success(),
        "Start should fail when pre_start gate fails"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pre_start gate") || stderr.contains("exited with"),
        "Error should mention gate failure: {stderr}"
    );
}

/// Test that pre_start gate timeout kills the gate and fails the start
#[test]
fn test_gate_pre_start_timeout() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.pre_start_timeout_test]
run = "sleep 60"
ready_delay = 1

[daemons.pre_start_timeout_test.gates]
pre_start = { run = "sleep 300", timeout = "1s" }
"#
    .to_string();
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "pre_start_timeout_test"]);
    assert!(
        !output.status.success(),
        "Start should fail when pre_start gate times out"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("timed out"),
        "Error should mention timeout: {stderr}"
    );
}

/// Test that post_start gate runs after daemon becomes ready (wait_ready mode)
#[test]
fn test_gate_post_start_wait_ready() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("post_start_wait");

    let toml_content = format!(
        r#"
[daemons.post_start_wait_test]
run = "sleep 60"
ready_delay = 1

[daemons.post_start_wait_test.gates]
post_start = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "post_start_wait_test"]);
    assert!(
        output.status.success(),
        "Start should succeed when post_start gate passes: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "post_start gate should have created marker file"
    );

    env.run_command(&["stop", "post_start_wait_test"]);
}

/// Test that post_start gate failure returns error in wait_ready mode
#[test]
fn test_gate_post_start_fail_wait_ready() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.post_start_fail_test]
run = "sleep 60"
ready_delay = 1

[daemons.post_start_fail_test.gates]
post_start = "exit 1"
"#
    .to_string();
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "post_start_fail_test"]);
    assert!(
        !output.status.success(),
        "Start should fail when post_start gate fails in wait_ready mode"
    );
}

/// Test that pre_stop gate runs before daemon is stopped
#[test]
fn test_gate_pre_stop() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("pre_stop");

    let toml_content = format!(
        r#"
[daemons.pre_stop_test]
run = "sleep 60"
ready_delay = 1

[daemons.pre_stop_test.gates]
pre_stop = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "pre_stop_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Marker should NOT exist yet (pre_stop runs on stop, not start)
    assert!(!marker.exists(), "pre_stop gate should not have run yet");

    let output = env.run_command(&["stop", "pre_stop_test"]);
    assert!(
        output.status.success(),
        "Stop should succeed when pre_stop gate passes: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "pre_stop gate should have created marker file"
    );
}

/// Test that pre_stop gate failure prevents daemon from being stopped
#[test]
fn test_gate_pre_stop_fails_blocks_stop() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.pre_stop_fail_test]
run = "sleep 60"
ready_delay = 1

[daemons.pre_stop_fail_test.gates]
pre_stop = "exit 1"
"#
    .to_string();
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "pre_stop_fail_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = env.run_command(&["stop", "pre_stop_fail_test"]);
    assert!(
        !output.status.success(),
        "Stop should fail when pre_stop gate fails"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pre_stop gate") || stderr.contains("exited with"),
        "Error should mention gate failure: {stderr}"
    );
}

/// Test that post_stop gate runs after daemon has stopped
#[test]
fn test_gate_post_stop() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("post_stop");

    let toml_content = format!(
        r#"
[daemons.post_stop_test]
run = "sleep 60"
ready_delay = 1

[daemons.post_stop_test.gates]
post_stop = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "post_stop_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Marker should NOT exist yet (post_stop runs after stop)
    assert!(!marker.exists(), "post_stop gate should not have run yet");

    let output = env.run_command(&["stop", "post_stop_test"]);
    assert!(
        output.status.success(),
        "Stop should succeed when post_stop gate passes: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "post_stop gate should have created marker file"
    );
}

/// Test that post_stop gate receives PITCHFORK_EXIT_CODE and PITCHFORK_EXIT_REASON env vars
#[test]
fn test_gate_post_stop_env_vars() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("post_stop_env");

    let toml_content = format!(
        r#"
[daemons.post_stop_env_test]
run = "sleep 60"
ready_delay = 1

[daemons.post_stop_env_test.gates]
post_stop = "sh -c 'echo $PITCHFORK_EXIT_CODE $PITCHFORK_EXIT_REASON > {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "post_stop_env_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = env.run_command(&["stop", "post_stop_env_test"]);
    assert!(
        output.status.success(),
        "Stop should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Poll for marker content
    let mut content = String::new();
    for _ in 0..30 {
        content = std::fs::read_to_string(&marker).unwrap_or_default();
        if !content.trim().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    assert!(
        marker.exists(),
        "post_stop gate should have created marker file"
    );
    assert_eq!(
        content.trim(),
        "-1 stop",
        "post_stop gate should receive PITCHFORK_EXIT_CODE=-1 and PITCHFORK_EXIT_REASON=stop"
    );
}

/// Test that gate commands receive PITCHFORK_DAEMON_ID and PITCHFORK_RETRY_COUNT env vars
#[test]
fn test_gate_env_vars() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("gate_env");

    let toml_content = format!(
        r#"
[daemons.gate_env_test]
run = "sleep 60"
ready_delay = 1

[daemons.gate_env_test.gates]
pre_start = "sh -c 'echo $PITCHFORK_DAEMON_ID $PITCHFORK_RETRY_COUNT > {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "gate_env_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Poll for marker content
    let mut content = String::new();
    for _ in 0..30 {
        content = std::fs::read_to_string(&marker).unwrap_or_default();
        if !content.trim().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    assert!(
        marker.exists(),
        "pre_start gate should have created marker file"
    );
    assert_eq!(
        content.trim(),
        "project/gate_env_test 0",
        "Gate should receive PITCHFORK_DAEMON_ID and PITCHFORK_RETRY_COUNT=0"
    );

    env.run_command(&["stop", "gate_env_test"]);
}

/// Test that gate shorthand form (string) works
#[test]
fn test_gate_shorthand_form() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("gate_shorthand");

    let toml_content = format!(
        r#"
[daemons.gate_shorthand_test]
run = "sleep 60"
ready_delay = 1

[daemons.gate_shorthand_test.gates]
pre_start = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "gate_shorthand_test"]);
    assert!(
        output.status.success(),
        "Start should succeed with shorthand gate form: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "Shorthand gate should have created marker file"
    );

    env.run_command(&["stop", "gate_shorthand_test"]);
}

/// Test that gate full form (object with timeout) works
#[test]
fn test_gate_full_form_with_timeout() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("gate_full_form");

    let toml_content = format!(
        r#"
[daemons.gate_full_form_test]
run = "sleep 60"
ready_delay = 1

[daemons.gate_full_form_test.gates]
pre_start = {{ run = "touch {}", timeout = "30s" }}
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "gate_full_form_test"]);
    assert!(
        output.status.success(),
        "Start should succeed with full form gate: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "Full form gate should have created marker file"
    );

    env.run_command(&["stop", "gate_full_form_test"]);
}

/// Test that no gate configured means the lifecycle proceeds normally
#[test]
fn test_no_gate_configured() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.no_gate_test]
run = "sleep 60"
ready_delay = 1
"#
    .to_string();
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "no_gate_test"]);
    assert!(
        output.status.success(),
        "Start should succeed when no gate is configured: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    env.run_command(&["stop", "no_gate_test"]);
}

/// Test that post_stop gate runs even when process was already dead
#[test]
fn test_gate_post_stop_already_dead() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("post_stop_dead");

    let toml_content = format!(
        r#"
[daemons.post_stop_dead_test]
run = "sh -c 'sleep 1 && exit 0'"
ready_delay = 1

[daemons.post_stop_dead_test.gates]
post_stop = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "post_stop_dead_test"]);
    // Daemon exits quickly, start may or may not succeed depending on timing
    eprintln!("start stdout: {}", String::from_utf8_lossy(&output.stdout));
    eprintln!("start stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Wait for daemon to exit
    std::thread::sleep(Duration::from_secs(2));

    // Stop the already-dead daemon — post_stop gate should still run
    let output = env.run_command(&["stop", "post_stop_dead_test"]);
    eprintln!("stop stdout: {}", String::from_utf8_lossy(&output.stdout));
    eprintln!("stop stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Poll for marker file
    for _ in 0..30 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    assert!(
        marker.exists(),
        "post_stop gate should run even when process was already dead"
    );
}

/// Test that multiple gates work together (pre_start + post_start + pre_stop + post_stop)
#[test]
fn test_gate_all_four_lifecycle_points() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let pre_start_marker = env.marker_path("all_pre_start");
    let post_start_marker = env.marker_path("all_post_start");
    let pre_stop_marker = env.marker_path("all_pre_stop");
    let post_stop_marker = env.marker_path("all_post_stop");

    let toml_content = format!(
        r#"
[daemons.all_gates_test]
run = "sleep 60"
ready_delay = 1

[daemons.all_gates_test.gates]
pre_start = "touch {}"
post_start = "touch {}"
pre_stop = "touch {}"
post_stop = "touch {}"
"#,
        pre_start_marker.display(),
        post_start_marker.display(),
        pre_stop_marker.display(),
        post_stop_marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "all_gates_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(pre_start_marker.exists(), "pre_start gate should have run");
    assert!(
        post_start_marker.exists(),
        "post_start gate should have run"
    );
    assert!(
        !pre_stop_marker.exists(),
        "pre_stop gate should not have run yet"
    );
    assert!(
        !post_stop_marker.exists(),
        "post_stop gate should not have run yet"
    );

    let output = env.run_command(&["stop", "all_gates_test"]);
    assert!(
        output.status.success(),
        "Stop should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(pre_stop_marker.exists(), "pre_stop gate should have run");
    assert!(post_stop_marker.exists(), "post_stop gate should have run");
}
