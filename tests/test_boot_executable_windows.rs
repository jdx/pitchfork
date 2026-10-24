//! Boot registration against private registry hives. RegOverridePredefKey is
//! process-local, so no real login registration is created or removed.
//! This binary has one test because registry overrides and settings are global.
#![cfg(windows)]

use pitchfork_cli::boot_manager::BootManager;
use std::io::Write;
use std::path::{Path, PathBuf};
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
const STALE_COMMAND: &str = r#""C:\removed\pitchfork.exe" supervisor run --boot"#;
const ENABLED: [u8; 12] = [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
const DISABLED: [u8; 12] = [0x03, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];

fn report_cleanup_errors(errors: &[String]) {
    if errors.is_empty() {
        return;
    }
    let message = errors.join("\n");
    if std::thread::panicking() {
        // A failed diagnostic write must not interrupt remaining cleanup.
        let _ = writeln!(std::io::stderr(), "fixture cleanup failed:\n{message}");
    } else {
        panic!("fixture cleanup failed:\n{message}");
    }
}

struct WorkingDirectory(PathBuf);

impl WorkingDirectory {
    fn enter(path: &Path) -> Self {
        let guard = Self(std::env::current_dir().unwrap());
        std::env::set_current_dir(path).unwrap();
        guard
    }
}

impl Drop for WorkingDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::env::set_current_dir(&self.0) {
            report_cleanup_errors(&[format!("restore working directory: {error}")]);
        }
    }
}

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
        let roots = (|| -> windows_registry::Result<_> {
            Ok((
                software.create(format!(r"{name}\user"))?,
                software.create(format!(r"{name}\machine"))?,
            ))
        })();
        let (user, machine) = match roots {
            Ok(roots) => roots,
            Err(error) => {
                let cleanup = software.remove_tree(&name);
                panic!("create private registry hives: {error}; cleanup: {cleanup:?}");
            }
        };
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

    fn clear(&self) {
        for root in [&self.user, &self.machine] {
            let names = root.keys().unwrap().collect::<Vec<_>>();
            for name in names {
                root.remove_tree(name).unwrap();
            }
        }
    }
}

impl Drop for RegistrySandbox {
    fn drop(&mut self) {
        let mut errors = Vec::new();
        // Attempt both resets and removal before reporting any cleanup failure.
        for (name, root) in [("HKLM", HKEY_LOCAL_MACHINE), ("HKCU", HKEY_CURRENT_USER)] {
            let result = unsafe { RegOverridePredefKey(root, std::ptr::null_mut()) };
            if result != 0 {
                errors.push(format!(
                    "restore {name}: {}",
                    std::io::Error::from_raw_os_error(result as i32)
                ));
            }
        }
        if let Err(error) = self.software.remove_tree(&self.name) {
            errors.push(format!("remove private registry keys: {error}"));
        }
        report_cleanup_errors(&errors);
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
        let result = unsafe {
            RegSetKeySecurity(
                self.key.as_raw(),
                DACL_SECURITY_INFORMATION,
                self.original.as_mut_ptr().cast(),
            )
        };
        if result != 0 {
            report_cleanup_errors(&[format!(
                "restore registry permissions: {}",
                std::io::Error::from_raw_os_error(result as i32)
            )]);
        }
    }
}

fn assert_registry_cleanup_on_unwind() {
    let software = CURRENT_USER.open("Software").unwrap();
    let mut name = String::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let registry = RegistrySandbox::new();
        name = registry.name.clone();
        panic!("registry cleanup probe");
    }));
    assert_eq!(
        result.unwrap_err().downcast_ref::<&str>().copied(),
        Some("registry cleanup probe")
    );
    assert!(software.open(name).is_err());
    // Neither empty sandbox root has Software: these reads require restored roots.
    CURRENT_USER.open("Software").unwrap();
    windows_registry::LOCAL_MACHINE.open("Software").unwrap();
}

