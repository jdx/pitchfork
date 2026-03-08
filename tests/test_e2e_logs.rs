mod common;

use common::{TestEnv, get_script_path};
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::Child;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

struct ChildGuard {
    child: Child,
    stderr_thread: Option<JoinHandle<()>>,
}

impl ChildGuard {
    fn new(child: Child, stderr_thread: JoinHandle<()>) -> Self {
        Self {
            child,
            stderr_thread: Some(stderr_thread),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(stderr_thread) = self.stderr_thread.take() {
            let _ = stderr_thread.join();
        }
    }
}

fn start_web_supervisor(env: &TestEnv) -> (ChildGuard, u16) {
    let port = "0";
    let mut child = env.run_background(&["supervisor", "run", "--web-port", port]);
    let (actual_port, stderr_thread) = wait_for_web_server(&mut child);
    (ChildGuard::new(child, stderr_thread), actual_port)
}

fn wait_for_web_server(child: &mut Child) -> (u16, JoinHandle<()>) {
    let stderr = child
        .stderr
        .take()
        .expect("supervisor stderr should be piped");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (port_tx, port_rx) = mpsc::channel();
    let stderr_thread = std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        let mut recent_lines = Vec::new();
        let mut sent_port = false;
        for line in reader.lines() {
            let line = line.expect("failed to read supervisor stderr");
            if recent_lines.len() == 20 {
                recent_lines.remove(0);
            }
            recent_lines.push(line.clone());

            if !sent_port
                && let Some(port_start) = line.find("Web UI listening on http://127.0.0.1:")
            {
                let port_str = &line[port_start + "Web UI listening on http://127.0.0.1:".len()..];
                let port_digits: String = port_str
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(port) = port_digits.parse::<u16>() {
                    let _ = port_tx.send(Ok(port));
                    sent_port = true;
                }
            }
        }

        if !sent_port {
            let _ = port_tx.send(Err(recent_lines));
        }
    });

    let port = port_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("web server did not report a listening port in time")
        .unwrap_or_else(|lines| {
            panic!(
                "web server exited before reporting a port; recent stderr: {}",
                lines.join(" | ")
            )
        });

    rt.block_on(async move {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/health");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(response) = client.get(&url).send().await
                && response.status().is_success()
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "web server did not become ready on port {port}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    (port, stderr_thread)
}

fn read_sse_until(
    url: &str,
    on_open: Option<mpsc::Sender<()>>,
    on_chunk: impl FnMut(&str),
    predicate: impl Fn(&str) -> bool,
) -> String {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        let mut on_chunk = on_chunk;
        let client = reqwest::Client::new();
        let mut response = client
            .get(url)
            .send()
            .await
            .expect("failed to connect to SSE endpoint");
        assert!(
            response.status().is_success(),
            "SSE endpoint should succeed"
        );
        if let Some(tx) = on_open {
            let _ = tx.send(());
        }

        let mut body = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline {
            let chunk = match tokio::time::timeout(Duration::from_secs(2), response.chunk()).await {
                Err(_) => continue,
                Ok(Err(err)) => panic!("failed to read SSE chunk: {err}"),
                Ok(Ok(chunk)) => chunk,
            };
            let Some(chunk) = chunk else {
                break;
            };
            let chunk_text = String::from_utf8_lossy(&chunk);
            body.push_str(&chunk_text);
            on_chunk(&body);
            if predicate(&body) {
                return body;
            }
        }

        panic!("did not receive expected SSE output, got: {body}");
    })
}

fn wait_for_log_content(env: &TestEnv, daemon_id: &str, needle: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        let logs = env.read_logs(daemon_id);
        if logs.contains(needle) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "log file for {daemon_id} did not contain '{needle}' in time"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_supervisor_cli(env: &TestEnv) {
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        let output = env.run_command(&["list"]);
        if output.status.success() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "supervisor CLI did not become ready in time: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ============================================================================
// Log Viewing Tests
// ============================================================================

#[test]
fn test_logs_tail_command() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.test_tail]
run = "bun run {} 1 5"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "test_tail"]);

    // Wait for the first log entry to be written before starting tail
    // This ensures the log file exists when tail_logs starts monitoring
    env.sleep(Duration::from_secs(2));

    // Test tail command
    let mut child = env.run_background(&["logs", "-t", "test_tail"]);

    // Wait for some more output to be generated
    env.sleep(Duration::from_secs(4));

    // Kill the tail process
    let _ = child.kill();
    let output = child.wait_with_output().expect("Failed to get output");

    // Check the stdout from the tail command
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Tail output: {stdout}");

    // Verify that tail command captured the streaming output
    assert!(!stdout.is_empty(), "Tail output should not be empty");
    assert!(
        stdout.contains("Output 5/5") || stdout.contains("Output 4/5"),
        "Tail output should contain new output: {stdout}"
    );

    // Also verify the log file has content
    let logs = env.read_logs("test_tail");
    assert!(!logs.is_empty(), "Log file should not be empty");

    // Clean up
    env.run_command(&["stop", "test_tail"]);
}

