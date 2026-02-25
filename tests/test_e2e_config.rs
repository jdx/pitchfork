mod common;

use common::TestEnv;
use std::fs;

/// Test basic config add with positional arguments
#[test]
fn test_config_add_basic() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir(); // Create the project directory first

    let output = env.run_command(&["config", "add", "api", "bun", "run", "server/index.ts"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("stdout: {}", stdout);
    println!("stderr: {}", stderr);

    assert!(output.status.success(), "config add should succeed");
    assert!(
        stdout.contains("added api"),
        "output should confirm daemon was added"
    );

    // Verify the TOML file was created correctly
    let toml_path = env.project_dir().join("pitchfork.toml");
    assert!(toml_path.exists(), "pitchfork.toml should exist");

    let toml_content = fs::read_to_string(&toml_path).unwrap();
    println!("Generated TOML:\n{}", toml_content);

    // Parse and verify the daemon configuration
    let pt = pitchfork_cli::pitchfork_toml::PitchforkToml::read(&toml_path).unwrap();
    assert_eq!(pt.daemons.len(), 1);

    let api = pt.daemons.get("api").unwrap();
    assert_eq!(api.run, "bun run server/index.ts");
    assert_eq!(api.retry.count(), 0); // default value
    assert!(api.watch.is_empty());
}

/// Test config add with --run flag
#[test]
fn test_config_add_with_run_flag() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&["config", "add", "worker", "--run", "npm run worker"]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let toml_content = fs::read_to_string(&toml_path).unwrap();

    // Verify the generated TOML
    assert!(toml_content.contains("run = \"npm run worker\""));
}

/// Test config add with retry option
#[test]
fn test_config_add_with_retry() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&[
        "config",
        "add",
        "api",
        "--run",
        "bun run server/index.ts",
        "--retry",
        "3",
    ]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let toml_content = fs::read_to_string(&toml_path).unwrap();

    // Verify retry is set correctly
    assert!(toml_content.contains("retry = 3"));
    // Verify the run command is properly formatted (not embedded with CLI flags)
    // The run value should be exactly "bun run server/index.ts", not something like:
    // run = "--cmd 'bun run server/index.ts' --retry 3"
    assert!(toml_content.contains("run = \"bun run server/index.ts\""));
}

/// Test config add with watch patterns
#[test]
fn test_config_add_with_watch() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&[
        "config",
        "add",
        "api",
        "--run",
        "bun run server",
        "--watch",
        "server/**/*.ts",
        "--watch",
        "server/**/*.sql",
    ]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let pt = pitchfork_cli::pitchfork_toml::PitchforkToml::read(&toml_path).unwrap();

    let api = pt.daemons.get("api").unwrap();
    assert_eq!(api.watch.len(), 2);
    assert!(api.watch.contains(&"server/**/*.ts".to_string()));
    assert!(api.watch.contains(&"server/**/*.sql".to_string()));
}

/// Test config add with autostart and autostop
#[test]
fn test_config_add_with_auto_flags() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&[
        "config",
        "add",
        "api",
        "--run",
        "npm start",
        "--autostart",
        "--autostop",
    ]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let toml_content = fs::read_to_string(&toml_path).unwrap();

    assert!(
        toml_content.contains("auto = [\"start\", \"stop\"]")
            || toml_content.contains("auto = [\"stop\", \"start\"]")
            || toml_content.contains("auto = [\"start\"") && toml_content.contains("\"stop\"]")
    );
}

/// Test config add with environment variables
#[test]
fn test_config_add_with_env() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&[
        "config",
        "add",
        "api",
        "--run",
        "npm start",
        "--env",
        "NODE_ENV=development",
        "--env",
        "PORT=3000",
    ]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let toml_content = fs::read_to_string(&toml_path).unwrap();

    assert!(toml_content.contains("NODE_ENV = \"development\""));
    assert!(toml_content.contains("PORT = \"3000\""));
}

/// Test config add with ready checks
#[test]
fn test_config_add_with_ready_checks() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&[
        "config",
        "add",
        "api",
        "--run",
        "npm start",
        "--ready-delay",
        "5",
        "--ready-output",
        "Server ready",
        "--ready-http",
        "http://localhost:3000/health",
        "--ready-port",
        "3000",
    ]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let toml_content = fs::read_to_string(&toml_path).unwrap();

    assert!(toml_content.contains("ready_delay = 5"));
    assert!(toml_content.contains("ready_output = \"Server ready\""));
    assert!(toml_content.contains("ready_http = \"http://localhost:3000/health\""));
    assert!(toml_content.contains("ready_port = 3000"));
}

/// Test config add with dependencies
#[test]
fn test_config_add_with_depends() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&[
        "config",
        "add",
        "api",
        "--run",
        "npm start",
        "--depends",
        "postgres",
        "--depends",
        "redis",
    ]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let pt = pitchfork_cli::pitchfork_toml::PitchforkToml::read(&toml_path).unwrap();

    let api = pt.daemons.get("api").unwrap();
    assert_eq!(api.depends.len(), 2);
    assert!(api.depends.contains(&"postgres".to_string()));
    assert!(api.depends.contains(&"redis".to_string()));
}

