//! Boot registration against private registry hives. RegOverridePredefKey is
//! process-local, so no real login registration is created or removed.
//! This binary has one test because registry overrides and settings are global.
#![cfg(windows)]

use pitchfork_cli::boot_manager::BootManager;
use windows_registry::{CURRENT_USER, Key, Type};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, RegGetKeySecurity, RegOverridePredefKey,
    RegSetKeySecurity,
};

const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_APPROVED: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

struct RegistrySandbox {
    software: Key,
    name: String,
    user: Key,
    machine: Key,
}

impl RegistrySandbox {
    fn new() -> Self {
        let software = CURRENT_USER
            .options()
            .read()
            .write()
            .open("Software")
            .unwrap();
        let name = format!("pitchfork-test-{}", uuid::Uuid::new_v4());
        let user = software.create(format!(r"{name}\user")).unwrap();
        let machine = software.create(format!(r"{name}\machine")).unwrap();
        let sandbox = Self {
            software,
            name,
            user,
            machine,
        };
        // SAFETY: handles remain alive until after both overrides are reset.
        // This is the only test in the process and no worker has been started.
        unsafe {
            assert_eq!(
                RegOverridePredefKey(HKEY_CURRENT_USER, sandbox.user.as_raw()),
                0
            );
            assert_eq!(
                RegOverridePredefKey(HKEY_LOCAL_MACHINE, sandbox.machine.as_raw()),
                0
            );
        }
        sandbox
    }
}

impl Drop for RegistrySandbox {
    fn drop(&mut self) {
        unsafe {
            assert_eq!(
                RegOverridePredefKey(HKEY_LOCAL_MACHINE, std::ptr::null_mut()),
                0
            );
            assert_eq!(
                RegOverridePredefKey(HKEY_CURRENT_USER, std::ptr::null_mut()),
                0
            );
        }
        self.software.remove_tree(&self.name).unwrap();
    }
}

/// Deny writes to one sandbox key, restoring its original DACL even on failure.
struct DenyRegistryWrites {
    key: Key,
    original: Vec<u32>,
}

impl DenyRegistryWrites {
    fn new(root: &Key, path: &str) -> Self {
        // Keep WRITE_DAC access on this handle so restoration does not require
        // reopening the key under the temporary access restrictions.
        let key = root.options().access(KEY_ALL_ACCESS).open(path).unwrap();
        let mut size = 0;
        // SAFETY: the first call queries the size, then an aligned buffer of
        // that size receives the self-relative security descriptor.
        unsafe {
            assert_eq!(
                RegGetKeySecurity(
                    key.as_raw(),
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    &mut size
                ),
                ERROR_INSUFFICIENT_BUFFER
            );
            let mut original = vec![0u32; (size as usize).div_ceil(size_of::<u32>())];
            assert_eq!(
                RegGetKeySecurity(
                    key.as_raw(),
                    DACL_SECURITY_INFORMATION,
                    original.as_mut_ptr().cast(),
                    &mut size
                ),
                0
            );
            let guard = Self { key, original };
            // Deny KEY_SET_VALUE explicitly, including inherited grants. Reads
            // still succeed, so root selection sees the existing registration.
            let sddl: Vec<u16> = "D:(D;;0x00000002;;;WD)(A;;KR;;;WD)\0"
                .encode_utf16()
                .collect();
            let mut descriptor = std::ptr::null_mut();
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut()
                ),
                0
            );
            let result =
                RegSetKeySecurity(guard.key.as_raw(), DACL_SECURITY_INFORMATION, descriptor);
            LocalFree(descriptor);
            assert_eq!(result, 0);
            guard
        }
    }
}

impl Drop for DenyRegistryWrites {
    fn drop(&mut self) {
        // SAFETY: the handle retains WRITE_DAC, and the saved descriptor stays
        // alive until RegSetKeySecurity finishes restoring this sandbox key.
        unsafe {
            assert_eq!(
                RegSetKeySecurity(
                    self.key.as_raw(),
                    DACL_SECURITY_INFORMATION,
                    self.original.as_mut_ptr().cast()
                ),
                0
            );
        }
    }
}

