mod common;

use common::TestEnv;
use std::fs;
use std::time::Duration;

// ============================================================================
// Slug Resolution Tests
// ============================================================================

/// Test that a daemon with a slug can be started/stopped using the slug
#[test]
fn test_slug_start_stop() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("slugtest");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("pitchfork.toml"),
        r#"
[daemons.api-server]
run = "sleep 60"
"#,
    )
    .unwrap();

    // Register slug in global config
    let _ = env.run_command_in_dir(&["proxy", "add", "api", "--daemon", "api-server"], &project);

    // Start using the slug from the same project directory.
    let output = env.run_command_in_dir(&["start", "api"], &project);
    println!("start stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("start stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "Start via slug should succeed");

    env.sleep(Duration::from_secs(1));

    // Status using slug
    let status_output = env.run_command_in_dir(&["status", "api"], &project);
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    println!("status: {status_str}");
    assert!(
        status_str.contains("running"),
        "Daemon should be running when queried by slug: {status_str}"
    );

    // Stop using slug
    let stop_output = env.run_command_in_dir(&["stop", "api"], &project);
    assert!(stop_output.status.success(), "Stop via slug should succeed");

    env.sleep(Duration::from_secs(1));

    // Verify stopped — status via slug should still resolve
    let status_output2 = env.run_command_in_dir(&["status", "api"], &project);
    let status_str2 = String::from_utf8_lossy(&status_output2.stdout);
    assert!(
        status_str2.contains("stopped") || status_str2.contains("exited"),
        "Daemon should be stopped: {status_str2}"
    );
}

/// Test that slug works from a different directory (cross-namespace resolution)
#[test]
fn test_slug_cross_directory() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("cross-slug");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("pitchfork.toml"),
        r#"
[daemons.backend]
run = "sleep 60"
"#,
    )
    .unwrap();

    // Register the slug in global config
    let _ = env.run_command_in_dir(&["proxy", "add", "be", "--daemon", "backend"], &project);

    // Start from project directory using daemon name
    let output = env.run_command_in_dir(&["start", "backend"], &project);
    assert!(output.status.success(), "Start should succeed");

    env.sleep(Duration::from_secs(1));

    // Query using slug from a different directory (no pitchfork.toml there).
    // The slug is resolved via the global config's [slugs] registry.
    let other_dir = env.create_other_dir();
    let status_output = env.run_command_in_dir(&["status", "be"], &other_dir);
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    println!("cross-dir slug status: {status_str}");
    assert!(
        status_str.contains("running"),
        "Should find daemon by slug from different directory: {status_str}"
    );

    // Stop using slug from different directory
    let stop_output = env.run_command_in_dir(&["stop", "be"], &other_dir);
    assert!(
        stop_output.status.success(),
        "Stop via slug from different directory should succeed"
    );
}