fn assert_permission_and_cwd_cleanup_on_unwind(registry: &RegistrySandbox) {
    let original_dir = std::env::current_dir().unwrap();
    let temporary_dir = original_dir.join("cleanup probe");
    std::fs::create_dir(&temporary_dir).unwrap();
    registry.machine.create(RUN).unwrap();
    let result = std::panic::catch_unwind(|| {
        let _cwd = WorkingDirectory::enter(&temporary_dir);
        let _denied = DenyRegistryWrites::new(&registry.machine, RUN);
        assert!(registry.machine.options().write().open(RUN).is_err());
        panic!("permission and cwd cleanup probe");
    });
    assert_eq!(
        result.unwrap_err().downcast_ref::<&str>().copied(),
        Some("permission and cwd cleanup probe")
    );
    assert_eq!(std::env::current_dir().unwrap(), original_dir);
    registry
        .machine
        .create(RUN)
        .unwrap()
        .set_string("pitchfork", "restored")
        .unwrap();
}

#[test]
fn windows_boot_registration_round_trips_spaces_unicode_and_repairs() {
    assert_registry_cleanup_on_unwind();
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
    let _cwd = WorkingDirectory::enter(temp.path());
    let manager = BootManager::new().unwrap();
    let expected = format!("\"{}\" supervisor run --boot", executable.display());
    assert_permission_and_cwd_cleanup_on_unwind(&registry);
    let scenarios: &[fn(&RegistrySandbox, &BootManager, &str)] = &[
        round_trips_and_repeated_enable_is_idempotent,
        updates_preserve_a_user_registration,
        machine_registration_takes_precedence,
        approval_applies_only_to_its_own_root,
        repair_preserves_disabled_approval,
        permission_failures_do_not_duplicate_registrations,
        legacy_commands_are_repaired_and_malformed_commands_are_preserved,
    ];
    for scenario in scenarios {
        registry.clear();
        scenario(&registry, &manager, &expected);
    }
    registry.clear();
    invalid_executable_does_not_block_status_or_disable(
        &registry,
        &manager,
        &expected,
        temp.path(),
    );
}

fn round_trips_and_repeated_enable_is_idempotent(
    registry: &RegistrySandbox,
    manager: &BootManager,
    expected: &str,
) {
    assert!(!manager.is_current_level_up_to_date().unwrap());
    manager.check_and_reregister_if_stale();
    assert!(!manager.is_enabled().unwrap());
    manager.enable().unwrap();
    let key = registry.machine.open(RUN).unwrap();
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
}

fn updates_preserve_a_user_registration(
    registry: &RegistrySandbox,
    manager: &BootManager,
    expected: &str,
) {
    let key = registry.machine.create(RUN).unwrap();
    // A user-only registration must be readable and removable without creating
    // a machine entry during an automatic check.
    let user_key = registry.user.create(RUN).unwrap();
    user_key.set_string("pitchfork", expected).unwrap();
    assert!(manager.is_enabled().unwrap());
    assert!(manager.is_current_level_up_to_date().unwrap());
    manager.check_and_reregister_if_stale();
    assert!(key.get_string("pitchfork").is_err());
    // Explicit updates must not move a user entry to HKLM even when HKLM is
    // writable. Automatic repair must retain the same root as well.
    for update in [BootManager::enable, BootManager::refresh] {
        user_key.set_string("pitchfork", STALE_COMMAND).unwrap();
        update(manager).unwrap();
        assert_eq!(user_key.get_string("pitchfork").unwrap(), expected);
        assert!(key.get_string("pitchfork").is_err());
    }
    user_key.set_string("pitchfork", STALE_COMMAND).unwrap();
    manager.check_and_reregister_if_stale();
    assert_eq!(user_key.get_string("pitchfork").unwrap(), expected);
    assert!(key.get_string("pitchfork").is_err());
    manager.disable().unwrap();
    assert!(!manager.is_enabled().unwrap());
    assert!(user_key.get_string("pitchfork").is_err());
}

