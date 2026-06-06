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

    // Poll for marker content (hook fires async, and shell redirection can
    // create the file before echo writes its value).
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
        "on_fail hook should have created marker file"
    );

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
    // Use PITCHFORK_INTERVAL=1s to speed this up
    let output = env.run_command_with_env(
        &["start", "fail_retry_hook"],
        &[("PITCHFORK_INTERVAL", "1s")],
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
        "Should have 2 retry hook invocations (retry=2), got: {lines:?}"
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
        "project/id_env_test",
        "PITCHFORK_DAEMON_ID should be the qualified daemon id (namespace/name)"
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
        "project/hook_env_test 7",
        "Hook should receive qualified PITCHFORK_DAEMON_ID and PITCHFORK_EXIT_CODE"
    );
}

/// Test that on_stop hook fires when a daemon is explicitly stopped
#[test]
fn test_hook_on_stop() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_stop");

    let toml_content = format!(
        r#"
[daemons.stop_hook_test]
run = "sleep 60"
ready_delay = 1

[daemons.stop_hook_test.hooks]
on_stop = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "stop_hook_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Explicitly stop the daemon
    let output = env.run_command(&["stop", "stop_hook_test"]);
    assert!(
        output.status.success(),
        "Stop should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Poll for marker file (hook fires async)
    for _ in 0..30 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        marker.exists(),
        "on_stop hook should have created marker file after explicit stop"
    );
}

/// Test that on_stop hook receives PITCHFORK_EXIT_REASON=stop
#[test]
fn test_hook_on_stop_exit_reason() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_stop_reason");

    let toml_content = format!(
        r#"
[daemons.stop_reason_test]
run = "sleep 60"
ready_delay = 1

[daemons.stop_reason_test.hooks]
on_stop = "sh -c 'echo $PITCHFORK_EXIT_REASON > {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "stop_reason_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    env.run_command(&["stop", "stop_reason_test"]);

    // Poll for marker file
    for _ in 0..30 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        marker.exists(),
        "on_stop hook should have created marker file"
    );

    let content = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        content.trim(),
        "stop",
        "PITCHFORK_EXIT_REASON should be 'stop' when daemon is explicitly stopped"
    );
}

/// Test that on_exit hook fires when a daemon is explicitly stopped
#[test]
fn test_hook_on_exit_on_stop() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_exit_stop");

    let toml_content = format!(
        r#"
[daemons.exit_stop_test]
run = "sleep 60"
ready_delay = 1

[daemons.exit_stop_test.hooks]
on_exit = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "exit_stop_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    env.run_command(&["stop", "exit_stop_test"]);

    // Poll for marker file
    for _ in 0..30 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        marker.exists(),
        "on_exit hook should fire when daemon is stopped"
    );
}

/// Test that on_exit hook fires when a daemon crashes (non-zero exit)
#[test]
fn test_hook_on_exit_on_fail() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_exit_fail");

    let toml_content = format!(
        r#"
[daemons.exit_fail_test]
run = "sh -c 'exit 1'"
retry = 0

[daemons.exit_fail_test.hooks]
on_exit = "sh -c 'echo $PITCHFORK_EXIT_REASON > {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "exit_fail_test"]);
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
        "on_exit hook should fire when daemon crashes"
    );

    let content = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        content.trim(),
        "fail",
        "PITCHFORK_EXIT_REASON should be 'fail' when daemon exits with non-zero code"
    );
}

/// Test that on_exit hook fires when a daemon exits cleanly on its own (exit code 0)
#[test]
fn test_hook_on_exit_clean_exit() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_exit_clean");

    let toml_content = format!(
        r#"
[daemons.exit_clean_test]
run = "sh -c 'exit 0'"
retry = 0

[daemons.exit_clean_test.hooks]
on_exit = "sh -c 'echo $PITCHFORK_EXIT_REASON > {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "exit_clean_test"]);
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
        "on_exit hook should fire when daemon exits cleanly on its own"
    );

    let content = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        content.trim(),
        "exit",
        "PITCHFORK_EXIT_REASON should be 'exit' when daemon exits cleanly on its own"
    );
}