/// Test that slug takes priority over daemon name when both match
#[test]
fn test_slug_priority_over_name() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Project A: daemon named "web" with slug "frontend" (registered via proxy add)
    let project_a = env.project_dir().join("proj-a");
    fs::create_dir_all(&project_a).unwrap();
    fs::write(
        project_a.join("pitchfork.toml"),
        r#"
[daemons.web]
run = "sleep 60"
"#,
    )
    .unwrap();

    // Register slug "frontend" → proj-a daemon "web"
    let _ = env.run_command_in_dir(&["proxy", "add", "frontend", "--daemon", "web"], &project_a);

    // Project B: daemon named "frontend" (no slug)
    let project_b = env.project_dir().join("proj-b");
    fs::create_dir_all(&project_b).unwrap();
    fs::write(
        project_b.join("pitchfork.toml"),
        r#"
[daemons.frontend]
run = "sleep 60"
"#,
    )
    .unwrap();

    // Start both
    let _ = env.run_command_in_dir(&["start", "web"], &project_a);
    env.sleep(Duration::from_millis(500));
    let _ = env.run_command_in_dir(&["start", "frontend"], &project_b);
    env.sleep(Duration::from_secs(1));

    // When querying "frontend" from project_a, slug match should win
    // (proj-a/web has slug "frontend", proj-b/frontend has no slug)
    let status_output = env.run_command_in_dir(&["status", "frontend"], &project_a);
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    println!("slug priority status: {status_str}");
    // Should find proj-a/web via slug — the qualified name printed is "proj-a/web"
    // (or the display name "web" when namespace is unambiguous).
    // Verify it is NOT proj-b/frontend by checking the qualified ID in the output.
    assert!(
        status_str.contains("running"),
        "Should find daemon by slug: {status_str}"
    );
    // The status output prints "Name: <qualified_id>"; confirm it resolved to proj-a/web
    // and NOT to proj-b/frontend.
    assert!(
        !status_str.contains("proj-b"),
        "Slug match should resolve to proj-a/web, not proj-b/frontend: {status_str}"
    );
    // Positively confirm the output mentions proj-a or the daemon name "web".
    // Both "proj-a/web" and "web" are acceptable representations.
    assert!(
        status_str.contains("proj-a") || status_str.contains("web"),
        "Slug match should positively resolve to proj-a/web: {status_str}"
    );
    // Extra guard: the output must NOT contain "proj-b/frontend" in any form,
    // even if the output happens to include both daemons.
    assert!(
        !status_str.contains("proj-b/frontend"),
        "Output must not reference proj-b/frontend at all: {status_str}"
    );

    // Cleanup
    let _ = env.run_command_in_dir(&["stop", "web"], &project_a);
    let _ = env.run_command_in_dir(&["stop", "frontend"], &project_b);
}

/// Test that slug takes priority over a daemon with the *same* name in another namespace.
/// This is the canonical "slug beats same-name" scenario:
/// - proj-c has a daemon named "frontend" with slug "frontend-slug"
/// - proj-d has a daemon named "frontend" (no slug)
/// Querying "frontend-slug" must resolve to proj-c/frontend, not proj-d/frontend.
#[test]
fn test_slug_priority_over_same_name() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Project C: daemon named "frontend" with slug "frontend-slug"
    let project_c = env.project_dir().join("proj-c");
    fs::create_dir_all(&project_c).unwrap();
    fs::write(
        project_c.join("pitchfork.toml"),
        r#"
[daemons.frontend]
run = "sleep 60"
"#,
    )
    .unwrap();

    // Register slug "frontend-slug" → proj-c daemon "frontend"
    let _ = env.run_command_in_dir(
        &["proxy", "add", "frontend-slug", "--daemon", "frontend"],
        &project_c,
    );

    // Project D: daemon also named "frontend" (no slug)
    let project_d = env.project_dir().join("proj-d");
    fs::create_dir_all(&project_d).unwrap();
    fs::write(
        project_d.join("pitchfork.toml"),
        r#"
[daemons.frontend]
run = "sleep 60"
"#,
    )
    .unwrap();

    // Start both
    let _ = env.run_command_in_dir(&["start", "frontend"], &project_c);
    env.sleep(Duration::from_millis(500));
    let _ = env.run_command_in_dir(&["start", "frontend"], &project_d);
    env.sleep(Duration::from_secs(1));

    // Querying "frontend-slug" must resolve to proj-c/frontend via slug,
    // NOT to proj-d/frontend (which has no slug and a different name).
    let status_output = env.run_command_in_dir(&["status", "frontend-slug"], &project_d);
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    println!("same-name slug priority status: {status_str}");

    assert!(
        status_str.contains("running"),
        "Should find daemon by slug: {status_str}"
    );
    assert!(
        status_str.contains("proj-c"),
        "Slug match should resolve to proj-c/frontend: {status_str}"
    );
    assert!(
        !status_str.contains("proj-d"),
        "Must not resolve to proj-d/frontend: {status_str}"
    );

    // Cleanup
    let _ = env.run_command_in_dir(&["stop", "frontend"], &project_c);
    let _ = env.run_command_in_dir(&["stop", "frontend"], &project_d);
}

/// Test that logs command works with slug
#[test]
fn test_slug_logs() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("slug-logs");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("pitchfork.toml"),
        r#"