#[test]
fn test_logs_follow_alias() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.test_follow]
run = "bun run {} 1 5"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "test_follow"]);

    // Wait for the first log entry to be written before starting follow
    env.sleep(Duration::from_secs(2));

    // Test follow command with -f alias
    let mut child = env.run_background(&["logs", "-f", "test_follow"]);

    // Wait for some more output to be generated
    env.sleep(Duration::from_secs(4));

    // Kill the follow process
    let _ = child.kill();
    let output = child.wait_with_output().expect("Failed to get output");

    // Check the stdout from the follow command
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Follow output: {stdout}");

    // Verify that follow command captured the streaming output
    assert!(!stdout.is_empty(), "Follow output should not be empty");
    assert!(
        stdout.contains("Output 5/5") || stdout.contains("Output 4/5"),
        "Follow output should contain new output: {stdout}"
    );

    // Clean up
    env.run_command(&["stop", "test_follow"]);
}

#[test]
fn test_web_logs_sse_skips_existing_content_on_connect() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.sse_connect]
run = "bun run {} 1 6"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    let (_supervisor, port) = start_web_supervisor(&env);
    wait_for_supervisor_cli(&env);

    env.run_command(&["start", "sse_connect"]);
    wait_for_log_content(&env, "sse_connect", "Output 1/6");

    let existing_logs = env.read_logs("sse_connect");
    assert!(
        existing_logs.contains("Output 1/6"),
        "expected existing log content before SSE connect"
    );

    let stream_url = format!("http://127.0.0.1:{port}/logs/project%2Fsse_connect/stream");
    let body = read_sse_until(
        &stream_url,
        None,
        |_| {},
        |body| body.contains("Output 4/6"),
    );

    assert!(
        body.contains("Output 4/6"),
        "SSE should stream newly appended content"
    );
    assert!(
        !body.contains("Output 1/6"),
        "SSE should not replay existing content on initial connect: {body}"
    );

    env.run_command(&["stop", "sse_connect"]);
}

#[cfg(unix)]
#[test]
fn test_web_logs_sse_clears_and_restreams_after_rotation() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_toml("");

    let (_supervisor, port) = start_web_supervisor(&env);
    wait_for_supervisor_cli(&env);

    let log_path = env.log_path("rotate_sse");
    fs::create_dir_all(log_path.parent().unwrap()).unwrap();
    fs::write(&log_path, "").unwrap();

    let stream_url = format!("http://127.0.0.1:{port}/logs/project%2Frotate_sse/stream");
    let (open_tx, open_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let reader = std::thread::spawn({
        let stream_url = stream_url.clone();
        move || {
            let mut ready_tx = Some(ready_tx);
            read_sse_until(
                &stream_url,
                Some(open_tx),
                move |body| {
                    if body.contains("ready before rotation")
                        && let Some(tx) = ready_tx.take()
                    {
                        let _ = tx.send(());
                    }
                },
                |body| body.contains("event: clear") && body.contains("new line after rotation"),
            )
        }
    });

    open_rx
        .recv_timeout(Duration::from_secs(8))
        .expect("SSE stream did not connect in time");
    fs::write(&log_path, "ready before rotation\n").unwrap();
    ready_rx
        .recv_timeout(Duration::from_secs(8))
        .expect("SSE stream did not observe pre-rotation content in time");

    let rotated_path = log_path.with_extension("log.1");
    fs::rename(&log_path, &rotated_path).unwrap();
    fs::write(&log_path, "new line after rotation\n").unwrap();

    let body = reader.join().unwrap();
    assert!(
        body.contains("event: clear"),
        "rotation should emit a clear event: {body}"
    );
    assert!(
        body.contains("ready before rotation"),
        "expected pre-rotation content before clear event: {body}"
    );
    assert!(
        body.contains("new line after rotation"),
        "rotation should stream the new file from the beginning: {body}"
    );
}

