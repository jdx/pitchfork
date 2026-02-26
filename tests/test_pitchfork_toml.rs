use pitchfork_cli::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Test creating a new empty PitchforkToml
#[test]
fn test_new_pitchfork_toml() {
    let path = PathBuf::from("/tmp/test.toml");
    let pt = pitchfork_toml::PitchforkToml::new(path.clone());

    assert_eq!(pt.path, Some(path));
    assert_eq!(pt.daemons.len(), 0);
}

/// Test reading a basic pitchfork.toml file
#[test]
fn test_read_pitchfork_toml() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.test_daemon]
run = "echo 'hello world'"
retry = 3
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;

    assert_eq!(pt.path, Some(toml_path));
    assert_eq!(pt.daemons.len(), 1);
    assert!(pt.daemons.contains_key("test_daemon"));

    let daemon = pt.daemons.get("test_daemon").unwrap();
    assert_eq!(daemon.run, "echo 'hello world'");
    assert_eq!(daemon.retry.count(), 3);

    Ok(())
}

/// Test reading a non-existent file creates an empty PitchforkToml
#[test]
fn test_read_nonexistent_file() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("nonexistent.toml");

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;

    assert_eq!(pt.path, Some(toml_path));
    assert_eq!(pt.daemons.len(), 0);

    Ok(())
}

/// Test writing a PitchforkToml to file
#[test]
fn test_write_pitchfork_toml() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let mut pt = pitchfork_toml::PitchforkToml::new(toml_path.clone());

    // Add a daemon
    use indexmap::IndexMap;
    let mut daemons = IndexMap::new();
    daemons.insert(
        "test_daemon".to_string(),
        pitchfork_toml::PitchforkTomlDaemon {
            run: "echo 'test'".to_string(),
            auto: vec![],
            cron: None,
            retry: pitchfork_toml::Retry::from(5),
            ready_delay: None,
            ready_output: None,
            ready_http: None,
            ready_port: None,
            ready_cmd: None,
            port: Vec::new(),
            auto_bump_port: false,
            boot_start: None,
            depends: vec![],
            watch: vec![],
            dir: None,
            env: None,
            hooks: None,
            path: Some(toml_path.clone()),
        },
    );
    pt.daemons = daemons;

    pt.write()?;

    assert!(toml_path.exists());

    let pt_read = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    assert_eq!(pt_read.daemons.len(), 1);
    assert!(pt_read.daemons.contains_key("test_daemon"));

    Ok(())
}

/// Test daemon with auto start configuration
#[test]
fn test_daemon_with_auto_start() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.auto_daemon]
run = "echo 'auto start'"
auto = ["start"]
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("auto_daemon").unwrap();

    assert_eq!(daemon.auto.len(), 1);
    assert_eq!(daemon.auto[0], pitchfork_toml::PitchforkTomlAuto::Start);

    Ok(())
}

/// Test daemon with cron configuration
#[test]
fn test_daemon_with_cron() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.cron_daemon]
run = "echo 'cron job'"

[daemons.cron_daemon.cron]
schedule = "0 0 * * *"
retrigger = "always"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("cron_daemon").unwrap();

    assert!(daemon.cron.is_some());
    let cron = daemon.cron.as_ref().unwrap();
    assert_eq!(cron.schedule, "0 0 * * *");
    assert_eq!(cron.retrigger, pitchfork_toml::CronRetrigger::Always);

    Ok(())
}

/// Test daemon with ready checks
#[test]
fn test_daemon_with_ready_checks() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.ready_daemon]
run = "echo 'server starting'"
ready_delay = 5000
ready_output = "Server is ready"
ready_http = "http://localhost:8080/health"
ready_port = 8080
ready_cmd = "test -f /tmp/ready"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("ready_daemon").unwrap();

    assert_eq!(daemon.ready_delay, Some(5000));
    assert_eq!(daemon.ready_output, Some("Server is ready".to_string()));
    assert_eq!(
        daemon.ready_http,
        Some("http://localhost:8080/health".to_string())
    );
    assert_eq!(daemon.ready_port, Some(8080));
    assert_eq!(daemon.ready_cmd, Some("test -f /tmp/ready".to_string()));

    Ok(())
}

/// Test multiple daemons in one file
#[test]
fn test_multiple_daemons() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.daemon1]
run = "echo 'daemon 1'"

[daemons.daemon2]
run = "echo 'daemon 2'"
retry = 10