[daemons.myservice]
run = "echo 'slug log test' && sleep 30"
"#,
    )
    .unwrap();

    // Register slug in global config
    let _ = env.run_command_in_dir(&["proxy", "add", "svc", "--daemon", "myservice"], &project);

    // Start using the slug; later commands should resolve it the same way.
    let _ = env.run_command_in_dir(&["start", "svc"], &project);
    env.sleep(Duration::from_secs(2));

    // Get logs using slug
    let logs_output = env.run_command_in_dir(&["logs", "svc", "-n", "10"], &project);
    let logs_str = String::from_utf8_lossy(&logs_output.stdout);
    println!("slug logs: {logs_str}");
    assert!(
        logs_str.contains("slug log test"),
        "Logs via slug should work: {logs_str}"
    );

    let _ = env.run_command_in_dir(&["stop", "myservice"], &project);
}

/// Test that restart command works with slug
#[test]
fn test_slug_restart() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("slug-restart");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("pitchfork.toml"),
        r#"
[daemons.worker]
run = "sleep 60"
"#,
    )
    .unwrap();

    // Register slug
    let _ = env.run_command_in_dir(&["proxy", "add", "w", "--daemon", "worker"], &project);

    // Start using the slug; restart should keep working via the same slug.
    let _ = env.run_command_in_dir(&["start", "w"], &project);
    env.sleep(Duration::from_secs(1));

    // Restart using slug
    let restart_output = env.run_command_in_dir(&["restart", "w"], &project);
    println!(
        "restart stdout: {}",
        String::from_utf8_lossy(&restart_output.stdout)
    );
    assert!(
        restart_output.status.success(),
        "Restart via slug should succeed"
    );

    env.sleep(Duration::from_secs(1));

    let status_output = env.run_command_in_dir(&["status", "w"], &project);
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    assert!(
        status_str.contains("running"),
        "Daemon should be running after restart via slug: {status_str}"
    );

    let _ = env.run_command_in_dir(&["stop", "worker"], &project);
}

// ============================================================================
// Proxy URL Display Tests
// ============================================================================

/// Test that list command shows proxy URL when proxy is enabled
#[test]
fn test_list_shows_proxy_url() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("proxy-list");
    fs::create_dir_all(&project).unwrap();

    // Allocate a free port by binding to port 0 and reading the assigned port.
    // Keep the listener alive until *after* pitchfork.toml is written so that
    // the port cannot be stolen by another process between the bind and the
    // write.  The daemon will fail to bind if the port is still held, but
    // python3's http.server retries on EADDRINUSE, so in practice the tiny
    // window between drop and daemon-bind is safe enough for CI.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port: u16 = listener.local_addr().unwrap().port();
    fs::write(
        project.join("pitchfork.toml"),
        format!(
            r#"
[daemons.api]
run = "python3 -m http.server {port}"
expected_port = [{port}]
"#
        ),
    )
    .unwrap();
    // Drop the listener only after the config file is written, minimising the
    // TOCTOU window to the time between this drop and the daemon's bind().
    drop(listener);

    // Register slug
    let _ = env.run_command_in_dir(&["proxy", "add", "api"], &project);

    let _ = env.run_command_in_dir(&["start", "api"], &project);
    // Give the daemon time to start and bind the port
    env.sleep(Duration::from_secs(2));

    // Run list with proxy enabled
    let list_output = env.run_command_in_dir_with_env(
        &["list"],
        &project,
        &[
            ("PITCHFORK_PROXY_ENABLE", "true"),
            ("PITCHFORK_PROXY_TLD", "localhost"),
            ("PITCHFORK_PROXY_PORT", "7777"),
        ],
    );
    let list_str = String::from_utf8_lossy(&list_output.stdout);
    println!("list with proxy: {list_str}");

    // The proxy URL (e.g. http://api.proxy-list.localhost:7777) should appear
    // in the row content. We check for the TLD+port combination which is
    // unambiguous regardless of whether the table header is rendered (headers
    // are suppressed in non-TTY environments by comfy-table).
    assert!(
        list_str.contains("localhost:7777"),
        "List should show proxy URL in row when proxy is enabled: {list_str}"
    );

    let _ = env.run_command_in_dir(&["stop", "api"], &project);
    // Wait for the daemon to fully stop before killing the port to avoid a race
    // between the stop command and the port cleanup.
    env.sleep(Duration::from_secs(1));
    env.kill_port(port);
}

