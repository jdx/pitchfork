mod common;

use common::TestEnv;
use std::time::Duration;

/// Test that pre_start hook blocks daemon start until it succeeds
#[test]
fn test_hook_pre_start_passes() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("pre_start_pass");

    let toml_content = format!(
        r#"
[daemons.pre_start_pass_test]
run = "sleep 60"
ready_delay = 1

[daemons.pre_start_pass_test.hooks]
pre_start = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "pre_start_pass_test"]);
    assert!(
        output.status.success(),
        "Start should succeed when pre_start hook passes: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "pre_start hook should have created marker file"
    );

    env.run_command(&["stop", "pre_start_pass_test"]);
}

/// Test that pre_start hook failure prevents daemon from starting
#[test]
fn test_hook_pre_start_fails_blocks_start() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.pre_start_fail_test]
run = "sleep 60"
ready_delay = 1

[daemons.pre_start_fail_test.hooks]
pre_start = { run = "exit 1", block = true }
"#
    .to_string();
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "pre_start_fail_test"]);
    assert!(
        !output.status.success(),
        "Start should fail when pre_start hook fails"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pre_start hook") || stderr.contains("exited with"),
        "Error should mention hook failure: {stderr}"
    );
}

/// Test that pre_start hook timeout kills the hook and fails the start
#[test]
fn test_hook_pre_start_timeout() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.pre_start_timeout_test]
run = "sleep 60"
ready_delay = 1

[daemons.pre_start_timeout_test.hooks]
pre_start = { run = "sleep 300", block = true, timeout = "1s" }
"#
    .to_string();
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "pre_start_timeout_test"]);
    assert!(
        !output.status.success(),
        "Start should fail when pre_start hook times out"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("timed out"),
        "Error should mention timeout: {stderr}"
    );
}

/// Test that on_ready hook runs after daemon becomes ready (wait_ready mode)
#[test]
fn test_hook_on_ready_wait_ready() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_ready_wait");

    let toml_content = format!(
        r#"
[daemons.on_ready_wait_test]
run = "sleep 60"
ready_delay = 1

[daemons.on_ready_wait_test.hooks]
on_ready = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "on_ready_wait_test"]);
    assert!(
        output.status.success(),
        "Start should succeed when on_ready hook passes: {}",
        String::from_utf8_lossy(&output.stderr)
    );

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

    env.run_command(&["stop", "on_ready_wait_test"]);
}

/// Test that on_ready hook with block=true failure returns error in wait_ready mode
#[test]
fn test_hook_on_ready_fail_wait_ready() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.on_ready_fail_test]
run = "sleep 60"
ready_delay = 1

[daemons.on_ready_fail_test.hooks]
on_ready = { run = "exit 1", block = true }
"#
    .to_string();
    env.create_toml(&toml_content);

    // on_ready hook with block=true failure means the daemon is not ready,
    // so start should fail (returns DaemonFailedWithCode).
    let output = env.run_command(&["start", "on_ready_fail_test"]);
    assert!(
        !output.status.success(),
        "Start should fail when blocking on_ready hook fails (daemon not ready)"
    );
}

/// Test that pre_stop hook runs before daemon is stopped
#[test]
fn test_hook_pre_stop() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("pre_stop");

    let toml_content = format!(
        r#"
[daemons.pre_stop_test]
run = "sleep 60"
ready_delay = 1

[daemons.pre_stop_test.hooks]
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
    assert!(!marker.exists(), "pre_stop hook should not have run yet");

    let output = env.run_command(&["stop", "pre_stop_test"]);
    assert!(
        output.status.success(),
        "Stop should succeed when pre_stop hook passes: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "pre_stop hook should have created marker file"
    );
}

/// Test that pre_stop hook failure prevents daemon from being stopped
#[test]
fn test_hook_pre_stop_fails_blocks_stop() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.pre_stop_fail_test]
run = "sleep 60"
ready_delay = 1

[daemons.pre_stop_fail_test.hooks]
pre_stop = { run = "exit 1", block = true }
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
        "Stop should fail when pre_stop hook fails"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pre_stop hook") || stderr.contains("exited with"),
        "Error should mention hook failure: {stderr}"
    );
}

