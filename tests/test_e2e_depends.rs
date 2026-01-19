mod common;

use common::TestEnv;
use std::time::Duration;

/// Test that starting a daemon auto-starts its dependencies
#[test]
fn test_start_with_dependency() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.db]
run = "echo 'db ready' && sleep 30"
ready_delay = 1

[daemons.api]
run = "echo 'api ready' && sleep 30"
depends = ["db"]
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Start only api - db should be started automatically as a dependency
    let output = env.run_command(&["start", "api"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("stdout: {}", stdout);
    println!("stderr: {}", stderr);

    assert!(
        output.status.success(),
        "Start command should succeed when starting api (with dependency on db)"
    );

    // Check that both daemons are running
    let list_output = env.run_command(&["list"]);
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    println!("List output: {}", list_stdout);

    assert!(list_stdout.contains("db"), "db daemon should be running");
    assert!(list_stdout.contains("api"), "api daemon should be running");

    // Verify logs show both started
    let db_logs = env.read_logs("db");
    let api_logs = env.read_logs("api");
    assert!(
        db_logs.contains("db ready"),
        "db logs should contain ready message"
    );
    assert!(
        api_logs.contains("api ready"),
        "api logs should contain ready message"
    );

    // Clean up
    env.run_command(&["stop", "--all"]);
}

/// Test that dependencies are started in correct order
#[test]
fn test_dependency_start_order() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Create a chain: api -> backend -> database
    let toml_content = r#"
[daemons.database]
run = "echo 'database started' && sleep 30"
ready_delay = 1

[daemons.backend]
run = "echo 'backend started' && sleep 30"
depends = ["database"]
ready_delay = 1

[daemons.api]
run = "echo 'api started' && sleep 30"
depends = ["backend"]
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Start api - should start database, then backend, then api
    let output = env.run_command(&["start", "api"]);

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success(), "Start command should succeed");

    // All three should be running
    let list_output = env.run_command(&["list"]);
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);

    assert!(
        list_stdout.contains("database"),
        "database daemon should be running"
    );
    assert!(
        list_stdout.contains("backend"),
        "backend daemon should be running"
    );
    assert!(list_stdout.contains("api"), "api daemon should be running");

    // Verify logs exist for all (confirms they were started)
    let db_logs = env.read_logs("database");
    let backend_logs = env.read_logs("backend");
    let api_logs = env.read_logs("api");

    assert!(
        db_logs.contains("database started"),
        "database logs should exist"
    );
    assert!(
        backend_logs.contains("backend started"),
        "backend logs should exist"
    );
    assert!(api_logs.contains("api started"), "api logs should exist");

    // Clean up
    env.run_command(&["stop", "--all"]);
}

/// Test that starting --all respects dependency order
#[test]
fn test_start_all_with_dependencies() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.db]
run = "echo 'db started' && sleep 30"
ready_delay = 1

[daemons.cache]
run = "echo 'cache started' && sleep 30"
ready_delay = 1

[daemons.api]
run = "echo 'api started' && sleep 30"
depends = ["db", "cache"]
ready_delay = 1

[daemons.worker]
run = "echo 'worker started' && sleep 30"
depends = ["db"]
ready_delay = 1
"#;
    env.create_toml(toml_content);

    let output = env.run_command(&["start", "--all"]);
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success(), "Start --all should succeed");

    // All daemons should be running
    let list_output = env.run_command(&["list"]);
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);

    assert!(list_stdout.contains("db"), "db should be running");
    assert!(list_stdout.contains("cache"), "cache should be running");
    assert!(list_stdout.contains("api"), "api should be running");
    assert!(list_stdout.contains("worker"), "worker should be running");

    // Clean up
    env.run_command(&["stop", "--all"]);
}

/// Test that already running dependencies are skipped
#[test]
fn test_skip_running_dependency() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.db]
run = "echo 'db ready' && sleep 30"
ready_delay = 1

