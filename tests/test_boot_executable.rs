//! Boot registration only: isolated HOME and a fake service manager, no supervisor.
//! One test owns the process-wide environment and settings lazies.
#![cfg(target_os = "linux")]

use pitchfork_cli::boot_manager::BootManager;
use std::os::unix::fs::{PermissionsExt, symlink};

#[test]
fn boot_registration_uses_configured_executable() {
    // HOME does not relocate system units. Never let this test write /etc.
    assert!(
        !nix::unistd::Uid::effective().is_root(),
        "run this test as non-root"
    );
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let bin = home.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let systemctl = bin.join("systemctl");
    std::fs::write(
        &systemctl,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/systemctl.calls"
case "$*" in
  '--user is-enabled pitchfork.service') test -f "$HOME/.config/systemd/user/pitchfork.service" ;;
  'is-enabled pitchfork.service') exit 1 ;;
  '--user daemon-reload'|'--user enable pitchfork.service'|'--user disable pitchfork.service') exit 0 ;;
  *) exit 99 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
    let stable = bin.join("pitchfork-stable");
    symlink(std::env::current_exe().unwrap(), &stable).unwrap();
    let config_dir = home.join(".config/pitchfork");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "[settings.boot]\nexecutable = {}\n",
            toml::Value::String(stable.to_string_lossy().into_owned())
        ),
    )
    .unwrap();
    // SAFETY: this is the only test in this process; no worker threads exist.
    unsafe {
        std::env::set_var("HOME", home);
        std::env::set_var("PITCHFORK_CONFIG_DIR", &config_dir);
        std::env::set_var("PATH", &bin);
        std::env::remove_var("PITCHFORK_BOOT_EXECUTABLE");
    }
    std::env::set_current_dir(home).unwrap();
    let manager = BootManager::new().unwrap();
    manager.enable().unwrap();
    let unit = home.join(".config/systemd/user/pitchfork.service");
    let contents = std::fs::read_to_string(&unit).unwrap();
    assert!(
        contents.contains(&format!(
            "ExecStart={} supervisor run --boot\n",
            stable.display()
        )),
        "{contents}"
    );
    assert!(manager.is_current_level_up_to_date().unwrap());

    // A stable path stays literal even when its target differs from this
    // process (for example, an older supervisor running during an upgrade).
    let with_policy = format!("{contents}\n[Service]\nTimeoutStopSec=240\n");
    std::fs::write(&unit, &with_policy).unwrap();
    std::fs::remove_file(&stable).unwrap();
    symlink("/bin/sh", &stable).unwrap();
    let calls = std::fs::read_to_string(home.join("systemctl.calls")).unwrap();
    manager.check_and_reregister_if_stale();
    assert!(manager.is_current_level_up_to_date().unwrap());
    assert_eq!(std::fs::read_to_string(&unit).unwrap(), with_policy);
    assert_eq!(
        std::fs::read_to_string(home.join("systemctl.calls")).unwrap(),
        calls
    );

    // Repair a version-pinned registration toward the configured alias,
    // not toward the currently running executable.
    std::fs::write(
        &unit,
        "[Service]\nExecStart=/removed/version/pitchfork supervisor run --boot\n",
    )
    .unwrap();
    assert!(!manager.is_current_level_up_to_date().unwrap());
    manager.check_and_reregister_if_stale();
    assert_eq!(std::fs::read_to_string(&unit).unwrap(), contents);
    manager.refresh().unwrap();
    assert_eq!(std::fs::read_to_string(&unit).unwrap(), contents);

    // No registration is not stale; read errors must not replace an entry.
    std::fs::remove_file(&unit).unwrap();
    let calls = std::fs::read_to_string(home.join("systemctl.calls")).unwrap();
    manager.check_and_reregister_if_stale();
    assert!(!unit.exists());
    std::fs::create_dir(&unit).unwrap();
    manager.check_and_reregister_if_stale();
    assert!(unit.is_dir());
    assert_eq!(
        std::fs::read_to_string(home.join("systemctl.calls")).unwrap(),
        calls
    );
    std::fs::remove_dir(&unit).unwrap();
    std::fs::write(&unit, &contents).unwrap();

    // A broken explicit choice must not silently register the running binary.
    let non_executable = home.join("not-executable");
    std::fs::write(&non_executable, "not executable").unwrap();
    let unsupported = [
        "with space",
        "with\t tab",
        "with\nnewline",
        "quote\"",
        "back\\slash",
        "percent%",
        "dollar$",
        "single'quote",
    ];
    for name in unsupported {
        symlink("/bin/sh", home.join(name)).unwrap();
    }
    let dangling = home.join("dangling");
    symlink(home.join("absent-target"), &dangling).unwrap();
    let invalid = [
        "relative/pitchfork".to_string(),
        "~/bin/pitchfork".to_string(),
        dangling.display().to_string(),
        home.join("missing").display().to_string(),
        home.display().to_string(),
        non_executable.display().to_string(),
    ];
    for path in invalid
        .into_iter()
        .chain(unsupported.map(|name| home.join(name).display().to_string()))
    {
        // SAFETY: this single test is still the sole environment owner.
        unsafe {
            std::env::set_var("PITCHFORK_BOOT_EXECUTABLE", &path);
        }
        pitchfork_cli::settings::reload_settings();
        let invalid_manager = BootManager::new().unwrap();
        let calls = std::fs::read_to_string(home.join("systemctl.calls")).unwrap();
        let error = invalid_manager.refresh().unwrap_err().to_string();
        assert!(error.contains("settings.boot.executable"), "{error}");
        assert!(error.contains(&path), "{error}");
        assert!(invalid_manager.enable().is_err());
        assert!(invalid_manager.is_current_level_up_to_date().is_err());
        invalid_manager.check_and_reregister_if_stale();
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), contents);
        assert_eq!(
            std::fs::read_to_string(home.join("systemctl.calls")).unwrap(),
            calls
        );

        // Bad explicit settings must not trap an existing registration.
        // The fake manager always reports the real system level as disabled;
        // only this test's HOME-local fixture can be removed here.
        assert!(invalid_manager.is_enabled().unwrap());
        assert!(invalid_manager.is_current_level_enabled().unwrap());
        invalid_manager.disable().unwrap();
        assert!(!unit.exists());
        assert!(!invalid_manager.is_enabled().unwrap());
        std::fs::write(&unit, &contents).unwrap();
    }

    // Unset/empty keeps the old behavior, including following the currently
    // running binary rather than preserving arbitrary registered symlinks.
    unsafe {
        std::env::remove_var("PITCHFORK_BOOT_EXECUTABLE");
    }
    std::fs::write(config_dir.join("config.toml"), "").unwrap();
    pitchfork_cli::settings::reload_settings();
    let default_manager = BootManager::new().unwrap();
    assert!(!default_manager.is_current_level_up_to_date().unwrap());
    default_manager.check_and_reregister_if_stale();
    let default_contents = std::fs::read_to_string(&unit).unwrap();
    assert!(default_contents.contains(&format!(
            "ExecStart={} supervisor run --boot\n",
            std::env::current_exe()
                .unwrap()
                .canonicalize()
                .unwrap()
                .display()
        )));
    assert!(default_manager.is_current_level_up_to_date().unwrap());
    std::fs::remove_file(&unit).unwrap();
    default_manager.enable().unwrap();
    assert_eq!(std::fs::read_to_string(&unit).unwrap(), default_contents);
    default_manager.refresh().unwrap();
    assert_eq!(std::fs::read_to_string(&unit).unwrap(), default_contents);
}