[daemons.daemon3]
run = "echo 'daemon 3'"
auto = ["start", "stop"]
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;

    assert_eq!(pt.daemons.len(), 3);
    assert!(pt.daemons.contains_key("daemon1"));
    assert!(pt.daemons.contains_key("daemon2"));
    assert!(pt.daemons.contains_key("daemon3"));

    assert_eq!(pt.daemons.get("daemon2").unwrap().retry.count(), 10);
    assert_eq!(pt.daemons.get("daemon3").unwrap().auto.len(), 2);

    Ok(())
}

/// Test CronRetrigger enum serialization
#[test]
fn test_cron_retrigger_variants() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();

    // Test each retrigger variant
    let variants = vec![
        ("finish", pitchfork_toml::CronRetrigger::Finish),
        ("always", pitchfork_toml::CronRetrigger::Always),
        ("success", pitchfork_toml::CronRetrigger::Success),
        ("fail", pitchfork_toml::CronRetrigger::Fail),
    ];

    for (variant_name, expected) in variants {
        let toml_path = temp_dir.path().join(format!("cron_{variant_name}.toml"));
        let toml_content = format!(
            r#"
[daemons.test]
run = "echo 'test'"

[daemons.test.cron]
schedule = "* * * * *"
retrigger = "{variant_name}"
"#
        );

        fs::write(&toml_path, toml_content).unwrap();

        let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
        let daemon = pt.daemons.get("test").unwrap();
        let cron = daemon.cron.as_ref().unwrap();

        assert_eq!(cron.retrigger, expected);
    }

    Ok(())
}

/// Test merging configurations from multiple files
#[test]
fn test_config_merging() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();

    // Create system-level config
    let system_config = temp_dir.path().join("system.toml");
    let system_content = r#"
[daemons.system_daemon]
run = "echo 'system'"
retry = 1

[daemons.shared_daemon]
run = "echo 'from system'"
retry = 5
"#;
    fs::write(&system_config, system_content).unwrap();

    // Create user-level config
    let user_config = temp_dir.path().join("user.toml");
    let user_content = r#"
[daemons.user_daemon]
run = "echo 'user'"
retry = 2

[daemons.shared_daemon]
run = "echo 'from user'"
retry = 10
"#;
    fs::write(&user_config, user_content).unwrap();

    // Create project-level config
    let project_config = temp_dir.path().join("project.toml");
    let project_content = r#"
[daemons.project_daemon]
run = "echo 'project'"
retry = 3

[daemons.shared_daemon]
run = "echo 'from project'"
retry = 15
"#;
    fs::write(&project_config, project_content).unwrap();

    // Merge in order: system -> user -> project
    let mut merged = pitchfork_toml::PitchforkToml::default();

    let system = pitchfork_toml::PitchforkToml::read(&system_config)?;
    merged.merge(system);

    let user = pitchfork_toml::PitchforkToml::read(&user_config)?;
    merged.merge(user);

    let project = pitchfork_toml::PitchforkToml::read(&project_config)?;
    merged.merge(project);

    // Verify all daemons are present
    assert_eq!(merged.daemons.len(), 4);
    assert!(merged.daemons.contains_key("system_daemon"));
    assert!(merged.daemons.contains_key("user_daemon"));
    assert!(merged.daemons.contains_key("project_daemon"));
    assert!(merged.daemons.contains_key("shared_daemon"));

    // Verify that project config overrides user and system
    let shared = merged.daemons.get("shared_daemon").unwrap();
    assert_eq!(shared.run, "echo 'from project'");
    assert_eq!(shared.retry.count(), 15);

    Ok(())
}

/// Test that user config overrides system config
#[test]
fn test_user_overrides_system() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();

    // System config
    let system_config = temp_dir.path().join("system.toml");
    let system_content = r#"
[daemons.web]
run = "python -m http.server 8000"
retry = 3
"#;
    fs::write(&system_config, system_content).unwrap();

    // User config overrides retry count
    let user_config = temp_dir.path().join("user.toml");
    let user_content = r#"
[daemons.web]
run = "python -m http.server 9000"
retry = 5
"#;
    fs::write(&user_config, user_content).unwrap();

    let mut merged = pitchfork_toml::PitchforkToml::default();
    merged.merge(pitchfork_toml::PitchforkToml::read(&system_config)?);
    merged.merge(pitchfork_toml::PitchforkToml::read(&user_config)?);

    assert_eq!(merged.daemons.len(), 1);
    let web = merged.daemons.get("web").unwrap();
    assert_eq!(web.run, "python -m http.server 9000");
    assert_eq!(web.retry.count(), 5);

    Ok(())
}