/// Test that status command shows proxy URL when proxy is enabled
#[test]
fn test_status_shows_proxy_url() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("proxy-status");
    fs::create_dir_all(&project).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port: u16 = listener.local_addr().unwrap().port();
    fs::write(
        project.join("pitchfork.toml"),
        format!(
            r#"
[daemons.server]
run = "python3 -m http.server {port}"
expected_port = [{port}]
"#
        ),
    )
    .unwrap();
    drop(listener);

    // Register slug
    let _ = env.run_command_in_dir(&["proxy", "add", "server"], &project);

    let _ = env.run_command_in_dir(&["start", "server"], &project);
    env.sleep(Duration::from_secs(2));

    // Run status with proxy enabled
    let status_output = env.run_command_in_dir_with_env(
        &["status", "server"],
        &project,
        &[
            ("PITCHFORK_PROXY_ENABLE", "true"),
            ("PITCHFORK_PROXY_TLD", "localhost"),
            ("PITCHFORK_PROXY_PORT", "7777"),
        ],
    );
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    println!("status with proxy: {status_str}");

    assert!(
        status_str.contains("Proxy:"),
        "Status should show Proxy: line when proxy is enabled: {status_str}"
    );
    assert!(
        status_str.contains("localhost:7777"),
        "Status should show proxy URL: {status_str}"
    );

    let _ = env.run_command_in_dir(&["stop", "server"], &project);
}

/// Test that start command shows proxy URL when proxy is enabled
#[test]
fn test_start_shows_proxy_url() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("proxy-start");
    fs::create_dir_all(&project).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port: u16 = listener.local_addr().unwrap().port();
    fs::write(
        project.join("pitchfork.toml"),
        format!(
            r#"
[daemons.app]
run = "python3 -m http.server {port}"
expected_port = [{port}]
"#
        ),
    )
    .unwrap();
    drop(listener);

    // Register slug
    let _ = env.run_command_in_dir(&["proxy", "add", "app"], &project);

    // Start with proxy enabled
    let start_output = env.run_command_in_dir_with_env(
        &["start", "app"],
        &project,
        &[
            ("PITCHFORK_PROXY_ENABLE", "true"),
            ("PITCHFORK_PROXY_TLD", "localhost"),
            ("PITCHFORK_PROXY_PORT", "7777"),
        ],
    );
    let start_str = String::from_utf8_lossy(&start_output.stdout);
    println!("start with proxy: {start_str}");

    assert!(
        start_str.contains("Proxy:") || start_str.contains("localhost:7777"),
        "Start should show proxy URL when proxy is enabled: {start_str}"
    );

    let _ = env.run_command_in_dir(&["stop", "app"], &project);
}

// ============================================================================
// Proxy Command Tests
// ============================================================================

/// Test that `pitchfork proxy status` works when proxy is disabled
#[test]
fn test_proxy_status_disabled() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    env.create_toml(
        r#"
[daemons.dummy]
run = "sleep 1"
"#,
    );

    let output =
        env.run_command_with_env(&["proxy", "status"], &[("PITCHFORK_PROXY_ENABLE", "false")]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("proxy status disabled: {stdout}");

    assert!(output.status.success(), "proxy status should succeed");
    assert!(
        stdout.contains("disabled"),
        "Should show 'disabled' when proxy is off: {stdout}"
    );
}

