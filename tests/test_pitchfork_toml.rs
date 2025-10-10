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