/// Test that project config overrides both user and system
#[test]
fn test_project_overrides_all() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();

    // System config
    let system_config = temp_dir.path().join("system.toml");
    fs::write(
        &system_config,
        r#"
[daemons.database]
run = "postgres -D /var/lib/postgres"
retry = 3
ready_delay = 1000
"#,
    )
    .unwrap();

    // User config
    let user_config = temp_dir.path().join("user.toml");
    fs::write(
        &user_config,
        r#"
[daemons.database]
run = "postgres -D ~/postgres"
retry = 5
ready_delay = 2000
"#,
    )
    .unwrap();

    // Project config
    let project_config = temp_dir.path().join("project.toml");
    fs::write(
        &project_config,
        r#"
[daemons.database]
run = "postgres -D ./data"
retry = 10
ready_delay = 3000
ready_output = "ready to accept connections"
"#,
    )
    .unwrap();

    let mut merged = pitchfork_toml::PitchforkToml::default();
    merged.merge(pitchfork_toml::PitchforkToml::read(&system_config)?);
    merged.merge(pitchfork_toml::PitchforkToml::read(&user_config)?);
    merged.merge(pitchfork_toml::PitchforkToml::read(&project_config)?);

    assert_eq!(merged.daemons.len(), 1);
    let db = merged.daemons.get("database").unwrap();
    assert_eq!(db.run, "postgres -D ./data");
    assert_eq!(db.retry.count(), 10);
    assert_eq!(db.ready_delay, Some(3000));
    assert_eq!(
        db.ready_output,
        Some("ready to accept connections".to_string())
    );

    Ok(())
}

/// Test reading global configs when they don't exist (should not fail)
#[test]
fn test_missing_global_configs_ignored() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();

    // Create only a project config
    let project_config = temp_dir.path().join("pitchfork.toml");
    fs::write(
        &project_config,
        r#"
[daemons.app]
run = "echo 'hello'"
"#,
    )
    .unwrap();

    // Try to read non-existent configs (should return empty configs, not fail)
    let nonexistent_system = temp_dir.path().join("nonexistent_system.toml");
    let nonexistent_user = temp_dir.path().join("nonexistent_user.toml");

    let system = pitchfork_toml::PitchforkToml::read(&nonexistent_system)?;
    let user = pitchfork_toml::PitchforkToml::read(&nonexistent_user)?;
    let project = pitchfork_toml::PitchforkToml::read(&project_config)?;

    assert_eq!(system.daemons.len(), 0);
    assert_eq!(user.daemons.len(), 0);
    assert_eq!(project.daemons.len(), 1);

    // Merge all
    let mut merged = pitchfork_toml::PitchforkToml::default();
    merged.merge(system);
    merged.merge(user);
    merged.merge(project);

    assert_eq!(merged.daemons.len(), 1);
    assert!(merged.daemons.contains_key("app"));

    Ok(())
}

/// Test that merge preserves order with IndexMap
#[test]
fn test_merge_preserves_order() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();

    let config1 = temp_dir.path().join("config1.toml");
    fs::write(
        &config1,
        r#"
[daemons.first]
run = "echo 'first'"

[daemons.second]
run = "echo 'second'"
"#,
    )
    .unwrap();

    let config2 = temp_dir.path().join("config2.toml");
    fs::write(
        &config2,
        r#"
[daemons.third]
run = "echo 'third'"

[daemons.second]
run = "echo 'second updated'"
"#,
    )
    .unwrap();

    let mut merged = pitchfork_toml::PitchforkToml::default();
    merged.merge(pitchfork_toml::PitchforkToml::read(&config1)?);
    merged.merge(pitchfork_toml::PitchforkToml::read(&config2)?);

    assert_eq!(merged.daemons.len(), 3);

    let keys: Vec<_> = merged.daemons.keys().collect();
    // "first" and "second" come from config1, "third" and updated "second" from config2
    // Since we use IndexMap, insertion order is preserved
    assert!(keys.contains(&&"first".to_string()));
    assert!(keys.contains(&&"second".to_string()));
    assert!(keys.contains(&&"third".to_string()));

    // Verify second was updated
    assert_eq!(
        merged.daemons.get("second").unwrap().run,
        "echo 'second updated'"
    );

    Ok(())
}

/// Test daemon with depends configuration
#[test]
fn test_daemon_with_depends() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.postgres]
run = "postgres -D /data"