#[test]
fn windows_boot_registration_round_trips_spaces_unicode_and_repairs() {
    let registry = RegistrySandbox::new();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("Program Files λ");
    std::fs::create_dir_all(&dir).unwrap();
    let executable = dir.join("pitchfork.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let config = temp.path().join("config");
    std::fs::create_dir(&config).unwrap();
    // SAFETY: this test owns its process-wide environment and settings lazies.
    unsafe {
        std::env::set_var("PITCHFORK_CONFIG_DIR", &config);
        std::env::set_var("PITCHFORK_BOOT_EXECUTABLE", &executable);
    }
    std::env::set_current_dir(temp.path()).unwrap();
    let manager = BootManager::new().unwrap();
    assert!(!manager.is_current_level_up_to_date().unwrap());
    manager.check_and_reregister_if_stale();
    assert!(!manager.is_enabled().unwrap());
    manager.enable().unwrap();
    let key = registry.machine.open(RUN).unwrap();
    let expected = format!("\"{}\" supervisor run --boot", executable.display());
    assert_eq!(key.get_string("pitchfork").unwrap(), expected);
    assert!(manager.is_enabled().unwrap());
    assert!(!manager.is_other_level_enabled().unwrap());
    assert!(manager.is_current_level_up_to_date().unwrap());
    // Enabling an existing Windows registration must be idempotent: there is
    // no second privilege-level launcher to conflict with this same entry.
    manager.enable().unwrap();
    manager.refresh().unwrap();
    manager.check_and_reregister_if_stale();
    assert_eq!(key.get_string("pitchfork").unwrap(), expected);

    let key = registry.machine.options().write().read().open(RUN).unwrap();
    let stale = r#""C:\removed\pitchfork.exe" supervisor run --boot"#;

    // A user-only registration must be readable and removable without creating
    // a machine entry during an automatic check.
    let user_key = registry.user.create(RUN).unwrap();
    key.remove_value("pitchfork").unwrap();
    user_key.set_string("pitchfork", &expected).unwrap();
    assert!(manager.is_enabled().unwrap());
    assert!(manager.is_current_level_up_to_date().unwrap());
    manager.check_and_reregister_if_stale();
    assert!(key.get_string("pitchfork").is_err());
    // Explicit updates must not move a user entry to HKLM even when HKLM is
    // writable. Automatic repair must retain the same root as well.
    for update in [BootManager::enable, BootManager::refresh] {
        user_key.set_string("pitchfork", stale).unwrap();
        update(&manager).unwrap();
        assert_eq!(user_key.get_string("pitchfork").unwrap(), expected);
        assert!(key.get_string("pitchfork").is_err());
    }
    user_key.set_string("pitchfork", stale).unwrap();
    manager.check_and_reregister_if_stale();
    assert_eq!(user_key.get_string("pitchfork").unwrap(), expected);
    assert!(key.get_string("pitchfork").is_err());
    manager.disable().unwrap();
    assert!(!manager.is_enabled().unwrap());
    assert!(user_key.get_string("pitchfork").is_err());

    // The machine entry takes precedence when both roots have registrations,
    // regardless of which root has the selected executable. Disable clears both.
    key.set_string("pitchfork", stale).unwrap();
    user_key.set_string("pitchfork", &expected).unwrap();
    assert!(!manager.is_current_level_up_to_date().unwrap());
    manager.check_and_reregister_if_stale();
    assert_eq!(key.get_string("pitchfork").unwrap(), expected);
    assert_eq!(user_key.get_string("pitchfork").unwrap(), expected);
    user_key.set_string("pitchfork", stale).unwrap();
    assert!(manager.is_current_level_up_to_date().unwrap());
    manager.enable().unwrap();
    manager.refresh().unwrap();
    manager.check_and_reregister_if_stale();
    assert_eq!(key.get_string("pitchfork").unwrap(), expected);
    assert_eq!(user_key.get_string("pitchfork").unwrap(), stale);
    manager.disable().unwrap();
    assert!(key.get_string("pitchfork").is_err());
    assert!(user_key.get_string("pitchfork").is_err());
    manager.enable().unwrap();
    assert_eq!(key.get_string("pitchfork").unwrap(), expected);

    // An approval record affects only the Run entry in its own root. Exercise
    // both directions so an orphaned disabled record cannot hide an enabled
    // entry, and mixed approval states still allow status and disable to work.
    let enabled = [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let disabled = [0x03, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];
    key.remove_value("pitchfork").unwrap();
    for (root, other_root) in [
        (&registry.machine, &registry.user),
        (&registry.user, &registry.machine),
    ] {
        let run = root.create(RUN).unwrap();
        let other_run = other_root.create(RUN).unwrap();
        let approval = root.create(STARTUP_APPROVED).unwrap();
        let other_approval = other_root.create(STARTUP_APPROVED).unwrap();
        run.set_string("pitchfork", &expected).unwrap();
        approval
            .set_bytes("pitchfork", Type::Bytes, &enabled)
            .unwrap();
        other_approval
            .set_bytes("pitchfork", Type::Bytes, &disabled)
            .unwrap();
        assert!(manager.is_enabled().unwrap());
        assert!(manager.is_current_level_enabled().unwrap());
        manager.check_and_reregister_if_stale();
        assert_eq!(approval.get_value("pitchfork").unwrap().as_ref(), enabled);
        assert_eq!(
            other_approval.get_value("pitchfork").unwrap().as_ref(),
            disabled
        );

        other_run.set_string("pitchfork", &expected).unwrap();
        assert!(manager.is_enabled().unwrap());
        manager.disable().unwrap();
        assert!(run.get_string("pitchfork").is_err());
        assert!(other_run.get_string("pitchfork").is_err());
        assert!(!manager.is_enabled().unwrap());

        run.set_string("pitchfork", &expected).unwrap();
        other_run.set_string("pitchfork", &expected).unwrap();
        approval
            .set_bytes("pitchfork", Type::Bytes, &disabled)
            .unwrap();
        assert!(!manager.is_enabled().unwrap());
        manager.check_and_reregister_if_stale();
        assert!(!manager.is_enabled().unwrap());
        assert_eq!(approval.get_value("pitchfork").unwrap().as_ref(), disabled);
        assert_eq!(run.get_string("pitchfork").unwrap(), expected);
        assert_eq!(
            other_approval.get_value("pitchfork").unwrap().as_ref(),
            disabled
        );

        // A missing approval value allows its own Run entry, even while the
        // other registration remains disabled.
        other_approval.remove_value("pitchfork").unwrap();
        assert!(manager.is_enabled().unwrap());
        run.remove_value("pitchfork").unwrap();
        assert!(manager.is_enabled().unwrap());
        other_run.remove_value("pitchfork").unwrap();
        assert!(!manager.is_enabled().unwrap());
        approval.remove_value("pitchfork").unwrap();
    }

    // Repairing a disabled, stale registration preserves approval byte-for-byte
    // and does not create an entry in the other root, even when both are writable.
    for (root, other) in [(&registry.machine, &user_key), (&registry.user, &key)] {
        let run = root.create(RUN).unwrap();
        run.set_string("pitchfork", stale).unwrap();
        let approval = root.create(STARTUP_APPROVED).unwrap();
        approval
            .set_bytes("pitchfork", Type::Bytes, &disabled)
            .unwrap();
        assert!(!manager.is_enabled().unwrap());
        manager.check_and_reregister_if_stale();
        assert!(!manager.is_enabled().unwrap());
        assert_eq!(approval.get_value("pitchfork").unwrap().as_ref(), disabled);
        assert_eq!(run.get_string("pitchfork").unwrap(), expected);
        assert!(manager.is_current_level_up_to_date().unwrap());
        manager.check_and_reregister_if_stale();
        assert_eq!(approval.get_value("pitchfork").unwrap().as_ref(), disabled);
        assert!(other.get_string("pitchfork").is_err());
        // Only an explicit enable may restore Task Manager approval.
        manager.enable().unwrap();
        let value = approval.get_value("pitchfork").unwrap();
        assert_eq!(value.ty(), Type::Bytes);
        assert_eq!(value.as_ref(), enabled);
        assert!(manager.is_enabled().unwrap());
        assert!(other.get_string("pitchfork").is_err());
        approval.remove_value("pitchfork").unwrap();
        assert!(manager.is_enabled().unwrap());
        run.remove_value("pitchfork").unwrap();
        assert!(!manager.is_enabled().unwrap());
    }

    // An unwritable existing HKLM entry must remain stale without creating a
    // second entry in HKCU. Explicit updates must explain the elevation needed.
    key.set_string("pitchfork", stale).unwrap();
    {
        let _denied = DenyRegistryWrites::new(&registry.machine, RUN);
        assert!(!manager.is_current_level_up_to_date().unwrap());
        manager.check_and_reregister_if_stale();
        for update in [BootManager::enable, BootManager::refresh] {
            let error = update(&manager).unwrap_err().to_string();
            assert!(error.contains("HKLM"), "{error}");
            assert!(error.contains("elevated terminal"), "{error}");
        }
        assert_eq!(key.get_string("pitchfork").unwrap(), stale);
        assert!(user_key.get_string("pitchfork").is_err());
    }
    manager.check_and_reregister_if_stale();
    assert_eq!(key.get_string("pitchfork").unwrap(), expected);

    // For a new registration, access-denied fallback to HKCU still works.
    key.remove_value("pitchfork").unwrap();
    {
        let _denied = DenyRegistryWrites::new(&registry.machine, RUN);
        manager.enable().unwrap();
        assert_eq!(user_key.get_string("pitchfork").unwrap(), expected);
        assert!(key.get_string("pitchfork").is_err());
    }
    manager.disable().unwrap();

    // Approval can fail after a new Run value was already written. Preserve
    // that entry and report the failure instead of duplicating it under HKCU.
    {
        let _denied = DenyRegistryWrites::new(&registry.machine, STARTUP_APPROVED);
        let error = manager.enable().unwrap_err().to_string();
        assert!(error.contains("HKLM"), "{error}");
        assert!(error.contains("elevated terminal"), "{error}");
        assert_eq!(key.get_string("pitchfork").unwrap(), expected);
        assert!(user_key.get_string("pitchfork").is_err());
    }
    manager.enable().unwrap();

    // Migrate a version-pinned entry and an old unquoted entry with spaces.
    for stale in [
        r"C:\removed\pitchfork.exe supervisor run --boot".to_string(),
        format!("{} supervisor run --boot", executable.display()),
    ] {
        key.set_string("pitchfork", stale).unwrap();
        assert!(!manager.is_current_level_up_to_date().unwrap());
        manager.check_and_reregister_if_stale();
        assert_eq!(key.get_string("pitchfork").unwrap(), expected);
    }
    // A read failure must not silently overwrite a malformed registration.
    key.set_string("pitchfork", "\"unterminated").unwrap();
    assert!(manager.is_current_level_up_to_date().is_err());
    manager.check_and_reregister_if_stale();
    assert_eq!(key.get_string("pitchfork").unwrap(), "\"unterminated");
    manager.refresh().unwrap();

    unsafe {
        std::env::set_var("PITCHFORK_BOOT_EXECUTABLE", temp.path().join("missing.exe"));
    }
    pitchfork_cli::settings::reload_settings();
    let invalid = BootManager::new().unwrap();
    assert!(invalid.enable().is_err());
    assert!(invalid.refresh().is_err());
    assert_eq!(key.get_string("pitchfork").unwrap(), expected);
    // Invalid configuration must not prevent status or disable.
    assert!(invalid.is_enabled().unwrap());
    invalid.disable().unwrap();
    invalid.disable().unwrap();
    assert!(!invalid.is_enabled().unwrap());
    assert!(key.get_string("pitchfork").is_err());
    manager.check_and_reregister_if_stale();
    assert!(key.get_string("pitchfork").is_err());
    assert!(!manager.is_current_level_up_to_date().unwrap());
    drop(key);
    // Windows cannot remove a temporary directory while it is the CWD.
    std::env::set_current_dir(std::env::temp_dir()).unwrap();
}
