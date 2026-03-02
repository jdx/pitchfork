use pitchfork_cli::daemon_id::DaemonId;
use pitchfork_cli::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Helper function to get a daemon by name from a PitchforkToml
fn get_daemon_by_name<'a>(
    pt: &'a pitchfork_toml::PitchforkToml,
    name: &str,
) -> Option<&'a pitchfork_toml::PitchforkTomlDaemon> {
    pt.daemons
        .iter()
        .find(|(k, _)| k.name() == name)
        .map(|(_, v)| v)
}

/// Helper function to check if daemons contains a daemon with given name
fn daemons_contains_name(pt: &pitchfork_toml::PitchforkToml, name: &str) -> bool {
    pt.daemons.keys().any(|k| k.name() == name)
}

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
    assert!(daemons_contains_name(&pt, "test_daemon"));

    let daemon = get_daemon_by_name(&pt, "test_daemon").unwrap();
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

    // Add a daemon using the namespace that will be derived from the toml_path on write/re-read
    // (i.e. temp directory name, not "global").
    use indexmap::IndexMap;
    let mut daemons = IndexMap::new();
    let toml_ns = pitchfork_toml::namespace_from_path(&toml_path).unwrap();
    daemons.insert(
        DaemonId::try_new(&toml_ns, "test_daemon").unwrap(),
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
    // Note: namespace depends on the temp directory path, so we just check by daemon name
    assert!(daemons_contains_name(&pt_read, "test_daemon"));

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
    let daemon = get_daemon_by_name(&pt, "auto_daemon").unwrap();

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
    let daemon = get_daemon_by_name(&pt, "cron_daemon").unwrap();

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
    let daemon = get_daemon_by_name(&pt, "ready_daemon").unwrap();

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
    assert!(daemons_contains_name(&pt, "daemon1"));
    assert!(daemons_contains_name(&pt, "daemon2"));
    assert!(daemons_contains_name(&pt, "daemon3"));

    assert_eq!(
        get_daemon_by_name(&pt, "daemon2").unwrap().retry.count(),
        10
    );
    assert_eq!(get_daemon_by_name(&pt, "daemon3").unwrap().auto.len(), 2);

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
        let daemon = get_daemon_by_name(&pt, "test").unwrap();
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
    assert!(daemons_contains_name(&merged, "system_daemon"));
    assert!(daemons_contains_name(&merged, "user_daemon"));
    assert!(daemons_contains_name(&merged, "project_daemon"));
    assert!(daemons_contains_name(&merged, "shared_daemon"));

    // Verify that project config overrides user and system
    let shared = get_daemon_by_name(&merged, "shared_daemon").unwrap();
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
    let web = get_daemon_by_name(&merged, "web").unwrap();
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
    let db = get_daemon_by_name(&merged, "database").unwrap();
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
    assert!(daemons_contains_name(&merged, "app"));

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
    assert!(keys.iter().any(|k| k.name() == "first"));
    assert!(keys.iter().any(|k| k.name() == "second"));
    assert!(keys.iter().any(|k| k.name() == "third"));

    // Verify second was updated - find key with name "second"
    let second_key = keys.iter().find(|k| k.name() == "second").unwrap();
    assert_eq!(
        merged.daemons.get(*second_key).unwrap().run,
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
    let postgres = get_daemon_by_name(&pt, "postgres").unwrap();
    assert!(postgres.depends.is_empty());

    // Check redis has no dependencies
    let redis = get_daemon_by_name(&pt, "redis").unwrap();
    assert!(redis.depends.is_empty());

    // Check api has correct dependencies
    let api_key = pt.daemons.keys().find(|k| k.name() == "api").unwrap();
    let api = pt.daemons.get(api_key).unwrap();
    assert_eq!(api.depends.len(), 2);
    assert!(api.depends.iter().any(|d| d.name() == "postgres"));
    assert!(api.depends.iter().any(|d| d.name() == "redis"));

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
    let daemon = get_daemon_by_name(&pt, "standalone").unwrap();

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
    let infinite = get_daemon_by_name(&pt, "infinite_retry").unwrap();
    assert!(infinite.retry.is_infinite());
    assert_eq!(infinite.retry.count(), u32::MAX);
    assert_eq!(infinite.retry.to_string(), "infinite");

    // Test no retry (false = 0)
    let no_retry = get_daemon_by_name(&pt, "no_retry").unwrap();
    assert!(!no_retry.retry.is_infinite());
    assert_eq!(no_retry.retry.count(), 0);
    assert_eq!(no_retry.retry.to_string(), "0");

    // Test numeric retry
    let numeric = get_daemon_by_name(&pt, "numeric_retry").unwrap();
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

    let frontend = get_daemon_by_name(&pt, "frontend").unwrap();
    assert_eq!(frontend.dir, Some("frontend".to_string()));

    let api = get_daemon_by_name(&pt, "api").unwrap();
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
    let daemon = get_daemon_by_name(&pt, "test").unwrap();
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
    let daemon = get_daemon_by_name(&pt, "api").unwrap();

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
    let daemon = get_daemon_by_name(&pt, "worker").unwrap();

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
    let daemon = get_daemon_by_name(&pt, "test").unwrap();
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
    let daemon = get_daemon_by_name(&pt, "frontend").unwrap();

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
    let mut daemons: IndexMap<DaemonId, pitchfork_toml::PitchforkTomlDaemon> = IndexMap::new();
    let ns = pitchfork_toml::namespace_from_path(&toml_path)?;
    daemons.insert(
        DaemonId::try_new(&ns, "test").unwrap(),
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
    let daemon = get_daemon_by_name(&pt2, "test").unwrap();
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
    let daemon = get_daemon_by_name(&pt2, "test").unwrap();
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

    // Get the namespace (directory name)
    let ns = temp_dir.path().file_name().unwrap().to_str().unwrap();

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

    // Daemon IDs should be qualified with namespace
    let api_key = DaemonId::parse(&format!("{ns}/api")).unwrap();
    let worker_key = DaemonId::parse(&format!("{ns}/worker")).unwrap();
    let debug_key = DaemonId::parse(&format!("{ns}/debug")).unwrap();

    let pt = pt.expect("all_merged_from should succeed");

    // api should be overridden by local
    let api = pt.daemons.get(&api_key).unwrap();
    assert_eq!(api.run, "npm run dev");
    assert_eq!(api.ready_port, Some(3001));

    // worker should remain from base
    let worker = pt.daemons.get(&worker_key).unwrap();
    assert_eq!(worker.run, "npm run worker");

    // debug should be added from local
    assert!(pt.daemons.contains_key(&debug_key));
    assert_eq!(pt.daemons.get(&debug_key).unwrap().run, "npm run debug");

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
    let local_only_key = DaemonId::parse(&format!("{ns}/local_only")).unwrap();
    let pt = pt.expect("all_merged_from should succeed (local-only)");
    assert!(pt.daemons.contains_key(&local_only_key));
    assert_eq!(pt.daemons.get(&local_only_key).unwrap().run, "echo local");
}

/// Test nested directory structure with local.toml at different levels
#[test]
fn test_all_merged_from_nested_local_toml() {
    let temp_dir = TempDir::new().unwrap();

    // Get the parent namespace
    let parent_ns = temp_dir.path().file_name().unwrap().to_str().unwrap();

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

    // Daemon IDs should be qualified with their respective namespaces
    let child_ns = child_dir.file_name().unwrap().to_str().unwrap();
    let shared_key = DaemonId::parse(&format!("{parent_ns}/shared")).unwrap();
    let child_daemon_key = DaemonId::parse(&format!("{child_ns}/child_daemon")).unwrap();
    let local_only_key = DaemonId::parse(&format!("{child_ns}/local_only")).unwrap();

    let pt = pt.expect("all_merged_from with nested local.toml should succeed");
    assert!(
        pt.daemons.contains_key(&shared_key),
        "Should inherit from parent, got keys: {:?}",
        pt.daemons.keys().collect::<Vec<_>>()
    );
    assert!(pt.daemons.contains_key(&child_daemon_key));
    assert!(pt.daemons.contains_key(&local_only_key));

    // child_daemon should be overridden by local
    assert_eq!(
        pt.daemons.get(&child_daemon_key).unwrap().run,
        "echo child-local"
    );
}

// =============================================================================
// Tests for namespace collision detection in all_merged_from
// =============================================================================

/// Collision scenario: two distinct directories with the same name each have a
/// `pitchfork.toml`.  When cwd is nested inside the inner one, `find_up_all`
/// walks up and finds both, which would silently overwrite daemons.
/// `all_merged_from` must detect this and return an `Err`.
///
/// Directory layout:
/// ```
/// outer/
///   same-name/          ← namespace "same-name"  (outer)
///     pitchfork.toml
///     sub/
///       same-name/      ← namespace "same-name"  (inner, COLLISION)
///         pitchfork.toml
/// ```
#[test]
fn test_all_merged_from_namespace_collision_returns_empty() {
    let temp_dir = TempDir::new().unwrap();

    // outer/same-name/pitchfork.toml
    let outer_proj = temp_dir.path().join("same-name");
    fs::create_dir_all(&outer_proj).unwrap();
    fs::write(
        outer_proj.join("pitchfork.toml"),
        "[daemons.outer-daemon]\nrun = \"echo outer\"\n",
    )
    .unwrap();

    // outer/same-name/sub/same-name/pitchfork.toml  (same dir name → collision)
    let inner_proj = outer_proj.join("sub").join("same-name");
    fs::create_dir_all(&inner_proj).unwrap();
    fs::write(
        inner_proj.join("pitchfork.toml"),
        "[daemons.inner-daemon]\nrun = \"echo inner\"\n",
    )
    .unwrap();

    // all_merged_from with cwd = inner directory walks up and finds both configs
    let result = pitchfork_toml::PitchforkToml::all_merged_from(&inner_proj);

    // Must return an error on namespace collision
    assert!(
        result.is_err(),
        "Namespace collision should return Err, but got Ok with: {:?}",
        result
            .as_ref()
            .ok()
            .map(|pt| pt.daemons.keys().collect::<Vec<_>>())
    );
}

/// `pitchfork.local.toml` intentionally shares its directory's namespace with
/// the sibling `pitchfork.toml`.  This must NOT be flagged as a collision.
#[test]
fn test_all_merged_from_local_toml_no_collision() {
    let temp_dir = TempDir::new().unwrap();

    let proj = temp_dir.path().join("myproject");
    fs::create_dir_all(&proj).unwrap();

    fs::write(
        proj.join("pitchfork.toml"),
        "[daemons.api]\nrun = \"echo base\"\n",
    )
    .unwrap();
    fs::write(
        proj.join("pitchfork.local.toml"),
        "[daemons.api]\nrun = \"echo local\"\n",
    )
    .unwrap();

    let pt = pitchfork_toml::PitchforkToml::all_merged_from(&proj);
    let pt =
        pt.expect("pitchfork.local.toml next to pitchfork.toml must not trigger a collision error");

    // local.toml value should win (later in merge order)
    let proj_ns = proj.file_name().unwrap().to_str().unwrap();
    let api_key = DaemonId::parse(&format!("{proj_ns}/api")).unwrap();
    assert_eq!(pt.daemons[&api_key].run, "echo local");
}

/// Directories with *different* names never collide, even when nested.
#[test]
fn test_all_merged_from_different_namespaces_no_collision() {
    let temp_dir = TempDir::new().unwrap();

    // outer/parent-dir/pitchfork.toml  → namespace "parent-dir"
    let parent_dir = temp_dir.path().join("parent-dir");
    fs::create_dir_all(&parent_dir).unwrap();
    fs::write(
        parent_dir.join("pitchfork.toml"),
        "[daemons.outer]\nrun = \"echo outer\"\n",
    )
    .unwrap();

    // outer/parent-dir/child-dir/pitchfork.toml  → namespace "child-dir"
    let child_dir = parent_dir.join("child-dir");
    fs::create_dir_all(&child_dir).unwrap();
    fs::write(
        child_dir.join("pitchfork.toml"),
        "[daemons.inner]\nrun = \"echo inner\"\n",
    )
    .unwrap();

    let pt = pitchfork_toml::PitchforkToml::all_merged_from(&child_dir);
    let pt = pt.expect("Different namespace names must not trigger a collision");

    // Both daemons should be present with their respective namespaces
    assert!(!pt.daemons.is_empty());
    let outer_key = DaemonId::parse("parent-dir/outer").unwrap();
    let inner_key = DaemonId::parse("child-dir/inner").unwrap();
    assert!(
        pt.daemons.contains_key(&outer_key),
        "parent-dir/outer should be present, keys: {:?}",
        pt.daemons.keys().collect::<Vec<_>>()
    );
    assert!(
        pt.daemons.contains_key(&inner_key),
        "child-dir/inner should be present"
    );
}

#[test]
fn test_read_invalid_directory_namespace_requires_override() {
    let temp_dir = TempDir::new().unwrap();
    let project_dir = temp_dir.path().join("项目");
    fs::create_dir_all(&project_dir).unwrap();

    let toml_path = project_dir.join("pitchfork.toml");
    fs::write(
        &toml_path,
        r#"
[daemons.api]
run = "echo api"
"#,
    )
    .unwrap();

    let result = pitchfork_toml::PitchforkToml::read(&toml_path);
    assert!(
        result.is_err(),
        "non-ASCII directory should require explicit namespace"
    );

    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("namespace") && msg.contains("pitchfork.toml"),
        "error should mention namespace override guidance, got: {msg}"
    );
}

#[test]
fn test_read_namespace_override_takes_precedence_over_directory_name() {
    let temp_dir = TempDir::new().unwrap();
    let project_dir = temp_dir.path().join("项目");
    fs::create_dir_all(&project_dir).unwrap();

    let toml_path = project_dir.join("pitchfork.toml");
    fs::write(
        &toml_path,
        r#"
namespace = "my-proj"

[daemons.api]
run = "echo api"
"#,
    )
    .unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path).expect("read should succeed");
    let key = DaemonId::parse("my-proj/api").unwrap();
    assert!(
        pt.daemons.contains_key(&key),
        "daemon should use explicit namespace override"
    );
}