/// Test that `pitchfork proxy status` works when proxy is enabled
#[test]
fn test_proxy_status_enabled() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    env.create_toml(
        r#"
[daemons.dummy]
run = "sleep 1"
"#,
    );

    let output = env.run_command_with_env(
        &["proxy", "status"],
        &[
            ("PITCHFORK_PROXY_ENABLE", "true"),
            ("PITCHFORK_PROXY_TLD", "localhost"),
            ("PITCHFORK_PROXY_PORT", "7777"),
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("proxy status enabled: {stdout}");

    assert!(output.status.success(), "proxy status should succeed");
    assert!(
        stdout.contains("enabled"),
        "Should show 'enabled': {stdout}"
    );
    assert!(stdout.contains("localhost"), "Should show TLD: {stdout}");
    assert!(stdout.contains("7777"), "Should show port: {stdout}");
}

/// Test that `pitchfork proxy trust` fails gracefully when cert doesn't exist
#[test]
fn test_proxy_trust_missing_cert() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    env.create_toml(
        r#"
[daemons.dummy]
run = "sleep 1"
"#,
    );

    let output = env.run_command(&["proxy", "trust"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("proxy trust stderr: {stderr}");

    assert!(
        !output.status.success(),
        "proxy trust should fail when cert doesn't exist"
    );
    // miette renders errors differently in TTY vs non-TTY environments.
    // Check both stdout and stderr for the error message.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}{stdout}");
    assert!(
        combined.contains("not found")
            || combined.contains("certificate")
            || combined.contains("Certificate")
            || combined.contains("ca.pem"),
        "Should show helpful error about missing cert: stderr={stderr} stdout={stdout}"
    );
}

// ============================================================================
// Proxy URL Format Tests (unit-style via CLI)
// ============================================================================

/// Test proxy URL format for global namespace daemon (no slug = no proxy URL)
#[test]
fn test_proxy_url_global_namespace() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("global-proxy");
    fs::create_dir_all(&project).unwrap();
    // Use "global" namespace by not having a pitchfork.toml in parent
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port: u16 = listener.local_addr().unwrap().port();
    fs::write(
        project.join("pitchfork.toml"),
        format!(
            r#"
namespace = "global"

[daemons.myapi]
run = "python3 -m http.server {port}"
expected_port = [{port}]
"#
        ),
    )
    .unwrap();
    drop(listener);

    let _ = env.run_command_in_dir(&["start", "myapi"], &project);
    env.sleep(Duration::from_secs(2));

    let status_output = env.run_command_in_dir_with_env(
        &["status", "myapi"],
        &project,
        &[
            ("PITCHFORK_PROXY_ENABLE", "true"),
            ("PITCHFORK_PROXY_TLD", "localhost"),
            ("PITCHFORK_PROXY_PORT", "7777"),
        ],
    );
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    println!("global namespace proxy status (no slug): {status_str}");

    // No slug = no proxy URL. The status output should NOT contain a Proxy: line.
    assert!(
        !status_str.contains("Proxy:"),
        "Daemon without a slug should not have a proxy URL: {status_str}"
    );

    let _ = env.run_command_in_dir(&["stop", "myapi"], &project);
}

/// Test proxy URL format for local namespace daemon (no slug = no proxy URL)
#[test]
fn test_proxy_url_local_namespace() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let project = env.project_dir().join("myproject");
    fs::create_dir_all(&project).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port: u16 = listener.local_addr().unwrap().port();
    fs::write(
        project.join("pitchfork.toml"),
        format!(
            r#"
namespace = "myproject"

[daemons.api]
run = "python3 -m http.server {port}"
expected_port = [{port}]
"#
        ),
    )
    .unwrap();
    drop(listener);

    let _ = env.run_command_in_dir(&["start", "api"], &project);
    env.sleep(Duration::from_secs(2));

    let status_output = env.run_command_in_dir_with_env(
        &["status", "api"],
        &project,
        &[
            ("PITCHFORK_PROXY_ENABLE", "true"),
            ("PITCHFORK_PROXY_TLD", "localhost"),
            ("PITCHFORK_PROXY_PORT", "7777"),
        ],
    );
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    println!("local namespace proxy status (no slug): {status_str}");

    // No slug = no proxy URL. The status output should NOT contain a Proxy: line.
    assert!(
        !status_str.contains("Proxy:"),
        "Daemon without a slug should not have a proxy URL: {status_str}"
    );

    let _ = env.run_command_in_dir(&["stop", "api"], &project);
}