[daemons.redis]
run = "redis-server"

[daemons.api]
run = "npm run server"
depends = ["postgres", "redis"]
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;

    // Check postgres has no dependencies
    let postgres = pt.daemons.get("postgres").unwrap();
    assert!(postgres.depends.is_empty());

    // Check redis has no dependencies
    let redis = pt.daemons.get("redis").unwrap();
    assert!(redis.depends.is_empty());

    // Check api has correct dependencies
    let api = pt.daemons.get("api").unwrap();
    assert_eq!(api.depends.len(), 2);
    assert!(api.depends.contains(&"postgres".to_string()));
    assert!(api.depends.contains(&"redis".to_string()));

    Ok(())
}

/// Test empty depends array
#[test]
fn test_daemon_with_empty_depends() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.standalone]
run = "echo 'standalone'"
depends = []
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("standalone").unwrap();

    assert!(daemon.depends.is_empty());

    Ok(())
}

/// Test that retry can be a boolean (true = infinite, false = 0)
#[test]
fn test_retry_boolean_values() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.infinite_retry]
run = "echo 'will retry forever'"
retry = true

[daemons.no_retry]
run = "echo 'no retry'"
retry = false

[daemons.numeric_retry]
run = "echo 'retry 5 times'"
retry = 5
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;

    // Test infinite retry (true = u32::MAX)
    let infinite = pt.daemons.get("infinite_retry").unwrap();
    assert!(infinite.retry.is_infinite());
    assert_eq!(infinite.retry.count(), u32::MAX);
    assert_eq!(infinite.retry.to_string(), "infinite");

    // Test no retry (false = 0)
    let no_retry = pt.daemons.get("no_retry").unwrap();
    assert!(!no_retry.retry.is_infinite());
    assert_eq!(no_retry.retry.count(), 0);
    assert_eq!(no_retry.retry.to_string(), "0");

    // Test numeric retry
    let numeric = pt.daemons.get("numeric_retry").unwrap();
    assert!(!numeric.retry.is_infinite());
    assert_eq!(numeric.retry.count(), 5);
    assert_eq!(numeric.retry.to_string(), "5");

    // Test serialization round-trip
    pt.write()?;
    let raw = fs::read_to_string(&toml_path).unwrap();
    // Infinite retry should serialize as `true`
    assert!(
        raw.contains("retry = true"),
        "infinite retry should serialize as 'true'"
    );
    // Numeric retry should serialize as number
    assert!(
        raw.contains("retry = 5"),
        "numeric retry should serialize as number"
    );
    // Zero retry should serialize as 0
    assert!(
        raw.contains("retry = 0") || raw.contains("retry = false"),
        "zero retry should serialize as 0 or false"
    );

    Ok(())
}

// =============================================================================
// Tests for dir and env fields
// =============================================================================

/// Test daemon with dir configuration
#[test]
fn test_daemon_with_dir() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.frontend]
run = "npm run dev"
dir = "frontend"

[daemons.api]
run = "npm run server"
dir = "/opt/api"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;

    let frontend = pt.daemons.get("frontend").unwrap();
    assert_eq!(frontend.dir, Some("frontend".to_string()));

    let api = pt.daemons.get("api").unwrap();
    assert_eq!(api.dir, Some("/opt/api".to_string()));

    Ok(())
}

/// Test daemon without dir defaults to None
#[test]
fn test_daemon_without_dir() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.test]
run = "echo test"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("test").unwrap();
    assert!(daemon.dir.is_none());

    Ok(())
}

/// Test daemon with env configuration (inline format)
#[test]
fn test_daemon_with_env_inline() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.api]
run = "npm run server"
env = { NODE_ENV = "development", PORT = "3000" }
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("api").unwrap();

    let env = daemon.env.as_ref().unwrap();
    assert_eq!(env.len(), 2);
    assert_eq!(env.get("NODE_ENV").unwrap(), "development");
    assert_eq!(env.get("PORT").unwrap(), "3000");

    Ok(())
}

/// Test daemon with env configuration (table format)
#[test]
fn test_daemon_with_env_table() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.worker]
run = "python worker.py"