[daemons.api]
run = "echo 'api ready' && sleep 30"
depends = ["db"]
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // First start db
    let output = env.run_command(&["start", "db"]);
    assert!(output.status.success(), "Start db should succeed");

    // Get db status to verify it's running
    let list_output = env.run_command(&["list"]);
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(list_stdout.contains("db"), "db should be running");

    // Now start api - db should NOT be restarted
    let start_time = std::time::Instant::now();
    let output = env.run_command(&["start", "api"]);
    let elapsed = start_time.elapsed();

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    println!("elapsed: {:?}", elapsed);

    assert!(output.status.success(), "Start api should succeed");

    // Should be faster than starting both (db was skipped)
    // Only api needed to start (1s ready_delay)
    assert!(
        elapsed < Duration::from_secs(3),
        "Should skip already running db, took {:?}",
        elapsed
    );

    // Clean up
    env.run_command(&["stop", "--all"]);
}

/// Test circular dependency detection
#[test]
fn test_circular_dependency_error() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.a]
run = "echo 'a'"
depends = ["c"]

[daemons.b]
run = "echo 'b'"
depends = ["a"]

[daemons.c]
run = "echo 'c'"
depends = ["b"]
"#;
    env.create_toml(toml_content);

    let output = env.run_command(&["start", "a"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("stderr: {}", stderr);

    assert!(
        !output.status.success(),
        "Start should fail with circular dependency"
    );
    assert!(
        stderr.to_lowercase().contains("circular"),
        "Error should mention circular dependency"
    );
}

/// Test missing dependency error
#[test]
fn test_missing_dependency_error() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let toml_content = r#"
[daemons.api]
run = "echo 'api'"
depends = ["nonexistent"]
"#;
    env.create_toml(toml_content);

    let output = env.run_command(&["start", "api"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("stderr: {}", stderr);

    assert!(
        !output.status.success(),
        "Start should fail with missing dependency"
    );
    assert!(
        stderr.contains("nonexistent"),
        "Error should mention missing daemon name"
    );
}

/// Test that force flag only affects explicitly requested daemon
#[test]
fn test_force_only_affects_requested() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Use bash -c to wrap compound commands so exec works correctly
    let toml_content = r#"
[daemons.db]
run = "bash -c 'echo db_started; sleep 60'"
ready_delay = 1

[daemons.api]
run = "bash -c 'echo api_started; sleep 60'"
depends = ["db"]
ready_delay = 1
"#;
    env.create_toml(toml_content);

    // Start api (which starts db as dependency)
    let output = env.run_command(&["start", "api"]);
    assert!(output.status.success(), "Initial start should succeed");

    // Wait for daemons to be fully registered as running
    env.sleep(Duration::from_secs(2));

    // Verify both are running
    let list_output = env.run_command(&["list"]);
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    println!("List between starts: {}", list_stdout);
    assert!(list_stdout.contains("running"), "Daemons should be running");

    // Get db's log content before force restart
    let db_logs_before = env.read_logs("db");
    println!("db logs before: {}", db_logs_before);
    let db_started_count_before = db_logs_before.matches("db_started").count();

    // Force restart api - db should NOT be restarted
    let output = env.run_command(&["start", "-f", "api"]);
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "Force restart should succeed");

    env.sleep(Duration::from_millis(500));

    // Check db logs - should still only have same count (not restarted)
    let db_logs_after = env.read_logs("db");
    println!("db logs after: {}", db_logs_after);

    let db_started_count_after = db_logs_after.matches("db_started").count();
    assert_eq!(
        db_started_count_before, db_started_count_after,
        "db should not have been restarted (force only affects api)"
    );

    // api should have been restarted (two "api_started" lines)
    let api_logs = env.read_logs("api");
    println!("api logs: {}", api_logs);
    let api_started_count = api_logs.matches("api_started").count();
    assert_eq!(api_started_count, 2, "api should have been restarted");

    // Clean up
    env.run_command(&["stop", "--all"]);
}
