mod common;

use common::{TestEnv, get_script_path};
use std::time::Duration;

const FAST_INTERVAL: (&str, &str) = ("PITCHFORK_INTERVAL_SECS", "2");

#[test]
fn test_interval_watch_long_running_task_stays_running() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.long_runner]
run = "sleep 60"
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Start the daemon with fast interval (2s instead of default 10s)
    let start_output = env.run_command_with_env(&["start", "long_runner"], &[FAST_INTERVAL]);

    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );

    // Sleep for 6 seconds to allow interval_watch to run multiple times (2s interval)
    println!("Waiting 6 seconds to let interval_watch refresh...");
    env.sleep(Duration::from_secs(6));

    // Check daemon status - should still be Running
    let status = env.get_daemon_status("long_runner");
    println!("Daemon status after 6s: {status:?}");

    let status = status.unwrap();
    assert!(
        status.contains("running"),
        "Daemon should still be Running after 6s, but was: {status}"
    );

    // Clean up
    let _ = env.run_command(&["stop", "long_runner"]);
}

#[test]
fn test_interval_watch_detects_failed_daemon() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");

    // Task runs for 5 seconds then fails - just after ready check (3s default)
    let toml_content = format!(
        r#"
[daemons.fail_after_ready]
run = "bun run {} 5"
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    // Start the daemon with fast interval - it should pass ready check
    let start_output = env.run_command_with_env(&["start", "fail_after_ready"], &[FAST_INTERVAL]);

    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );
    println!("Start exit code: {:?}", start_output.status.code());

    // Start should succeed (daemon passes ready check at 3s)
    assert!(
        start_output.status.success(),
        "Start should succeed as daemon passes ready check"
    );

    // Sleep for 10 seconds to allow daemon to fail (5s) and interval_watch to detect it (2s interval)
    println!("Waiting 10 seconds for daemon to fail and interval_watch to detect...");
    env.sleep(Duration::from_secs(10));

    // Check daemon status - should be Errored
    let status = env.get_daemon_status("fail_after_ready");
    println!("Daemon status after 10s: {status:?}");

    let status = status.unwrap();
    assert!(
        status.contains("errored"),
        "Daemon should be Errored after failing, but was: {status}"
    );

    // Verify logs show the failure
    let logs = env.read_logs("fail_after_ready");
    println!("Logs:\n{logs}");
    assert!(
        logs.contains("Failed after 5!"),
        "Logs should contain failure message"
    );

    // Clean up
    let _ = env.run_command(&["stop", "fail_after_ready"]);
}

#[test]
fn test_interval_watch_retry_on_failure() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");

    // Task runs for 5 seconds then fails, with retry=1 (2 total attempts)
    let toml_content = format!(
        r#"
[daemons.retry_after_ready]
run = "bun run {} 5"
retry = 1
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    // Start the daemon with fast interval - it should pass ready check
    let start_output = env.run_command_with_env(&["start", "retry_after_ready"], &[FAST_INTERVAL]);

    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );
    println!("Start exit code: {:?}", start_output.status.code());

    // Start should succeed (daemon passes ready check at 3s)
    assert!(
        start_output.status.success(),
        "Start should succeed as daemon passes ready check"
    );

    // Wait for daemon to fail (5s), retry interval (2s), fail again (5s), detect (2s) + buffer
    println!("Waiting 16 seconds for daemon to fail, retry, and fail again...");
    env.sleep(Duration::from_secs(16));

    // Check daemon status - should be Errored after exhausting retries
    let status = env.get_daemon_status("retry_after_ready");
    println!("Daemon status after 16s: {status:?}");

    let status = status.unwrap();
    assert!(
        status.contains("errored"),
        "Daemon should be Errored after exhausting retries, but was: {status}"
    );

    // Verify logs show TWO failures (original + 1 retry)
    let logs = env.read_logs("retry_after_ready");
    println!("Logs:\n{logs}");

    let failure_count = logs.matches("Failed after 5!").count();
    assert_eq!(
        failure_count, 2,
        "Logs should contain exactly 2 failure messages (original + 1 retry), found {failure_count}"
    );

    // Clean up
    let _ = env.run_command(&["stop", "retry_after_ready"]);
}