/// Test that on_exit hook runs after daemon has stopped
#[test]
fn test_hook_on_exit() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_exit");

    let toml_content = format!(
        r#"
[daemons.on_exit_test]
run = "sleep 60"
ready_delay = 1

[daemons.on_exit_test.hooks]
on_exit = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "on_exit_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Marker should NOT exist yet (on_exit runs after stop)
    assert!(!marker.exists(), "on_exit hook should not have run yet");

    let output = env.run_command(&["stop", "on_exit_test"]);
    assert!(
        output.status.success(),
        "Stop should succeed when on_exit hook passes: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // on_exit is fire-and-forget, so poll for the marker file
    let mut found = false;
    for _ in 0..30 {
        if marker.exists() {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(found, "on_exit hook should have created marker file");
}

/// Test that on_exit hook receives PITCHFORK_EXIT_CODE and PITCHFORK_EXIT_REASON env vars
#[test]
fn test_hook_on_exit_env_vars() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_exit_env");

    let toml_content = format!(
        r#"
[daemons.on_exit_env_test]
run = "sleep 60"
ready_delay = 1

[daemons.on_exit_env_test.hooks]
on_exit = "sh -c 'echo $PITCHFORK_EXIT_CODE $PITCHFORK_EXIT_REASON > {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "on_exit_env_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = env.run_command(&["stop", "on_exit_env_test"]);
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
        "on_exit hook should have created marker file"
    );
    assert_eq!(
        content.trim(),
        "-1 stop",
        "on_exit hook should receive PITCHFORK_EXIT_CODE=-1 and PITCHFORK_EXIT_REASON=stop"
    );
}

/// Test that hook commands receive PITCHFORK_DAEMON_ID and PITCHFORK_RETRY_COUNT env vars
#[test]
fn test_hook_env_vars() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("hook_env");

    let toml_content = format!(
        r#"
[daemons.hook_env_test]
run = "sleep 60"
ready_delay = 1

[daemons.hook_env_test.hooks]
pre_start = "sh -c 'echo $PITCHFORK_DAEMON_ID $PITCHFORK_RETRY_COUNT > {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "hook_env_test"]);
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
        "pre_start hook should have created marker file"
    );
    assert_eq!(
        content.trim(),
        "project/hook_env_test 0",
        "Hook should receive PITCHFORK_DAEMON_ID and PITCHFORK_RETRY_COUNT=0"
    );

    env.run_command(&["stop", "hook_env_test"]);
}

/// Test that hook shorthand form (string) works
#[test]
fn test_hook_shorthand_form() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("hook_shorthand");

    let toml_content = format!(
        r#"
[daemons.hook_shorthand_test]
run = "sleep 60"
ready_delay = 1

[daemons.hook_shorthand_test.hooks]
pre_start = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "hook_shorthand_test"]);
    assert!(
        output.status.success(),
        "Start should succeed with shorthand hook form: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "Shorthand hook should have created marker file"
    );

    env.run_command(&["stop", "hook_shorthand_test"]);
}

/// Test that hook full form (object with block and timeout) works
#[test]
fn test_hook_full_form_with_block_and_timeout() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("hook_full_form");

    let toml_content = format!(
        r#"
[daemons.hook_full_form_test]
run = "sleep 60"
ready_delay = 1

[daemons.hook_full_form_test.hooks]
pre_start = {{ run = "touch {}", block = true, timeout = "30s" }}
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "hook_full_form_test"]);
    assert!(
        output.status.success(),
        "Start should succeed with full form hook: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "Full form hook should have created marker file"
    );

    env.run_command(&["stop", "hook_full_form_test"]);
}

/// Test that no hook configured means the lifecycle proceeds normally
#[test]
fn test_no_hook_configured() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.no_hook_test]
run = "sleep 60"
ready_delay = 1
"#
    .to_string();
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "no_hook_test"]);
    assert!(
        output.status.success(),
        "Start should succeed when no hook is configured: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    env.run_command(&["stop", "no_hook_test"]);
}

/// Test that on_exit hook runs even when process was already dead
#[test]
fn test_hook_on_exit_already_dead() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_exit_dead");

    let toml_content = format!(
        r#"
[daemons.on_exit_dead_test]
run = "sh -c 'sleep 1 && exit 0'"
ready_delay = 1

[daemons.on_exit_dead_test.hooks]
on_exit = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "on_exit_dead_test"]);
    // Daemon exits quickly, start may or may not succeed depending on timing
    eprintln!("start stdout: {}", String::from_utf8_lossy(&output.stdout));
    eprintln!("start stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Wait for daemon to exit
    std::thread::sleep(Duration::from_secs(2));

    // Stop the already-dead daemon — on_exit hook should still run
    let output = env.run_command(&["stop", "on_exit_dead_test"]);
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
        "on_exit hook should run even when process was already dead"
    );
}

/// Test that multiple hooks work together (pre_start + on_ready + pre_stop + on_exit)
#[test]
fn test_hook_all_lifecycle_points() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let pre_start_marker = env.marker_path("all_pre_start");
    let on_ready_marker = env.marker_path("all_on_ready");
    let pre_stop_marker = env.marker_path("all_pre_stop");
    let on_exit_marker = env.marker_path("all_on_exit");

    let toml_content = format!(
        r#"
[daemons.all_hooks_test]
run = "sleep 60"
ready_delay = 1

[daemons.all_hooks_test.hooks]
pre_start = "touch {}"
on_ready = "touch {}"
pre_stop = "touch {}"
on_exit = "touch {}"
"#,
        pre_start_marker.display(),
        on_ready_marker.display(),
        pre_stop_marker.display(),
        on_exit_marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "all_hooks_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(pre_start_marker.exists(), "pre_start hook should have run");
    let on_ready_ok = env.poll_file_exists(&on_ready_marker, Duration::from_secs(5));
    assert!(on_ready_ok, "on_ready hook should have run");
    assert!(
        !pre_stop_marker.exists(),
        "pre_stop hook should not have run yet"
    );
    assert!(
        !on_exit_marker.exists(),
        "on_exit hook should not have run yet"
    );

    let output = env.run_command(&["stop", "all_hooks_test"]);
    assert!(
        output.status.success(),
        "Stop should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(pre_stop_marker.exists(), "pre_stop hook should have run");

    // on_exit is fire-and-forget, so poll for the marker file
    let mut on_exit_found = false;
    for _ in 0..30 {
        if on_exit_marker.exists() {
            on_exit_found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(on_exit_found, "on_exit hook should have run");
}