fn machine_registration_takes_precedence(
    registry: &RegistrySandbox,
    manager: &BootManager,
    expected: &str,
) {
    let key = registry.machine.create(RUN).unwrap();
    let user_key = registry.user.create(RUN).unwrap();
    // The machine entry takes precedence when both roots have registrations,
    // regardless of which root has the selected executable. Disable clears both.
    key.set_string("pitchfork", STALE_COMMAND).unwrap();
    user_key.set_string("pitchfork", expected).unwrap();
    assert!(!manager.is_current_level_up_to_date().unwrap());
    manager.check_and_reregister_if_stale();
    assert_eq!(key.get_string("pitchfork").unwrap(), expected);
    assert_eq!(user_key.get_string("pitchfork").unwrap(), expected);
    user_key.set_string("pitchfork", STALE_COMMAND).unwrap();
    assert!(manager.is_current_level_up_to_date().unwrap());
    manager.enable().unwrap();
    manager.refresh().unwrap();
    manager.check_and_reregister_if_stale();
    assert_eq!(key.get_string("pitchfork").unwrap(), expected);
    assert_eq!(user_key.get_string("pitchfork").unwrap(), STALE_COMMAND);
    manager.disable().unwrap();
    assert!(key.get_string("pitchfork").is_err());
    assert!(user_key.get_string("pitchfork").is_err());
    manager.enable().unwrap();
    assert_eq!(key.get_string("pitchfork").unwrap(), expected);
}

