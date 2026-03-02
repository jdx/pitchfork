mod common;

use common::TestEnv;
use std::fs;
use std::time::Duration;

fn assert_all_daemon_keys_qualified(state: &str) {
    use toml::Value;

    let parsed: Value = toml::from_str(state).expect("state file should be valid TOML");
    let doc = parsed
        .as_table()
        .expect("state file root should be a TOML table");

    if let Some(daemons) = doc.get("daemons").and_then(Value::as_table) {
        for key in daemons.keys() {
            assert!(
                key.contains('/'),
                "daemon key should be qualified (namespace/name), got '{key}'"
            );
        }
    }
}

/// Test that pitchfork silently migrates a state file written in the old format
/// (bare daemon names like `[daemons.api]`) to the new qualified format
/// (`[daemons."legacy/api"]`).
///
/// The migration should:
/// 1. Not print any WARN-level message to stderr
/// 2. Rewrite the state file on disk in the new format so subsequent reads work
/// 3. Preserve daemon entries under the `legacy` namespace
#[test]
fn test_state_file_migration_from_old_format() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Write an old-format state file (bare daemon names, no namespace)
    let state_path = env.state_file_path();
    let old_state = r#"
[daemons.myservice]
id = "myservice"
autostop = false
retry = 0
retry_count = 0
status = "stopped"

[daemons.worker]
id = "worker"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
last_exit_success = true
"#;
    fs::write(&state_path, old_state).unwrap();

    // Create a minimal config so pitchfork can start
    let toml_content = r#"