#[test]
fn test_local_toml_inherits_base_namespace_override() {
    let temp_dir = TempDir::new().unwrap();

    fs::write(
        temp_dir.path().join("pitchfork.toml"),
        r#"
namespace = "team"

[daemons.base]
run = "echo base"
"#,
    )
    .unwrap();

    fs::write(
        temp_dir.path().join("pitchfork.local.toml"),
        r#"
[daemons.local]
run = "echo local"
"#,
    )
    .unwrap();

    let pt = pitchfork_toml::PitchforkToml::all_merged_from(temp_dir.path())
        .expect("local should inherit base namespace override");

    assert!(
        pt.daemons
            .contains_key(&DaemonId::parse("team/base").unwrap())
    );
    assert!(
        pt.daemons
            .contains_key(&DaemonId::parse("team/local").unwrap())
    );
}

#[test]
fn test_local_toml_namespace_must_match_base_namespace_override() {
    let temp_dir = TempDir::new().unwrap();

    fs::write(
        temp_dir.path().join("pitchfork.toml"),
        r#"
namespace = "team"

[daemons.base]
run = "echo base"
"#,
    )
    .unwrap();

    let local_path = temp_dir.path().join("pitchfork.local.toml");
    fs::write(
        &local_path,
        r#"
namespace = "other"

[daemons.local]
run = "echo local"
"#,
    )
    .unwrap();

    let result = pitchfork_toml::PitchforkToml::read(&local_path);
    assert!(result.is_err(), "mismatched local namespace should fail");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("does not match sibling"),
        "error should explain mismatch, got: {msg}"
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
    let daemon = get_daemon_by_name(&pt, "api").unwrap();

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
    let daemon = get_daemon_by_name(&pt, "test").unwrap();
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
    let daemon = get_daemon_by_name(&pt, "test").unwrap();
    let hooks = daemon.hooks.as_ref().unwrap();
    assert!(hooks.on_ready.is_none());
    assert_eq!(hooks.on_fail, Some("echo failed".to_string()));
    assert!(hooks.on_retry.is_none());

    Ok(())
}

