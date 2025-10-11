mod common;

use common::{get_script_path, TestEnv};
use std::time::Duration;

// ----------------------------------------------------------------------------
// Tests with failing tasks (fail.ts 5 - fails after 5 seconds)
// These should trigger at least twice in 2 minutes (0s, 30s, 60s, 90s)
// ----------------------------------------------------------------------------

#[test]
#[ignore]
fn test_cron_finish_with_failing_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");

    let toml_content = format!(
        r#"
[daemons.cron_finish_fail]
run = "bun run {} 5"
cron = {{ schedule = "*/30 * * * * *", retrigger = "finish" }}
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    let start_output = env.run_command(&["start", "cron_finish_fail"]);
    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );

    assert!(
        start_output.status.success(),
        "Start should succeed for cron daemon"
    );

    println!("Waiting 2 minutes for cron to trigger at least twice...");
    env.sleep(Duration::from_secs(120));

    let logs = env.read_logs("cron_finish_fail");
    println!("Logs:\n{}", logs);

    let execution_count = logs.matches("Failed after 5!").count();
    assert!(
        execution_count >= 2,
        "With 'finish' retrigger, task should execute at least 2 times (found {})",
        execution_count
    );

    let _ = env.run_command(&["stop", "cron_finish_fail"]);
}

#[test]
#[ignore]
fn test_cron_always_with_failing_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");

    let toml_content = format!(
        r#"
[daemons.cron_always_fail]
run = "bun run {} 5"
cron = {{ schedule = "*/30 * * * * *", retrigger = "always" }}
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    let start_output = env.run_command(&["start", "cron_always_fail"]);
    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );

    assert!(
        start_output.status.success(),
        "Start should succeed for cron daemon"
    );

    println!("Waiting 2 minutes for cron to trigger at least twice...");
    env.sleep(Duration::from_secs(120));

    let logs = env.read_logs("cron_always_fail");
    println!("Logs:\n{}", logs);

    let execution_count = logs.matches("Failed after 5!").count();
    assert!(
        execution_count >= 2,
        "With 'always' retrigger, task should execute at least 2 times (found {})",
        execution_count
    );

    let _ = env.run_command(&["stop", "cron_always_fail"]);
}

#[test]
#[ignore]
fn test_cron_success_with_failing_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");

    let toml_content = format!(
        r#"
[daemons.cron_success_fail]
run = "bun run {} 5"
cron = {{ schedule = "*/30 * * * * *", retrigger = "success" }}
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    let start_output = env.run_command(&["start", "cron_success_fail"]);
    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );

    assert!(
        start_output.status.success(),
        "Start should succeed for cron daemon"
    );

    println!("Waiting 2 minutes for cron schedule...");
    env.sleep(Duration::from_secs(120));

    let logs = env.read_logs("cron_success_fail");
    println!("Logs:\n{}", logs);

    let execution_count = logs.matches("Failed after 5!").count();
    assert_eq!(
        execution_count, 1,
        "With 'success' retrigger and failing task, should only execute once (found {})",
        execution_count
    );

    let _ = env.run_command(&["stop", "cron_success_fail"]);
}

#[test]
#[ignore]
fn test_cron_fail_with_failing_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let fail_script = get_script_path("fail.ts");

    let toml_content = format!(
        r#"
[daemons.cron_fail_fail]
run = "bun run {} 5"
cron = {{ schedule = "*/30 * * * * *", retrigger = "fail" }}
"#,
        fail_script.display()
    );
    env.create_toml(&toml_content);

    let start_output = env.run_command(&["start", "cron_fail_fail"]);
    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );

    assert!(
        start_output.status.success(),
        "Start should succeed for cron daemon"
    );

    println!("Waiting 2 minutes for cron to trigger at least twice...");
    env.sleep(Duration::from_secs(120));

    let logs = env.read_logs("cron_fail_fail");
    println!("Logs:\n{}", logs);

    let execution_count = logs.matches("Failed after 5!").count();
    assert!(
        execution_count >= 2,
        "With 'fail' retrigger, task should execute at least 2 times (found {})",
        execution_count
    );

    let _ = env.run_command(&["stop", "cron_fail_fail"]);
}

// ----------------------------------------------------------------------------
// Tests with long-running tasks (slowly_output.ts 2 999 - runs continuously)
// These should NOT retrigger (except 'always') as the task is still running
// ----------------------------------------------------------------------------