#[test]
fn test_logs_clear_all() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    // Create two daemons so we can verify --clear clears all logs
    let toml_content = format!(
        r#"
[daemons.clear_test_1]
run = "bun run {} 1 3"
ready_delay = 0

[daemons.clear_test_2]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display(),
        script.display()
    );
    env.create_toml(&toml_content);

    // Start both daemons to generate logs
    env.run_command(&["start", "clear_test_1"]);
    env.run_command(&["start", "clear_test_2"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // Verify logs exist for both daemons
    let logs1 = env.read_logs("clear_test_1");
    let logs2 = env.read_logs("clear_test_2");
    assert!(!logs1.is_empty(), "Daemon 1 should have logs");
    assert!(!logs2.is_empty(), "Daemon 2 should have logs");

    // Clear all logs without specifying daemon
    let output = env.run_command(&["logs", "--clear"]);
    assert!(
        output.status.success(),
        "logs --clear should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify logs are cleared for both daemons
    let logs1_after = env.read_logs("clear_test_1");
    let logs2_after = env.read_logs("clear_test_2");
    assert!(
        logs1_after.is_empty(),
        "Daemon 1 logs should be cleared, got: {logs1_after}"
    );
    assert!(
        logs2_after.is_empty(),
        "Daemon 2 logs should be cleared, got: {logs2_after}"
    );

    // Clean up
    env.run_command(&["stop", "clear_test_1"]);
    env.run_command(&["stop", "clear_test_2"]);
}

// ============================================================================
// Time Filter Tests (--since / --until)
// ============================================================================

#[test]
fn test_logs_since_relative_time() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.since_test]
run = "bun run {} 1 5"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "since_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(6));

    // Test --since with relative time (last 3 seconds)
    let output = env.run_command(&["logs", "since_test", "--since", "3s"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs since 3s: {stdout}");

    assert!(output.status.success(), "logs --since should succeed");
    assert!(
        !stdout.is_empty(),
        "Should have some logs from last 3 seconds"
    );

    // Clean up
    env.run_command(&["stop", "since_test"]);
}

#[test]
fn test_logs_since_time_only() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.time_only_test]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "time_only_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // Get current time and format as HH:MM
    let now = chrono::Local::now();
    let time_str = now.format("%H:%M").to_string();

    // Test --since with time only (should use today's date)
    let output = env.run_command(&["logs", "time_only_test", "--since", &time_str]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs since {time_str}: {stdout}");

    assert!(
        output.status.success(),
        "logs --since with time only should succeed"
    );

    // Clean up
    env.run_command(&["stop", "time_only_test"]);
}

#[test]
fn test_logs_since_until_range() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.range_test]
run = "bun run {} 1 5"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    let start_time = chrono::Local::now();

    // Start daemon
    env.run_command(&["start", "range_test"]);

    // Wait for some logs
    env.sleep(Duration::from_secs(3));

    let mid_time = chrono::Local::now();

    // Wait for more logs
    env.sleep(Duration::from_secs(3));

    // Test --since and --until together
    let since_str = start_time.format("%Y-%m-%d %H:%M:%S").to_string();
    let until_str = mid_time.format("%Y-%m-%d %H:%M:%S").to_string();

    let output = env.run_command(&[
        "logs",
        "range_test",
        "--since",
        &since_str,
        "--until",
        &until_str,
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs in range: {stdout}");

    assert!(
        output.status.success(),
        "logs with time range should succeed"
    );

    // Verify we got some logs but not all
    let log_count = stdout.lines().count();
    println!("Log lines in range: {log_count}");

    // Should have some logs from the first 3 seconds
    assert!(log_count > 0, "Should have some logs in the time range");

    // Clean up
    env.run_command(&["stop", "range_test"]);
}

#[test]
fn test_logs_since_with_n_limit() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.since_n_test]
run = "bun run {} 1 10"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "since_n_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(11));

    // Test --since with -n limit
    let output = env.run_command(&["logs", "since_n_test", "--since", "10s", "-n", "3"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs since 10s with -n 3: {stdout}");

    assert!(
        output.status.success(),
        "logs --since with -n should succeed"
    );

    // Should limit to 3 lines (or fewer if less than 3 lines match)
    let line_count = stdout.lines().filter(|l| !l.trim().is_empty()).count();
    assert!(
        line_count <= 3,
        "Should have at most 3 lines, got {line_count}"
    );

    // Clean up
    env.run_command(&["stop", "since_n_test"]);
}

// ============================================================================
// Raw Output Tests
// ============================================================================

#[test]
fn test_logs_raw_output() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.raw_test]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "raw_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // Test --raw output
    let output = env.run_command(&["logs", "raw_test", "--raw"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Raw logs: {stdout}");

    assert!(output.status.success(), "logs --raw should succeed");
    assert!(!stdout.is_empty(), "Should have raw logs");

    // Raw output should not have ANSI color codes (basic check)
    // ANSI codes typically start with \x1b[ or \033[
    assert!(
        !stdout.contains("\x1b["),
        "Raw output should not contain ANSI escape codes"
    );

    // Clean up
    env.run_command(&["stop", "raw_test"]);
}

#[test]
fn test_logs_raw_with_time_filter() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.raw_time_test]
run = "bun run {} 1 5"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "raw_time_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(6));

    // Test --raw with --since
    let output = env.run_command(&["logs", "raw_time_test", "--raw", "--since", "5s"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Raw logs with time filter: {stdout}");

    assert!(
        output.status.success(),
        "logs --raw with --since should succeed"
    );
    assert!(!stdout.is_empty(), "Should have raw logs from time range");

    // Clean up
    env.run_command(&["stop", "raw_time_test"]);
}

// ============================================================================
// Line Limit Tests (-n)
// ============================================================================

#[test]
fn test_logs_n_limit() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.n_limit_test]
run = "bun run {} 1 10"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "n_limit_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(11));

    // Test -n limit
    let output = env.run_command(&["logs", "n_limit_test", "-n", "5"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs with -n 5: {stdout}");

    assert!(output.status.success(), "logs -n should succeed");

    // Count non-empty lines
    let line_count = stdout.lines().filter(|l| !l.trim().is_empty()).count();
    println!("Line count: {line_count}");

    // Should have at most 5 lines
    assert!(
        line_count <= 5,
        "Should have at most 5 lines, got {line_count}"
    );

    // Clean up
    env.run_command(&["stop", "n_limit_test"]);
}

#[test]
fn test_logs_without_n_uses_pager() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.pager_test]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "pager_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // When running without -n in non-interactive mode, should output all logs
    let output = env.run_command(&["logs", "pager_test"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs without -n: {stdout}");

    assert!(output.status.success(), "logs without -n should succeed");
    assert!(!stdout.is_empty(), "Should have logs");

    // Clean up
    env.run_command(&["stop", "pager_test"]);
}

// ============================================================================
// Multiple Daemon Tests
// ============================================================================

#[test]
fn test_logs_multiple_daemons() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.multi_log_1]
run = "bun run {} 1 3"
ready_delay = 0

