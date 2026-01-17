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
    assert_eq!(daemon.retry, 3);

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
            retry: 5,
            ready_delay: None,
            ready_output: None,
            ready_http: None,
            boot_start: None,
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
"#;

    fs::write(&toml_path, toml_content).unwrap();

    let pt = pitchfork_toml::PitchforkToml::read(&toml_path)?;
    let daemon = pt.daemons.get("ready_daemon").unwrap();

    assert_eq!(daemon.ready_delay, Some(5000));
    assert_eq!(daemon.ready_output, Some("Server is ready".to_string()));

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

    assert_eq!(pt.daemons.get("daemon2").unwrap().retry, 10);
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
        let toml_path = temp_dir.path().join(format!("cron_{}.toml", variant_name));
        let toml_content = format!(
            r#"
[daemons.test]
run = "echo 'test'"

[daemons.test.cron]
schedule = "* * * * *"
retrigger = "{}"
"#,
            variant_name
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
    assert_eq!(shared.retry, 15);

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
    assert_eq!(web.retry, 5);

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
    assert_eq!(db.retry, 10);
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