// =============================================================================
// Tests for resolve_daemon_id with invalid input
// =============================================================================

/// Test resolve_daemon_id with invalid input (spaces, --, etc.)
#[test]
fn test_resolve_daemon_id_invalid_input() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.valid_daemon]
run = "echo test"
"#;

    fs::write(&toml_path, toml_content).unwrap();
    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;

    // Valid daemon should resolve
    let result = pt.resolve_daemon_id("valid_daemon");
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap().len(),
        1,
        "valid_daemon should resolve to exactly one match"
    );

    // Daemon with spaces should fail
    let result = pt.resolve_daemon_id("my daemon");
    assert!(result.is_err(), "Daemon ID with spaces should be rejected");

    // Daemon with -- should fail
    let result = pt.resolve_daemon_id("my--daemon");
    assert!(result.is_err(), "Daemon ID with -- should be rejected");

    // Daemon with .. should fail
    let result = pt.resolve_daemon_id("my..daemon");
    assert!(result.is_err(), "Daemon ID with .. should be rejected");

    // Invalid qualified ID should fail
    let result = pt.resolve_daemon_id("invalid space/daemon");
    assert!(
        result.is_err(),
        "Qualified ID with invalid namespace should be rejected"
    );

    Ok(())
}

/// Test resolve_daemon_id_prefer_local with invalid input
#[test]
fn test_resolve_daemon_id_prefer_local_invalid_input() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.valid_daemon]
run = "echo test"
"#;

    fs::write(&toml_path, toml_content).unwrap();
    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;

    // Valid daemon should resolve
    let result = pt.resolve_daemon_id_prefer_local("valid_daemon", temp_dir.path());
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap().name(),
        "valid_daemon",
        "should resolve to valid_daemon"
    );

    // Daemon with spaces should fail
    let result = pt.resolve_daemon_id_prefer_local("my daemon", temp_dir.path());
    assert!(result.is_err(), "Daemon ID with spaces should be rejected");

    // Daemon with -- should fail
    let result = pt.resolve_daemon_id_prefer_local("my--daemon", temp_dir.path());
    assert!(result.is_err(), "Daemon ID with -- should be rejected");

    Ok(())
}