/// Test config add with hooks
#[test]
fn test_config_add_with_hooks() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&[
        "config",
        "add",
        "api",
        "--run",
        "npm start",
        "--on-ready",
        "curl -X POST http://localhost:3000/ready",
        "--on-fail",
        "./scripts/alert.sh",
        "--on-retry",
        "echo 'retrying'",
    ]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let toml_content = fs::read_to_string(&toml_path).unwrap();

    // Check that hooks section exists
    assert!(
        toml_content.contains("[daemons.api.hooks]")
            || toml_content.contains("on_ready")
            || toml_content.contains("on_fail")
    );
}

/// Test config add with cron schedule
#[test]
fn test_config_add_with_cron() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&[
        "config",
        "add",
        "backup",
        "--run",
        "./scripts/backup.sh",
        "--cron-schedule",
        "0 0 2 * * *",
        "--cron-retrigger",
        "always",
    ]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let toml_content = fs::read_to_string(&toml_path).unwrap();

    assert!(toml_content.contains("schedule = \"0 0 2 * * *\""));
    assert!(toml_content.contains("retrigger = \"always\""));
}

/// Test config add with all options combined
#[test]
fn test_config_add_complete() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&[
        "config",
        "add",
        "api",
        "--run",
        "bun run server/index.ts",
        "--retry",
        "3",
        "--watch",
        "server/**/*.ts",
        "--watch",
        "package.json",
        "--dir",
        "./api",
        "--env",
        "NODE_ENV=development",
        "--env",
        "PORT=3000",
        "--ready-delay",
        "3",
        "--ready-output",
        "Listening on",
        "--depends",
        "database",
        "--autostart",
        "--autostop",
        "--on-ready",
        "echo 'API is ready'",
    ]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let pt = pitchfork_cli::pitchfork_toml::PitchforkToml::read(&toml_path).unwrap();

    let api = pt.daemons.get("api").unwrap();
    assert_eq!(api.run, "bun run server/index.ts");
    assert_eq!(api.retry.count(), 3);
    assert_eq!(api.watch.len(), 2);
    assert_eq!(api.dir, Some("./api".to_string()));
    assert!(api.env.is_some());
    assert_eq!(api.ready_delay, Some(3));
    assert_eq!(api.ready_output, Some("Listening on".to_string()));
    assert_eq!(api.depends.len(), 1);
    assert_eq!(api.depends[0], "database");
    assert_eq!(api.auto.len(), 2);
    assert!(api.hooks.is_some());

    // Verify the generated TOML can be parsed correctly
    let toml_content = fs::read_to_string(&toml_path).unwrap();
    println!("Complete generated TOML:\n{}", toml_content);

    // Ensure the run command doesn't contain embedded CLI flags
    assert!(!toml_content.contains("run = \"--"));
    assert!(toml_content.contains("run = \"bun run server/index.ts\""));
}

/// Test that config add fails without run command or args
#[test]
fn test_config_add_fails_without_run() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    let output = env.run_command(&["config", "add", "api"]);

    assert!(
        !output.status.success(),
        "config add should fail without run command"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Either --run or command arguments must be provided")
            || stderr.contains("invalid input"),
        "Error message should indicate missing run command"
    );
}

/// Test config add to existing file preserves other daemons
#[test]
fn test_config_add_preserves_existing_daemons() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    // First add a daemon
    let _ = env.run_command(&["config", "add", "postgres", "--run", "postgres -D data"]);

    // Then add another daemon
    let output = env.run_command(&["config", "add", "api", "--run", "npm start"]);

    assert!(output.status.success(), "config add should succeed");

    let toml_path = env.project_dir().join("pitchfork.toml");
    let pt = pitchfork_cli::pitchfork_toml::PitchforkToml::read(&toml_path).unwrap();

    // Both daemons should exist
    assert_eq!(pt.daemons.len(), 2);
    assert!(pt.daemons.contains_key("postgres"));
    assert!(pt.daemons.contains_key("api"));

    // Verify the first daemon is preserved
    let postgres = pt.daemons.get("postgres").unwrap();
    assert_eq!(postgres.run, "postgres -D data");
}

/// Test that generated config can be used to start a daemon
#[test]
fn test_config_add_generates_valid_config() {
    let env = TestEnv::new();
    env.ensure_binary_exists().unwrap();
    env.create_project_dir();

    // Create a simple script that outputs "ready"
    let script_path = env.project_dir().join("server.sh");
    fs::write(&script_path, "#!/bin/bash\necho 'ready'\nsleep 30").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
    }

    // Add the daemon using config add
    let output = env.run_command(&[
        "config",
        "add",
        "test-server",
        "--run",
        &format!("{}", script_path.display()),
        "--ready-output",
        "ready",
        "--retry",
        "0",
    ]);

    assert!(output.status.success(), "config add should succeed");

    // Now try to start the daemon
    let output = env.run_command(&["start", "test-server"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("start stdout: {}", stdout);
    println!("start stderr: {}", stderr);

    // The daemon should start successfully (or fail with a clear error, but not a parse error)
    // If it fails, it should be because the daemon exited, not because of config parsing
    if !output.status.success() {
        // If it failed, make sure it's not a parsing error
        assert!(
            !stderr.contains("invalid option"),
            "Should not have shell parsing errors"
        );
        assert!(
            !stderr.contains("usage: exec"),
            "Should not have exec usage errors"
        );
    }

    // Clean up
    let _ = env.run_command(&["stop", "test-server"]);
}