[daemons.multi_log_2]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display(),
        script.display()
    );
    env.create_toml(&toml_content);

    // Start both daemons
    env.run_command(&["start", "multi_log_1"]);
    env.run_command(&["start", "multi_log_2"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // Test viewing logs from multiple daemons
    let output = env.run_command(&["logs", "multi_log_1", "multi_log_2"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs from multiple daemons: {stdout}");

    assert!(
        output.status.success(),
        "logs for multiple daemons should succeed"
    );
    assert!(!stdout.is_empty(), "Should have logs from both daemons");

    // Verify logs contain output from both daemons
    // In multi-daemon mode, daemon names should be shown
    assert!(
        stdout.contains("multi_log_1") || stdout.contains("Output"),
        "Should contain logs from daemon 1"
    );
    assert!(
        stdout.contains("multi_log_2") || stdout.contains("Output"),
        "Should contain logs from daemon 2"
    );

    // Clean up
    env.run_command(&["stop", "multi_log_1"]);
    env.run_command(&["stop", "multi_log_2"]);
}

// ============================================================================
// No-Pager Tests
// ============================================================================

#[test]
fn test_logs_no_pager() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let script = get_script_path("slowly_output.ts");
    let toml_content = format!(
        r#"
[daemons.no_pager_test]
run = "bun run {} 1 3"
ready_delay = 0
"#,
        script.display()
    );
    env.create_toml(&toml_content);

    // Start daemon
    env.run_command(&["start", "no_pager_test"]);

    // Wait for logs to be generated
    env.sleep(Duration::from_secs(4));

    // Test --no-pager flag
    let output = env.run_command(&["logs", "no_pager_test", "--no-pager"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Logs with --no-pager: {stdout}");

    assert!(output.status.success(), "logs --no-pager should succeed");
    assert!(!stdout.is_empty(), "Should have logs");

    // Clean up
    env.run_command(&["stop", "no_pager_test"]);
}