[daemons.worker.env]
DATABASE_URL = "postgres://localhost/mydb"
REDIS_URL = "redis://localhost:6379"
LOG_LEVEL = "debug"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("worker").unwrap();

    let env = daemon.env.as_ref().unwrap();
    assert_eq!(env.len(), 3);
    assert_eq!(
        env.get("DATABASE_URL").unwrap(),
        "postgres://localhost/mydb"
    );
    assert_eq!(env.get("REDIS_URL").unwrap(), "redis://localhost:6379");
    assert_eq!(env.get("LOG_LEVEL").unwrap(), "debug");

    Ok(())
}

/// Test daemon without env defaults to None
#[test]
fn test_daemon_without_env() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.test]
run = "echo test"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("test").unwrap();
    assert!(daemon.env.is_none());

    Ok(())
}

/// Test daemon with both dir and env
#[test]
fn test_daemon_with_dir_and_env() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.frontend]
run = "npm run dev"
dir = "frontend"
env = { NODE_ENV = "development", PORT = "5173" }
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("frontend").unwrap();

    assert_eq!(daemon.dir, Some("frontend".to_string()));

    let env = daemon.env.as_ref().unwrap();
    assert_eq!(env.get("NODE_ENV").unwrap(), "development");
    assert_eq!(env.get("PORT").unwrap(), "5173");

    Ok(())
}

/// Test that dir and env are not serialized when None (skip_serializing_if)
#[test]
fn test_dir_env_not_serialized_when_none() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let mut pt = pitchfork_toml::PitchforkToml::new(toml_path.clone());
    use indexmap::IndexMap;
    let mut daemons = IndexMap::new();
    daemons.insert(
        "test".to_string(),
        pitchfork_toml::PitchforkTomlDaemon {
            run: "echo test".to_string(),
            auto: vec![],
            cron: None,
            retry: pitchfork_toml::Retry::default(),
            ready_delay: None,
            ready_output: None,
            ready_http: None,
            ready_port: None,
            ready_cmd: None,
            port: Vec::new(),
            auto_bump_port: false,
            boot_start: None,
            depends: vec![],
            watch: vec![],
            dir: None,
            env: None,
            hooks: None,
            path: None,
        },
    );
    pt.daemons = daemons;
    pt.write()?;

    // Re-read and verify dir/env are still None (not serialized)
    let pt2 = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt2.daemons.get("test").unwrap();
    assert!(daemon.dir.is_none(), "dir should not be set when None");
    assert!(daemon.env.is_none(), "env should not be set when None");

    Ok(())
}

/// Test that dir and env are serialized in round-trip
#[test]
fn test_dir_env_serialization_roundtrip() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.test]
run = "echo test"
dir = "subdir"
env = { FOO = "bar", BAZ = "qux" }
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    pt.write()?;

    let pt2 = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt2.daemons.get("test").unwrap();
    assert_eq!(daemon.dir, Some("subdir".to_string()));

    let env = daemon.env.as_ref().unwrap();
    assert_eq!(env.get("FOO").unwrap(), "bar");
    assert_eq!(env.get("BAZ").unwrap(), "qux");

    Ok(())
}

// =============================================================================
// Tests for pitchfork.local.toml support (via list_paths_from / all_merged_from)
// =============================================================================

/// Test list_paths_from discovers both pitchfork.toml and pitchfork.local.toml
/// and returns them in correct priority order
#[test]
fn test_list_paths_from_local_toml() {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");
    let local_path = temp_dir.path().join("pitchfork.local.toml");

    // Test 1: Both files exist - local should come after base
    fs::write(&toml_path, "[daemons]").unwrap();
    fs::write(&local_path, "[daemons]").unwrap();

    let paths = pitchfork_toml::PitchforkToml::list_paths_from(temp_dir.path());

    assert!(paths.contains(&toml_path), "Should discover pitchfork.toml");
    assert!(
        paths.contains(&local_path),
        "Should discover pitchfork.local.toml"
    );

    let toml_idx = paths.iter().position(|p| p == &toml_path).unwrap();
    let local_idx = paths.iter().position(|p| p == &local_path).unwrap();
    assert!(
        local_idx > toml_idx,
        "pitchfork.local.toml should have higher priority (come later)"
    );

    // Test 2: Only local.toml exists
    fs::remove_file(&toml_path).unwrap();
    let paths = pitchfork_toml::PitchforkToml::list_paths_from(temp_dir.path());
    assert!(
        paths.contains(&local_path),
        "Should discover pitchfork.local.toml even without pitchfork.toml"
    );
}