fn approval_applies_only_to_its_own_root(
    registry: &RegistrySandbox,
    manager: &BootManager,
    expected: &str,
) {
    // An approval record affects only the Run entry in its own root. Exercise
    // both directions so an orphaned disabled record cannot hide an enabled
    // entry, and mixed approval states still allow status and disable to work.
    for (root, other_root) in [
        (&registry.machine, &registry.user),
        (&registry.user, &registry.machine),
    ] {
        let run = root.create(RUN).unwrap();
        let other_run = other_root.create(RUN).unwrap();
        let approval = root.create(STARTUP_APPROVED).unwrap();
        let other_approval = other_root.create(STARTUP_APPROVED).unwrap();
        run.set_string("pitchfork", expected).unwrap();
        approval
            .set_bytes("pitchfork", Type::Bytes, &ENABLED)
            .unwrap();
        other_approval
            .set_bytes("pitchfork", Type::Bytes, &DISABLED)
            .unwrap();
        assert!(manager.is_enabled().unwrap());
        assert!(manager.is_current_level_enabled().unwrap());
        manager.check_and_reregister_if_stale();
        assert_eq!(approval.get_value("pitchfork").unwrap().as_ref(), ENABLED);
        assert_eq!(
            other_approval.get_value("pitchfork").unwrap().as_ref(),
            DISABLED
        );

        other_run.set_string("pitchfork", expected).unwrap();
        assert!(manager.is_enabled().unwrap());
        manager.disable().unwrap();
        assert!(run.get_string("pitchfork").is_err());
        assert!(other_run.get_string("pitchfork").is_err());
        assert!(!manager.is_enabled().unwrap());

        run.set_string("pitchfork", expected).unwrap();
        other_run.set_string("pitchfork", expected).unwrap();
        approval
            .set_bytes("pitchfork", Type::Bytes, &DISABLED)
            .unwrap();
        assert!(!manager.is_enabled().unwrap());
        manager.check_and_reregister_if_stale();
        assert!(!manager.is_enabled().unwrap());
        assert_eq!(approval.get_value("pitchfork").unwrap().as_ref(), DISABLED);
        assert_eq!(run.get_string("pitchfork").unwrap(), expected);
        assert_eq!(
            other_approval.get_value("pitchfork").unwrap().as_ref(),
            DISABLED
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
}

fn repair_preserves_disabled_approval(
    registry: &RegistrySandbox,
    manager: &BootManager,
    expected: &str,
) {
    let key = registry.machine.create(RUN).unwrap();
    let user_key = registry.user.create(RUN).unwrap();
    // Repairing a disabled, stale registration preserves approval byte-for-byte
    // and does not create an entry in the other root, even when both are writable.
    for (root, other) in [(&registry.machine, &user_key), (&registry.user, &key)] {
        let run = root.create(RUN).unwrap();
        run.set_string("pitchfork", STALE_COMMAND).unwrap();
        let approval = root.create(STARTUP_APPROVED).unwrap();
        approval
            .set_bytes("pitchfork", Type::Bytes, &DISABLED)
            .unwrap();
        assert!(!manager.is_enabled().unwrap());
        manager.check_and_reregister_if_stale();
        assert!(!manager.is_enabled().unwrap());
        assert_eq!(approval.get_value("pitchfork").unwrap().as_ref(), DISABLED);
        assert_eq!(run.get_string("pitchfork").unwrap(), expected);
        assert!(manager.is_current_level_up_to_date().unwrap());
        manager.check_and_reregister_if_stale();
        assert_eq!(approval.get_value("pitchfork").unwrap().as_ref(), DISABLED);
        assert!(other.get_string("pitchfork").is_err());
        // Only an explicit enable may restore Task Manager approval.
        manager.enable().unwrap();
        let value = approval.get_value("pitchfork").unwrap();
        assert_eq!(value.ty(), Type::Bytes);
        assert_eq!(value.as_ref(), ENABLED);
        assert!(manager.is_enabled().unwrap());
        assert!(other.get_string("pitchfork").is_err());
        approval.remove_value("pitchfork").unwrap();
        assert!(manager.is_enabled().unwrap());
        run.remove_value("pitchfork").unwrap();
        assert!(!manager.is_enabled().unwrap());
    }
}

fn permission_failures_do_not_duplicate_registrations(
    registry: &RegistrySandbox,
    manager: &BootManager,
    expected: &str,
) {
    let key = registry.machine.create(RUN).unwrap();
    let user_key = registry.user.create(RUN).unwrap();
    registry.machine.create(STARTUP_APPROVED).unwrap();
    // An unwritable existing HKLM entry must remain stale without creating a
    // second entry in HKCU. Explicit updates must explain the elevation needed.
    key.set_string("pitchfork", STALE_COMMAND).unwrap();
    {
        let _denied = DenyRegistryWrites::new(&registry.machine, RUN);
        assert!(!manager.is_current_level_up_to_date().unwrap());
        manager.check_and_reregister_if_stale();
        for update in [BootManager::enable, BootManager::refresh] {
            let error = update(manager).unwrap_err().to_string();
            assert!(error.contains("HKLM"), "{error}");
            assert!(error.contains("elevated terminal"), "{error}");
        }
        assert_eq!(key.get_string("pitchfork").unwrap(), STALE_COMMAND);
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
}

fn legacy_commands_are_repaired_and_malformed_commands_are_preserved(
    registry: &RegistrySandbox,
    manager: &BootManager,
    expected: &str,
) {
    let key = registry.machine.create(RUN).unwrap();
    // Migrate a version-pinned entry and an old unquoted entry with spaces.
    for stale in [
        r"C:\removed\pitchfork.exe supervisor run --boot".to_string(),
        expected.replace('"', ""),
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
}

fn invalid_executable_does_not_block_status_or_disable(
    registry: &RegistrySandbox,
    manager: &BootManager,
    expected: &str,
    root: &Path,
) {
    let key = registry.machine.create(RUN).unwrap();
    key.set_string("pitchfork", expected).unwrap();
    unsafe {
        std::env::set_var("PITCHFORK_BOOT_EXECUTABLE", root.join("missing.exe"));
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
}