// =============================================================================
// Tests for cross-namespace dependency syntax
// =============================================================================

/// Test cross-namespace dependency parsing and preservation
#[test]
fn test_cross_namespace_dependency() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    let toml_content = r#"
[daemons.postgres]
run = "postgres -D /data"

[daemons.api]
run = "npm run server"
depends = ["postgres", "global/redis"]
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let api = get_daemon_by_name(&pt, "api").unwrap();

    // Should have 2 dependencies
    assert_eq!(api.depends.len(), 2);

    // First dep should be same-namespace (postgres)
    let postgres_dep = api.depends.iter().find(|d| d.name() == "postgres").unwrap();
    // The namespace should match the toml file's parent directory name.
    // Canonicalize the toml_path to resolve symlinks (e.g. /tmp → /private/tmp on macOS),
    // then take the parent directory name as the namespace.
    let canonical_toml = toml_path.canonicalize().unwrap();
    let expected_ns = canonical_toml
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(postgres_dep.namespace(), expected_ns);

    // Second dep should be cross-namespace (global/redis)
    let redis_dep = api.depends.iter().find(|d| d.name() == "redis").unwrap();
    assert_eq!(redis_dep.namespace(), "global");

    Ok(())
}

/// Test that invalid cross-namespace dependency fails parsing
#[test]
fn test_invalid_cross_namespace_dependency_fails() {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    // Contains an invalid cross-namespace dependency (has spaces)
    let toml_content = r#"
[daemons.api]
run = "npm run server"
depends = ["valid_dep", "invalid namespace/redis"]
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let result = pitchfork_toml::PitchforkToml::read(&toml_path);
    assert!(
        result.is_err(),
        "Invalid cross-namespace dependency should fail parsing"
    );
}