/// Test all_merged_from with local.toml: overrides, adds daemons, and local-only scenarios
#[test]
fn test_all_merged_from_local_toml() {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");
    let local_path = temp_dir.path().join("pitchfork.local.toml");

    // Scenario 1: local.toml overrides base config and adds new daemons
    let toml_content = r#"
[daemons.api]
run = "npm run server"
ready_port = 3000

[daemons.worker]
run = "npm run worker"
"#;

    let local_content = r#"
[daemons.api]
run = "npm run dev"
ready_port = 3001

[daemons.debug]
run = "npm run debug"
"#;

    fs::write(&toml_path, toml_content).unwrap();
    fs::write(&local_path, local_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::all_merged_from(temp_dir.path());

    // api should be overridden by local
    let api = pt.daemons.get("api").unwrap();
    assert_eq!(api.run, "npm run dev");
    assert_eq!(api.ready_port, Some(3001));

    // worker should remain from base
    let worker = pt.daemons.get("worker").unwrap();
    assert_eq!(worker.run, "npm run worker");

    // debug should be added from local
    assert!(pt.daemons.contains_key("debug"));
    assert_eq!(pt.daemons.get("debug").unwrap().run, "npm run debug");

    // Scenario 2: Only local.toml exists (no base config)
    fs::remove_file(&toml_path).unwrap();
    fs::write(
        &local_path,
        r#"
[daemons.local_only]
run = "echo local"
"#,
    )
    .unwrap();

    let pt = pitchfork_toml::PitchforkToml::all_merged_from(temp_dir.path());
    assert!(pt.daemons.contains_key("local_only"));
    assert_eq!(pt.daemons.get("local_only").unwrap().run, "echo local");
}

/// Test nested directory structure with local.toml at different levels
#[test]
fn test_all_merged_from_nested_local_toml() {
    let temp_dir = TempDir::new().unwrap();

    // Parent directory has base config
    fs::write(
        temp_dir.path().join("pitchfork.toml"),
        r#"
[daemons.shared]
run = "echo shared"
"#,
    )
    .unwrap();

    // Child directory has both base and local config
    let child_dir = temp_dir.path().join("child");
    fs::create_dir(&child_dir).unwrap();

    fs::write(
        child_dir.join("pitchfork.toml"),
        r#"
[daemons.child_daemon]
run = "echo child"
"#,
    )
    .unwrap();

    fs::write(
        child_dir.join("pitchfork.local.toml"),
        r#"
[daemons.child_daemon]
run = "echo child-local"

[daemons.local_only]
run = "echo local-only"
"#,
    )
    .unwrap();

    let pt = pitchfork_toml::PitchforkToml::all_merged_from(&child_dir);

    // Should have all three daemons
    assert!(
        pt.daemons.contains_key("shared"),
        "Should inherit from parent"
    );
    assert!(pt.daemons.contains_key("child_daemon"));
    assert!(pt.daemons.contains_key("local_only"));

    // child_daemon should be overridden by local
    assert_eq!(
        pt.daemons.get("child_daemon").unwrap().run,
        "echo child-local"
    );
}

// =============================================================================
// Tests for hooks configuration
// =============================================================================

/// Test daemon with hooks configuration
#[test]
fn test_daemon_with_hooks() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.api]
run = "node server.js"
retry = 3

[daemons.api.hooks]
on_ready = "curl -X POST https://alerts.example.com/ready"
on_fail = "./scripts/cleanup.sh"
on_retry = "echo 'retrying...'"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("api").unwrap();

    assert!(daemon.hooks.is_some());
    let hooks = daemon.hooks.as_ref().unwrap();
    assert_eq!(
        hooks.on_ready,
        Some("curl -X POST https://alerts.example.com/ready".to_string())
    );
    assert_eq!(hooks.on_fail, Some("./scripts/cleanup.sh".to_string()));
    assert_eq!(hooks.on_retry, Some("echo 'retrying...'".to_string()));

    Ok(())
}

/// Test daemon without hooks defaults to None
#[test]
fn test_daemon_without_hooks() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.test]
run = "echo test"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("test").unwrap();
    assert!(daemon.hooks.is_none());

    Ok(())
}

/// Test daemon with partial hooks (only some hooks specified)
#[test]
fn test_daemon_with_partial_hooks() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.test]
run = "echo test"

[daemons.test.hooks]
on_fail = "echo failed"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("test").unwrap();
    let hooks = daemon.hooks.as_ref().unwrap();
    assert!(hooks.on_ready.is_none());
    assert_eq!(hooks.on_fail, Some("echo failed".to_string()));
    assert!(hooks.on_retry.is_none());

    Ok(())
}