/// Test that both on_stop and on_exit fire when daemon is explicitly stopped
#[test]
fn test_hook_on_stop_and_on_exit_both_fire() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let stop_marker = env.marker_path("both_on_stop");
    let exit_marker = env.marker_path("both_on_exit");

    let toml_content = format!(
        r#"
[daemons.both_hooks_test]
run = "sleep 60"
ready_delay = 1

[daemons.both_hooks_test.hooks]
on_stop = "touch {}"
on_exit = "touch {}"
"#,
        stop_marker.display(),
        exit_marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "both_hooks_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    env.run_command(&["stop", "both_hooks_test"]);

    // Poll for both marker files
    for _ in 0..30 {
        if stop_marker.exists() && exit_marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        stop_marker.exists(),
        "on_stop hook should fire when daemon is explicitly stopped"
    );
    assert!(
        exit_marker.exists(),
        "on_exit hook should also fire when daemon is explicitly stopped"
    );
}

/// Test that on_exit does NOT fire during retry attempts, only after retries are exhausted
#[test]
fn test_hook_on_exit_not_fired_during_retries() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let counter_file = env.marker_path("on_exit_retry_count");

    // retry = 2 means 3 total attempts; on_exit should only fire once (after final failure)
    let toml_content = format!(
        r#"
[daemons.exit_retry_guard_test]
run = "sh -c 'exit 1'"
retry = 2

[daemons.exit_retry_guard_test.hooks]
on_exit = "sh -c 'echo x >> {}'"
"#,
        counter_file.display()
    );
    env.create_toml(&toml_content);

    // Use a very large PITCHFORK_INTERVAL to prevent check_retry() from firing
    // during the test. The run() retry loop (wait_ready=true path) handles all
    // retries internally. If check_retry() runs concurrently, it would duplicate
    // retry attempts and cause on_exit to fire more than once.
    let output = env.run_command_with_env(
        &["start", "exit_retry_guard_test"],
        &[("PITCHFORK_INTERVAL", "600s")],
    );
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Wait for the on_exit hook to fire after all retries are exhausted.
    // The run() retry loop uses exponential backoff (1s, 2s), so total time is ~3s+
    for _ in 0..60 {
        if let Ok(content) = std::fs::read_to_string(&counter_file) {
            if !content.trim().is_empty() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    assert!(
        counter_file.exists(),
        "on_exit hook should have fired after retries exhausted"
    );
    let content = std::fs::read_to_string(&counter_file).unwrap();
    let fire_count = content.lines().count();
    assert_eq!(
        fire_count, 1,
        "on_exit should fire exactly once (after retries exhausted), not on each crash attempt, got {fire_count} fires"
    );
}

/// Test that on_crash hook fires when a previously-running daemon crashes and retries are exhausted
#[test]
fn test_hook_on_crash() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_crash");

    // Daemon becomes ready (prints "ready"), then crashes after 0.5s.
    // With recovery = 0 (default when not specified), the daemon crashes
    // immediately after becoming ready, triggering on_crash (not on_fail).
    let toml_content = format!(
        r#"
[daemons.crash_hook_test]
run = "sh -c 'echo ready && sleep 0.5 && exit 1'"
retry = 1
recovery = 0
ready_output = "ready"

[daemons.crash_hook_test.hooks]
on_crash = "touch {}"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "crash_hook_test"]);
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Wait for daemon to become ready, crash, and on_crash to fire
    for _ in 0..60 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    assert!(
        marker.exists(),
        "on_crash hook should fire when daemon becomes ready then crashes with recovery exhausted"
    );
}