// =============================================================================
// Tests for invalid daemon names in config
// =============================================================================

/// Test that invalid daemon name in config file returns error
#[test]
fn test_invalid_daemon_name_in_config() {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    // Daemon name with -- is invalid
    let toml_content = r#"
[daemons.my--daemon]
run = "echo test"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let result = pitchfork_toml::PitchforkToml::read(&toml_path);
    assert!(
        result.is_err(),
        "Config with invalid daemon name 'my--daemon' should fail to parse"
    );
}

/// Test that daemon name with spaces in config file returns error
#[test]
fn test_daemon_name_with_spaces_in_config() {
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("pitchfork.toml");

    // Daemon name with spaces - TOML requires quotes for keys with spaces
    let toml_content = r#"
[daemons."my daemon"]
run = "echo test"
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let result = pitchfork_toml::PitchforkToml::read(&toml_path);
    assert!(
        result.is_err(),
        "Config with invalid daemon name 'my daemon' should fail to parse"
    );
}

// =============================================================================
// Tests for namespace resolution edge cases
// =============================================================================

/// Test namespace_from_path correctly extracts namespace from absolute paths
#[test]
fn test_namespace_from_path_absolute() {
    let temp_dir = TempDir::new().unwrap();
    let project_dir = temp_dir.path().join("my-project");
    fs::create_dir_all(&project_dir).unwrap();
    let toml_path = project_dir.join("pitchfork.toml");
    fs::write(&toml_path, "[daemons]\n").unwrap();

    let namespace = pitchfork_toml::namespace_from_path(&toml_path).unwrap();
    assert_eq!(namespace, "my-project");
}

