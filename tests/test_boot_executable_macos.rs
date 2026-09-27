//! Plist registration only, never launchctl or a supervisor.
#![cfg(target_os = "macos")]

use pitchfork_cli::boot_manager::BootManager;
use std::os::unix::fs::symlink;

#[test]
fn stable_boot_plist_preserves_local_policy() {
    // HOME cannot relocate /Library/LaunchDaemons. Never run this as root.
    assert!(
        !nix::unistd::Uid::effective().is_root(),
        "run this test as non-root"
    );
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let stable = home.join("stable pitchfork");
    symlink(std::env::current_exe().unwrap(), &stable).unwrap();
    // SAFETY: the only test in this binary owns its environment and lazies.
    unsafe {
        std::env::set_var("HOME", home);
        std::env::set_var("PITCHFORK_CONFIG_DIR", home);
        std::env::set_var("PITCHFORK_BOOT_EXECUTABLE", &stable);
        // Any accidental attempt to invoke launchctl must fail, not go live.
        std::env::set_var("PATH", home);
    }
    std::env::set_current_dir(home).unwrap();
    let manager = BootManager::new().unwrap();
    // refresh writes only the user fixture. enable also checks real system
    // registrations, which may legitimately exist on the machine running this.
    manager.refresh().unwrap();
    let file = home.join("Library/LaunchAgents/pitchfork.plist");
    let mut plist = plist::Value::from_file(&file).unwrap();
    assert_eq!(
        plist.as_dictionary().unwrap()["ProgramArguments"]
            .as_array()
            .unwrap()[0]
            .as_string(),
        stable.to_str()
    );
    let dict = plist.as_dictionary_mut().unwrap();
    dict.insert("KeepAlive".into(), plist::Value::Boolean(true));
    dict.insert("ExitTimeOut".into(), plist::Value::Integer(240.into()));
    plist.to_file_xml(&file).unwrap();
    let with_policy = std::fs::read(&file).unwrap();
    manager.check_and_reregister_if_stale();
    assert_eq!(std::fs::read(&file).unwrap(), with_policy);
    std::fs::remove_file(&stable).unwrap();
    symlink("/bin/sh", &stable).unwrap();
    manager.check_and_reregister_if_stale();
    assert!(manager.is_current_level_up_to_date().unwrap());
    assert_eq!(std::fs::read(&file).unwrap(), with_policy);

    // Stale registrations are repaired to the configured spelling.
    plist
        .as_dictionary_mut()
        .unwrap()
        .get_mut("ProgramArguments")
        .unwrap()
        .as_array_mut()
        .unwrap()[0] = plist::Value::String("/missing/pitchfork".into());
    plist.to_file_xml(&file).unwrap();
    assert!(!manager.is_current_level_up_to_date().unwrap());
    manager.check_and_reregister_if_stale();
    let repaired = plist::Value::from_file(&file).unwrap();
    assert_eq!(
        repaired.as_dictionary().unwrap()["ProgramArguments"]
            .as_array()
            .unwrap()[0]
            .as_string(),
        stable.to_str()
    );

    // A disappeared explicit target fails closed, even on an existing manager.
    std::fs::remove_file(&stable).unwrap();
    let before = std::fs::read(&file).unwrap();
    assert!(
        manager
            .refresh()
            .unwrap_err()
            .to_string()
            .contains("settings.boot.executable")
    );
    manager.check_and_reregister_if_stale();
    assert_eq!(std::fs::read(&file).unwrap(), before);
}