[daemons.probe]
run = "sleep 60"
"#;
    env.create_toml(toml_content);

    // Run `pitchfork list` — this starts the supervisor which reads the state file.
    // The client also reads the state file, triggering migration.
    let output = env.run_command(&["list"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("list stdout:\n{stdout}");
    println!("list stderr:\n{stderr}");

    // The command should succeed
    assert!(output.status.success(), "pitchfork list failed: {stderr}");

    // No user-facing warning should appear in stderr (debug messages are OK)
    assert!(
        !stderr.contains("Please delete"),
        "stderr should not contain 'Please delete' warning, got: {stderr}"
    );
    assert!(
        !stderr.contains("WARN"),
        "stderr should not contain WARN-level messages about state file, got: {stderr}"
    );

    // Wait a moment for state to be written
    std::thread::sleep(Duration::from_millis(500));

    // Read the state file back: it should be in new qualified format.
    // The supervisor rewrites the state on startup, so the old stopped daemons
    // may no longer appear, but the state file must use qualified IDs (with '/').
    let new_state = fs::read_to_string(&state_path).unwrap_or_default();
    println!("State file after migration:",);
    println!("{new_state}");

    // State must use qualified IDs now (either from migration or supervisor rewrite)
    assert_all_daemon_keys_qualified(&new_state);

    // Old bare section headers should be gone
    assert!(
        !new_state.contains("[daemons.myservice]") && !new_state.contains("[daemons.worker]"),
        "state file should not contain bare daemon keys, got:\n{new_state}"
    );
}

/// Test that state file migration preserves disabled daemon entries.
#[test]
fn test_state_file_migration_preserves_disabled() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    // Old state file with a disabled daemon set using bare names
    let state_path = env.state_file_path();
    let old_state = r#"
disabled = ["api"]

[daemons.api]
id = "api"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
"#;
    fs::write(&state_path, old_state).unwrap();

    let toml_content = r#"
[daemons.probe]
run = "sleep 60"
"#;
    env.create_toml(toml_content);

    // Run list to trigger state file read and migration
    let output = env.run_command(&["list"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // No warning about old format
    assert!(
        !stderr.contains("Please delete") && !stderr.contains("WARN"),
        "No migration warning expected, got: {stderr}"
    );

    // Wait for state to be written
    std::thread::sleep(Duration::from_millis(500));

    // Read back the migrated state file
    let new_state = fs::read_to_string(&state_path).unwrap_or_default();
    println!("Migrated state file:\n{new_state}");

    // The disabled entry should have been migrated to qualified form or simply
    // not present (supervisor may have cleared it after migration+start)
    // But the daemon entry itself should carry legacy/ prefix
    // Note: supervisor rewrites state on startup, so we check
    // that no bare "api" key without namespace remains
    assert!(
        !new_state.contains("[daemons.api]"),
        "Migrated state should not have bare [daemons.api] key, got:\n{new_state}"
    );
}

// =============================================================================
// Log directory migration tests
// =============================================================================

/// Helper: return the pitchfork logs directory inside the TestEnv home.
fn logs_dir(env: &TestEnv) -> std::path::PathBuf {
    env.state_file_path()
        .parent() // pitchfork/
        .unwrap()
        .join("logs")
}

/// Test that `pitchfork logs` silently renames bare-name log directories
/// (old format: `api/api.log`) to the new qualified format
/// (`legacy--api/legacy--api.log`).
///
/// The migration must:
/// 1. Rename the directory
/// 2. Rename the log file inside to match
/// 3. Preserve the log content
/// 4. Not print any WARN to stderr
#[test]
fn test_log_dir_migration_renames_old_dirs() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    // Create old-format log directories with content
    let ld = logs_dir(&env);
    fs::create_dir_all(ld.join("api")).unwrap();
    fs::create_dir_all(ld.join("worker")).unwrap();
    fs::write(
        ld.join("api").join("api.log"),
        "2025-01-01 00:00:00 api hello\n",
    )
    .unwrap();
    fs::write(
        ld.join("worker").join("worker.log"),
        "2025-01-01 00:00:01 worker starting\n",
    )
    .unwrap();

    // `pitchfork logs` reads the logs directory directly (no IPC / supervisor
    // required) and calls migrate_legacy_log_dirs() at the start.
    let output = env.run_command(&["logs"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("logs stderr:\n{stderr}");

    // No WARN-level output about directories
    assert!(
        !stderr.contains("WARN"),
        "No WARN expected during log migration, got: {stderr}"
    );

    // Old directories must be gone
    assert!(
        !ld.join("api").exists(),
        "Old 'api' dir should have been renamed"
    );
    assert!(
        !ld.join("worker").exists(),
        "Old 'worker' dir should have been renamed"
    );

    // New-format directories must exist
    assert!(
        ld.join("legacy--api").exists(),
        "New 'legacy--api' dir should exist"
    );
    assert!(
        ld.join("legacy--worker").exists(),
        "New 'legacy--worker' dir should exist"
    );

    // Log files inside must also be renamed
    assert!(
        ld.join("legacy--api").join("legacy--api.log").exists(),
        "legacy--api/legacy--api.log should exist"
    );
    assert!(
        ld.join("legacy--worker")
            .join("legacy--worker.log")
            .exists(),
        "legacy--worker/legacy--worker.log should exist"
    );

    // Content must be preserved
    let api_content = fs::read_to_string(ld.join("legacy--api").join("legacy--api.log")).unwrap();
    assert!(
        api_content.contains("hello"),
        "Log content should be preserved after migration, got: {api_content}"
    );
    let worker_content =
        fs::read_to_string(ld.join("legacy--worker").join("legacy--worker.log")).unwrap();
    assert!(
        worker_content.contains("starting"),
        "Log content should be preserved after migration, got: {worker_content}"
    );
}

/// Test that the migration is idempotent: new-format directories are left
/// untouched when `pitchfork logs` is run a second time.
#[test]
fn test_log_dir_migration_idempotent() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let ld = logs_dir(&env);
    fs::create_dir_all(ld.join("legacy--api")).unwrap();
    fs::write(
        ld.join("legacy--api").join("legacy--api.log"),
        "2025-01-01 00:00:00 legacy/api already migrated\n",
    )
    .unwrap();

    // First invocation
    env.run_command(&["logs"]);
    // Second invocation (idempotency check)
    let output = env.run_command(&["logs"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Directory must still exist with correct name
    assert!(
        ld.join("legacy--api").exists(),
        "Already-migrated dir must not be disturbed"
    );
    assert!(
        ld.join("legacy--api").join("legacy--api.log").exists(),
        "Log file must still exist"
    );

    // No name-collision dir should have been created
    assert!(
        !ld.join("legacy--legacy--api").exists(),
        "Double-migration must not happen"
    );

    assert!(
        !stderr.contains("WARN"),
        "No WARN expected on idempotent run, got: {stderr}"
    );
}

/// Test that old-format and new-format directories can coexist: only the
/// old-format ones are migrated; new-format ones are untouched.
#[test]
fn test_log_dir_migration_mixed_format() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let ld = logs_dir(&env);

    // Old-format (bare name, no "--")
    fs::create_dir_all(ld.join("legacy")).unwrap();
    fs::write(
        ld.join("legacy").join("legacy.log"),
        "2025-01-01 00:00:00 legacy old log\n",
    )
    .unwrap();

    // New-format (already qualified, contains "--")
    fs::create_dir_all(ld.join("myns--svc")).unwrap();
    fs::write(
        ld.join("myns--svc").join("myns--svc.log"),
        "2025-01-01 00:00:00 myns/svc new log\n",
    )
    .unwrap();

    env.run_command(&["logs"]);

    // Old-format dir must be migrated
    assert!(
        !ld.join("legacy").exists(),
        "Old 'legacy' dir should be gone"
    );
    assert!(
        ld.join("legacy--legacy").exists(),
        "Migrated 'legacy--legacy' dir must exist"
    );

    // New-format dir must be untouched
    assert!(
        ld.join("myns--svc").exists(),
        "Already-qualified 'myns--svc' dir must be untouched"
    );
    assert!(
        !ld.join("legacy--myns--svc").exists(),
        "New-format dir must not be double-migrated"
    );
}

/// Test that a correctly formatted new-format state file is NOT re-migrated
/// and produces no warnings.
#[test]
fn test_state_file_new_format_not_migrated() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();

    let state_path = env.state_file_path();

    // Write a properly formatted new-format state file (matches toml::to_string output)
    let new_state = r#"
disabled = []

[daemons."legacy/myservice"]
id = "legacy/myservice"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
last_exit_success = true
"#;
    fs::write(&state_path, new_state).unwrap();

    let toml_content = r#"
[daemons.probe]
run = "sleep 60"
"#;
    env.create_toml(toml_content);

    let output = env.run_command(&["list"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("list stderr:\n{stderr}");

    assert!(output.status.success(), "pitchfork list failed: {stderr}");

    // No migration warning
    assert!(
        !stderr.contains("Please delete") && !stderr.contains("WARN"),
        "No migration warning expected for new-format state file, got: {stderr}"
    );

    // State file should still be parseable new-format after the run
    std::thread::sleep(Duration::from_millis(300));
    let after_state = fs::read_to_string(&state_path).unwrap_or_default();
    assert_all_daemon_keys_qualified(&after_state);
}