/// Test namespace_from_path with relative path that gets canonicalized
#[test]
fn test_namespace_from_relative_path_in_subdirectory() {
    let temp_dir = TempDir::new().unwrap();
    let project_dir = temp_dir.path().join("test-project");
    fs::create_dir_all(&project_dir).unwrap();
    let toml_path = project_dir.join("pitchfork.toml");
    fs::write(&toml_path, "[daemons]\n").unwrap();

    // Read from the actual file path to test namespace extraction
    let pt = pitchfork_toml::PitchforkToml::read(&toml_path).unwrap();
    // When reading, the namespace is derived from the path
    // The path stored should be the one we passed in
    assert_eq!(pt.path, Some(toml_path));
}

/// Test namespace_from_path rejects directory containing double dashes without override
#[test]
fn test_namespace_from_path_rejects_double_dashes_without_override() {
    let temp_dir = TempDir::new().unwrap();
    let project_dir = temp_dir.path().join("my--project");
    fs::create_dir_all(&project_dir).unwrap();
    let toml_path = project_dir.join("pitchfork.toml");
    fs::write(&toml_path, "[daemons]\n").unwrap();

    let err = pitchfork_toml::namespace_from_path(&toml_path).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("namespace"));
    assert!(msg.contains("namespace ="));
}

/// Test namespace_from_path rejects non-ASCII/spaces without override
#[test]
fn test_namespace_from_path_rejects_non_ascii_and_spaces_without_override() {
    let temp_dir = TempDir::new().unwrap();
    let project_dir = temp_dir.path().join("我的 project");
    fs::create_dir_all(&project_dir).unwrap();
    let toml_path = project_dir.join("pitchfork.toml");
    fs::write(&toml_path, "[daemons]\n").unwrap();

    let err = pitchfork_toml::namespace_from_path(&toml_path).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("namespace"));
    assert!(msg.contains("namespace ="));
}

/// Test explicit namespace override works for otherwise invalid directory names
#[test]
fn test_namespace_from_path_uses_explicit_override() {
    let temp_dir = TempDir::new().unwrap();
    let project_dir = temp_dir.path().join("我的 project");
    fs::create_dir_all(&project_dir).unwrap();
    let toml_path = project_dir.join("pitchfork.toml");
    fs::write(&toml_path, "namespace = \"my-proj\"\n[daemons]\n").unwrap();

    let ns = pitchfork_toml::namespace_from_path(&toml_path).unwrap();
    assert_eq!(ns, "my-proj");
}

/// Shared helper: create a merged PitchforkToml with two projects ("project-a"
/// and "project-b") each containing a daemon named "api".
fn make_two_project_merged(
    temp_dir: &TempDir,
) -> Result<(
    pitchfork_toml::PitchforkToml,
    std::path::PathBuf,
    std::path::PathBuf,
)> {
    let project_a = temp_dir.path().join("project-a");
    let project_b = temp_dir.path().join("project-b");
    fs::create_dir_all(&project_a).unwrap();
    fs::create_dir_all(&project_b).unwrap();

    fs::write(
        project_a.join("pitchfork.toml"),
        "[daemons.api]\nrun = \"echo a\"\n",
    )
    .unwrap();
    fs::write(
        project_b.join("pitchfork.toml"),
        "[daemons.api]\nrun = \"echo b\"\n",
    )
    .unwrap();

    let pt_a = pitchfork_toml::PitchforkToml::read(project_a.join("pitchfork.toml"))?;
    let pt_b = pitchfork_toml::PitchforkToml::read(project_b.join("pitchfork.toml"))?;
    let mut merged = pitchfork_toml::PitchforkToml::default();
    merged.merge(pt_a);
    merged.merge(pt_b);
    Ok((merged, project_a, project_b))
}

/// Test resolve_daemon_id with ambiguous short ID across namespaces
#[test]
fn test_resolve_daemon_id_ambiguity() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let (merged, _, _) = make_two_project_merged(&temp_dir)?;

    // Both should exist with different namespaces
    assert_eq!(merged.daemons.len(), 2);

    // Resolving "api" should return multiple matches
    let matches = merged.resolve_daemon_id("api")?;
    assert_eq!(matches.len(), 2, "Should find api in both namespaces");

    // Resolving qualified ID should return exactly one match
    let matches = merged.resolve_daemon_id("project-a/api")?;
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].namespace(), "project-a");

    Ok(())
}