/// Test that on_recover hook fires before each runtime recovery attempt
#[test]
fn test_hook_on_recover() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("on_recover");

    let toml_content = format!(
        r#"
[daemons.recover_hook_test]
run = "sh -c 'exit 1'"
retry = 2

[daemons.recover_hook_test.hooks]
on_recover = "sh -c 'echo recover >> {}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    // Start the daemon - it will fail immediately, then check_retry will
    // attempt recovery with on_recover hook
    let output = env.run_command_with_env(
        &["start", "recover_hook_test"],
        &[("PITCHFORK_INTERVAL", "1s")],
    );
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Wait for recovery attempts
    for _ in 0..60 {
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
        "on_recover hook should have created marker file"
    );
    let content = std::fs::read_to_string(&marker).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert!(
        lines.len() >= 2,
        "Should have at least 2 on_recover invocations (retry=2), got: {lines:?}"
    );
}

/// Test that PITCHFORK_RECOVERY_COUNT env var is available and incremented
#[test]
fn test_env_var_pitchfork_recovery_count() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let marker = env.marker_path("recovery_count");

    let toml_content = format!(
        r#"
[daemons.recovery_count_test]
run = "sh -c 'echo $PITCHFORK_RECOVERY_COUNT > {0} && exit 1'"
retry = 2

[daemons.recovery_count_test.hooks]
on_recover = "sh -c 'echo $PITCHFORK_RECOVERY_COUNT >> {0}'"
"#,
        marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command_with_env(
        &["start", "recovery_count_test"],
        &[("PITCHFORK_INTERVAL", "1s")],
    );
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));

    // Wait for recovery to happen
    for _ in 0..60 {
        if let Ok(content) = std::fs::read_to_string(&marker) {
            if !content.trim().is_empty() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    assert!(marker.exists(), "recovery count marker should exist");
}

/// Test that explicit recovery config limits runtime recovery independently from retry
#[test]
fn test_recovery_config_limits_runtime_recovery() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let counter_file = env.marker_path("recovery_counter");

    // The daemon becomes ready (prints "ready"), then crashes after 0.5s.
    // With recovery = 1, only 1 runtime recovery should happen.
    // With retry = 5, startup would allow 5 retries, but recovery is independent.
    let toml_content = format!(
        r#"
[daemons.recovery_limit_test]
run = "sh -c 'echo ready && sleep 0.5 && exit 1'"
retry = 5
recovery = 1
ready_output = "ready"

[daemons.recovery_limit_test.hooks]
on_recover = "sh -c 'echo x >> {0}'"
on_crash = "touch {1}"
"#,
        counter_file.display(),
        env.marker_path("on_crash_recovery").display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "recovery_limit_test"]);
    assert!(
        output.status.success(),
        "Start should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Wait for on_crash to fire (recovery=1 means 1 recovery then crash)
    let crash_marker = env.marker_path("on_crash_recovery");
    for _ in 0..60 {
        if crash_marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    assert!(
        crash_marker.exists(),
        "on_crash should fire when recovery limit is exhausted"
    );

    let content = std::fs::read_to_string(&counter_file).unwrap_or_default();
    let recover_count = content.lines().count();
    assert_eq!(
        recover_count, 1,
        "Should have exactly 1 recovery attempt (recovery=1), got {recover_count}"
    );
}

/// Test that recovery = false prevents any runtime recovery
#[test]
fn test_recovery_false_no_runtime_recovery() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let crash_marker = env.marker_path("no_recovery_crash");
    let recover_marker = env.marker_path("no_recovery_recover");

    // Daemon becomes ready (prints "ready"), then crashes after 0.5s.
    // With recovery = false, no runtime recovery should happen.
    // on_crash should fire immediately (not on_recover).
    let toml_content = format!(
        r#"
[daemons.no_recovery_test]
run = "sh -c 'echo ready && sleep 0.5 && exit 1'"
retry = 3
recovery = false
ready_output = "ready"

[daemons.no_recovery_test.hooks]
on_recover = "touch {0}"
on_crash = "touch {1}"
"#,
        recover_marker.display(),
        crash_marker.display()
    );
    env.create_toml(&toml_content);

    let output = env.run_command(&["start", "no_recovery_test"]);
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));

    for _ in 0..60 {
        if crash_marker.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    assert!(
        crash_marker.exists(),
        "on_crash should fire immediately when recovery = false"
    );
    assert!(
        !recover_marker.exists(),
        "on_recover should NOT fire when recovery = false"
    );
}