#[test]
#[ignore]
fn test_cron_finish_with_long_running_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let slowly_script = get_script_path("slowly_output.ts");

    let toml_content = format!(
        r#"
[daemons.cron_finish_long]
run = "bun run {} 2 999"
cron = {{ schedule = "*/30 * * * * *", retrigger = "finish" }}
"#,
        slowly_script.display()
    );
    env.create_toml(&toml_content);

    let start_output = env.run_command(&["start", "cron_finish_long"]);
    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );

    assert!(
        start_output.status.success(),
        "Start should succeed for cron daemon"
    );

    println!("Waiting 2 minutes to verify task does NOT retrigger...");
    env.sleep(Duration::from_secs(120));

    let logs = env.read_logs("cron_finish_long");
    println!("Logs:\n{}", logs);

    // Count how many times "Output 1/999" appears (indicates a new start)
    let start_count = logs.matches("Output 1/999").count();
    assert_eq!(
        start_count, 1,
        "With 'finish' retrigger and long-running task, should only start once (found {})",
        start_count
    );

    let _ = env.run_command(&["stop", "cron_finish_long"]);
}

#[test]
#[ignore]
fn test_cron_always_with_long_running_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let slowly_script = get_script_path("slowly_output.ts");

    let toml_content = format!(
        r#"
[daemons.cron_always_long]
run = "bun run {} 2 999"
cron = {{ schedule = "*/30 * * * * *", retrigger = "always" }}
"#,
        slowly_script.display()
    );
    env.create_toml(&toml_content);

    let start_output = env.run_command(&["start", "cron_always_long"]);
    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );

    assert!(
        start_output.status.success(),
        "Start should succeed for cron daemon"
    );

    println!("Waiting 2 minutes for cron to retrigger at least twice...");
    env.sleep(Duration::from_secs(120));

    let logs = env.read_logs("cron_always_long");
    println!("Logs:\n{}", logs);

    // Count how many times "Output 1/999" appears (indicates a new start)
    let start_count = logs.matches("Output 1/999").count();
    assert!(
        start_count >= 2,
        "With 'always' retrigger, task should restart at least 2 times (found {})",
        start_count
    );

    let _ = env.run_command(&["stop", "cron_always_long"]);
}

#[test]
#[ignore]
fn test_cron_success_with_long_running_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let slowly_script = get_script_path("slowly_output.ts");

    let toml_content = format!(
        r#"
[daemons.cron_success_long]
run = "bun run {} 2 999"
cron = {{ schedule = "*/30 * * * * *", retrigger = "success" }}
"#,
        slowly_script.display()
    );
    env.create_toml(&toml_content);

    let start_output = env.run_command(&["start", "cron_success_long"]);
    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );

    assert!(
        start_output.status.success(),
        "Start should succeed for cron daemon"
    );

    println!("Waiting 2 minutes to verify task does NOT retrigger...");
    env.sleep(Duration::from_secs(120));

    let logs = env.read_logs("cron_success_long");
    println!("Logs:\n{}", logs);

    // Count how many times "Output 1/999" appears (indicates a new start)
    let start_count = logs.matches("Output 1/999").count();
    assert_eq!(
        start_count, 1,
        "With 'success' retrigger and long-running task, should only start once (found {})",
        start_count
    );

    let _ = env.run_command(&["stop", "cron_success_long"]);
}

#[test]
#[ignore]
fn test_cron_fail_with_long_running_task() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let slowly_script = get_script_path("slowly_output.ts");

    let toml_content = format!(
        r#"
[daemons.cron_fail_long]
run = "bun run {} 2 999"
cron = {{ schedule = "*/30 * * * * *", retrigger = "fail" }}
"#,
        slowly_script.display()
    );
    env.create_toml(&toml_content);

    let start_output = env.run_command(&["start", "cron_fail_long"]);
    println!(
        "Start stdout: {}",
        String::from_utf8_lossy(&start_output.stdout)
    );
    println!(
        "Start stderr: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );

    assert!(
        start_output.status.success(),
        "Start should succeed for cron daemon"
    );

    println!("Waiting 2 minutes to verify task does NOT retrigger...");
    env.sleep(Duration::from_secs(120));

    let logs = env.read_logs("cron_fail_long");
    println!("Logs:\n{}", logs);

    // Count how many times "Output 1/999" appears (indicates a new start)
    let start_count = logs.matches("Output 1/999").count();
    assert_eq!(
        start_count, 1,
        "With 'fail' retrigger and long-running task, should only start once (found {})",
        start_count
    );

    let _ = env.run_command(&["stop", "cron_fail_long"]);
}