/// Test resolve_daemon_id_prefer_local prefers current namespace
#[test]
fn test_resolve_daemon_id_prefer_local() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let (merged, project_a, project_b) = make_two_project_merged(&temp_dir)?;

    // When in project-a directory, should prefer project-a/api
    let resolved = merged.resolve_daemon_id_prefer_local("api", &project_a)?;
    assert_eq!(resolved.namespace(), "project-a");

    // When in project-b directory, should prefer project-b/api
    let resolved = merged.resolve_daemon_id_prefer_local("api", &project_b)?;
    assert_eq!(resolved.namespace(), "project-b");

    Ok(())
}

#[test]
fn test_resolve_daemon_id_prefer_local_errors_on_global_ambiguity() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let (mut merged, project_a, _) = make_two_project_merged(&temp_dir)?;

    let mut template = merged
        .daemons
        .values()
        .next()
        .cloned()
        .expect("expected at least one daemon in merged config");

    template.run = "echo global-worker".to_string();
    merged
        .daemons
        .insert(DaemonId::try_new("global", "worker")?, template.clone());

    template.run = "echo project-b-worker".to_string();
    merged
        .daemons
        .insert(DaemonId::try_new("project-b", "worker")?, template);

    let resolved = merged.resolve_daemon_id_prefer_local("worker", &project_a);
    assert!(
        resolved.is_err(),
        "expected ambiguity error for short id 'worker'"
    );
    let err_text = resolved.unwrap_err().to_string();
    assert!(
        err_text.contains("ambiguous"),
        "unexpected error message: {err_text}"
    );

    let adhoc = merged.resolve_daemon_id_prefer_local("adhoc-worker", &project_a);
    assert!(
        adhoc.is_err(),
        "unconfigured short id should return not found instead of implicit global fallback"
    );

    // Global fallback is allowed only when global/<id> is configured.
    let mut global_template = merged
        .daemons
        .values()
        .next()
        .cloned()
        .expect("expected at least one daemon in merged config");
    global_template.run = "echo global-adhoc-worker".to_string();
    merged.daemons.insert(
        DaemonId::try_new("global", "adhoc-worker")?,
        global_template,
    );

    let global_resolved = merged.resolve_daemon_id_prefer_local("adhoc-worker", &project_a)?;
    assert_eq!(global_resolved.namespace(), "global");
    assert_eq!(global_resolved.name(), "adhoc-worker");

    Ok(())
}

/// Test from_safe_path with namespace containing dot character
#[test]
fn test_from_safe_path_with_dot_in_namespace() {
    // Dot in namespace (from directory like ".hidden" or version like "v1.0")
    // This should work as long as the namespace itself isn't just "."
    let result = DaemonId::from_safe_path("v1.0--api");
    assert!(result.is_ok());
    let id = result.unwrap();
    assert_eq!(id.namespace(), "v1.0");
    assert_eq!(id.name(), "api");

    // Single dot as namespace should be rejected
    let result = DaemonId::from_safe_path(".--api");
    assert!(
        result.is_err(),
        "from_safe_path should reject '.' as namespace"
    );
}

/// Test from_safe_path with various edge characters in namespace
#[test]
fn test_from_safe_path_edge_characters() {
    // Underscore is valid
    let result = DaemonId::from_safe_path("my_project--api");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().namespace(), "my_project");

    // Single dash is valid
    let result = DaemonId::from_safe_path("my-project--api");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().namespace(), "my-project");

    // Numbers are valid
    let result = DaemonId::from_safe_path("project123--api");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().namespace(), "project123");
}

/// Test that try_new rejects invalid user input properly
#[test]
fn test_try_new_rejects_invalid_input() {
    // Double dash in name
    assert!(DaemonId::try_new("valid", "my--daemon").is_err());

    // Space in name
    assert!(DaemonId::try_new("valid", "my daemon").is_err());

    // Forward slash in name
    assert!(DaemonId::try_new("valid", "my/daemon").is_err());

    // Empty name
    assert!(DaemonId::try_new("valid", "").is_err());

    // Empty namespace
    assert!(DaemonId::try_new("", "daemon").is_err());

    // Dot as namespace
    assert!(DaemonId::try_new(".", "daemon").is_err());

    // Parent directory reference
    assert!(DaemonId::try_new("..", "daemon").is_err());
}
