// ─── Supported platforms (macOS, Linux, Windows) ──────────────────────────

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
mod imp {
    use crate::{Result, env};
    #[cfg(target_os = "linux")]
    use auto_launcher::LinuxLaunchMode;
    #[cfg(target_os = "macos")]
    use auto_launcher::MacOSLaunchMode;
    use auto_launcher::{AutoLaunch, AutoLaunchBuilder};
    use miette::IntoDiagnostic;

    /// Arguments of the registered `pitchfork` command.
    ///
    /// A system registration made through sudo records the invoking user, since
    /// launchd and systemd start the service without the sudo environment.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(crate) fn service_args(invoking_user: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = ["supervisor", "run", "--boot"]
            .into_iter()
            .map(String::from)
            .collect();
        if let Some(user) = invoking_user {
            args.push(env::INVOKING_USER_FLAG.to_string());
            args.push(user.to_string());
        }
        args
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn build_launcher(
        app_path: &str,
        args: &[String],
        #[cfg(target_os = "macos")] macos_mode: MacOSLaunchMode,
        #[cfg(target_os = "linux")] linux_mode: LinuxLaunchMode,
    ) -> Result<AutoLaunch> {
        let mut builder = AutoLaunchBuilder::new();
        builder
            .set_app_name("pitchfork")
            .set_app_path(app_path)
            .set_args(args);

        #[cfg(target_os = "macos")]
        builder.set_macos_launch_mode(macos_mode);

        #[cfg(target_os = "linux")]
        builder.set_linux_launch_mode(linux_mode);

        builder.build().into_diagnostic()
    }

    pub struct BootManager {
        /// Literal executable spelling selected for boot registration only.
        app_path: String,
        explicit_executable: bool,
        /// User recorded in the system registration, if any.
        invoking_user: Option<String>,
        /// The launcher matching the current privilege level (used for enable).
        current: AutoLaunch,
        /// The other level's launcher (used to detect cross-level registrations).
        other: AutoLaunch,
        /// Legacy macOS LaunchAgentSystem entry (pre-1.0.3 used /Library/LaunchAgents/
        /// instead of /Library/LaunchDaemons/ for root). Kept only for migration/cleanup.
        #[cfg(target_os = "macos")]
        legacy: AutoLaunch,
    }

    /// Where the system-level registration lives.
    #[cfg(target_os = "macos")]
    const SYSTEM_REGISTRATION: &str = "/Library/LaunchDaemons/pitchfork.plist";
    #[cfg(target_os = "linux")]
    const SYSTEM_REGISTRATION: &str = "/etc/systemd/system/pitchfork.service";

    /// The invoking user recorded in the system-level registration, if the
    /// registration exists, can be read, and records one.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn registered_system_invoking_user() -> Option<String> {
        let contents = match std::fs::read(SYSTEM_REGISTRATION) {
            Ok(contents) => contents,
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    warn!("failed to read {SYSTEM_REGISTRATION}: {err}");
                }
                return None;
            }
        };
        #[cfg(target_os = "macos")]
        let argv = super::launchd_program_arguments(&contents);
        #[cfg(target_os = "linux")]
        let argv = super::systemd_exec_start(&String::from_utf8_lossy(&contents));
        env::invoking_user_arg(argv?.into_iter().map(Into::into))
    }

    /// Write the launchd registration, then add the crash-restart keys that
    /// `auto-launcher` does not emit (the macOS counterpart of systemd's
    /// `Restart=on-failure`).
    fn write_registration(launcher: &AutoLaunch) -> Result<()> {
        // `enable` writes the plist anew, so a restart policy the user set on
        // the registration is read first and put back afterwards.
        #[cfg(target_os = "macos")]
        let path = current_plist_path()?;
        #[cfg(target_os = "macos")]
        let previous = std::fs::read(&path).ok();
        launcher.enable().into_diagnostic()?;
        #[cfg(target_os = "macos")]
        add_keep_alive(&path, previous.as_deref())?;
        Ok(())
    }

    /// The launchd plist `auto-launcher` writes at the current privilege level.
    #[cfg(target_os = "macos")]
    fn current_plist_path() -> Result<std::path::PathBuf> {
        if nix::unistd::Uid::effective().is_root() {
            return Ok(SYSTEM_REGISTRATION.into());
        }
        let home =
            dirs::home_dir().ok_or_else(|| miette::miette!("failed to find home directory"))?;
        Ok(home.join("Library/LaunchAgents/pitchfork.plist"))
    }

    #[cfg(target_os = "macos")]
    fn add_keep_alive(path: &std::path::Path, previous: Option<&[u8]>) -> Result<()> {
        let mut contents = std::fs::read(path).into_diagnostic()?;
        if let Some(restored) =
            previous.and_then(|p| super::launchd_with_restart_policy_of(&contents, p))
        {
            contents = restored;
            std::fs::write(path, &contents).into_diagnostic()?;
        }
        if let Some(updated) = super::launchd_with_keep_alive(&contents) {
            std::fs::write(path, updated).into_diagnostic()?;
        }
        Ok(())
    }

    impl BootManager {
        /// Manager whose current-level registration records the user this
        /// process acts on behalf of (see [`env::boot_service_invoking_user`]).
        pub fn new() -> Result<Self> {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            return Self::with_invoking_user(env::boot_service_invoking_user()?);
            #[cfg(windows)]
            Self::with_invoking_user(None)
        }

        /// Manager whose current-level registration records `invoking_user`.
        /// Only the current level's registration is ever written, and for root
        /// that is the system registration.
        fn with_invoking_user(invoking_user: Option<String>) -> Result<Self> {
            let configured = crate::settings::settings().boot.executable.clone();
            let explicit_executable = !configured.is_empty();
            let app_path = if configured.is_empty() {
                env::PITCHFORK_BIN.to_string_lossy().to_string()
            } else {
                configured
            };

            #[cfg(any(target_os = "macos", target_os = "linux"))]
            let (current_args, other_args) =
                (service_args(invoking_user.as_deref()), service_args(None));

            #[cfg(target_os = "macos")]
            let (current, other, legacy) = {
                let is_root = nix::unistd::Uid::effective().is_root();
                let (current_mode, other_mode) = if is_root {
                    (
                        MacOSLaunchMode::LaunchDaemonSystem,
                        MacOSLaunchMode::LaunchAgentUser,
                    )
                } else {
                    (
                        MacOSLaunchMode::LaunchAgentUser,
                        MacOSLaunchMode::LaunchDaemonSystem,
                    )
                };
                (
                    build_launcher(&app_path, &current_args, current_mode)?,
                    build_launcher(&app_path, &other_args, other_mode)?,
                    build_launcher(&app_path, &other_args, MacOSLaunchMode::LaunchAgentSystem)?,
                )
            };

            #[cfg(target_os = "linux")]
            let (current, other) = {
                let is_root = nix::unistd::Uid::effective().is_root();
                let (current_mode, other_mode) = if is_root {
                    (LinuxLaunchMode::SystemdSystem, LinuxLaunchMode::SystemdUser)
                } else {
                    (LinuxLaunchMode::SystemdUser, LinuxLaunchMode::SystemdSystem)
                };
                (
                    build_launcher(&app_path, &current_args, current_mode)?,
                    build_launcher(&app_path, &other_args, other_mode)?,
                )
            };

            // On Windows there is no root/user distinction; build two identical
            // launchers (AutoLaunch does not implement Clone).
            #[cfg(windows)]
            let (current, other) = (
                AutoLaunchBuilder::new()
                    .set_app_name("pitchfork")
                    .set_app_path(&app_path)
                    .set_args(&["supervisor", "run", "--boot"])
                    .build()
                    .into_diagnostic()?,
                AutoLaunchBuilder::new()
                    .set_app_name("pitchfork")
                    .set_app_path(&app_path)
                    .set_args(&["supervisor", "run", "--boot"])
                    .build()
                    .into_diagnostic()?,
            );

            #[cfg(target_os = "macos")]
            return Ok(Self {
                app_path,
                explicit_executable,
                invoking_user,
                current,
                other,
                legacy,
            });

            #[cfg(not(target_os = "macos"))]
            Ok(Self {
                app_path,
                explicit_executable,
                invoking_user,
                current,
                other,
            })
        }

        /// Validate only explicit choices, and only before registration work.
        /// Status/disable must remain usable when an executable has disappeared.
        fn validate_executable(&self) -> Result<()> {
            if !self.explicit_executable {
                return Ok(());
            }
            let invalid = |reason: &str| {
                miette::miette!(
                    "settings.boot.executable '{}': {reason}; set an absolute path to an \
                     existing executable, or unset the setting to use the running binary",
                    self.app_path
                )
            };
            let path = std::path::Path::new(&self.app_path);
            if !path.is_absolute() {
                return Err(invalid(
                    "path must be absolute (no PATH or tilde expansion)",
                ));
            }
            if self.app_path.chars().any(char::is_control) {
                return Err(invalid("path must not contain control characters"));
            }
            // auto-launcher writes an unquoted command and reads the first
            // whitespace-delimited token on these platforms. Reject paths it
            // cannot round-trip instead of writing a broken registration.
            #[cfg(any(target_os = "linux", windows))]
            if self.app_path.chars().any(char::is_whitespace) {
                return Err(invalid(
                    "boot registration does not support whitespace in paths on this platform",
                ));
            }
            #[cfg(target_os = "linux")]
            if self.app_path.contains(['\"', '\'', '\\', '%', '$']) {
                return Err(invalid(
                    "boot registration does not support quotes, backslashes, percent signs or dollar signs in systemd executable paths",
                ));
            }
            let metadata = std::fs::metadata(path)
                .map_err(|e| invalid(&format!("cannot access executable: {e}")))?;
            if !metadata.is_file() {
                return Err(invalid("path is not a regular file"));
            }
            #[cfg(unix)]
            {
                let path = std::ffi::CString::new(self.app_path.as_bytes())
                    .map_err(|_| invalid("path contains a NUL byte"))?;
                // SAFETY: path is a valid, NUL-terminated string. access only
                // checks permissions; it does not run the configured program.
                if unsafe { libc::access(path.as_ptr(), libc::X_OK) } != 0 {
                    return Err(invalid("file is not executable by this user"));
                }
            }
            Ok(())
        }

        /// User recorded in the registration written at the current level.
        pub fn invoking_user(&self) -> Option<&str> {
            self.invoking_user.as_deref()
        }

        /// Whether the system-level registration exists.
        pub fn is_system_level_enabled(&self) -> Result<bool> {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            let system = if nix::unistd::Uid::effective().is_root() {
                &self.current
            } else {
                &self.other
            };
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            return system.is_enabled().into_diagnostic();
            #[cfg(windows)]
            Ok(false)
        }

        /// The invoking user recorded in the existing system-level
        /// registration. Readable without root.
        pub fn system_invoking_user(&self) -> Option<String> {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            return registered_system_invoking_user();
            #[cfg(windows)]
            None
        }

        /// Whether the current-level registration already has the selected
        /// boot executable and invoking user, so `enable` has nothing to change.
        pub fn is_current_level_up_to_date(&self) -> Result<bool> {
            self.validate_executable()?;
            let registered = self.current.get_registered_app_path().into_diagnostic()?;
            if registered.as_deref() != Some(self.app_path.as_str()) {
                return Ok(false);
            }
            // Registrations written before crash-restart was added lack it.
            #[cfg(target_os = "macos")]
            {
                let plist = std::fs::read(current_plist_path()?).into_diagnostic()?;
                if super::launchd_with_keep_alive(&plist).is_some() {
                    return Ok(false);
                }
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            if nix::unistd::Uid::effective().is_root() {
                return Ok(registered_system_invoking_user() == self.invoking_user);
            }
            Ok(true)
        }

        /// Whether any registration (user- or system-level) exists.
        pub fn is_enabled(&self) -> Result<bool> {
            #[cfg(target_os = "macos")]
            return Ok(self.current.is_enabled().into_diagnostic()?
                || self.other.is_enabled().into_diagnostic()?
                || self.legacy.is_enabled().into_diagnostic()?);

            #[cfg(not(target_os = "macos"))]
            Ok(self.current.is_enabled().into_diagnostic()?
                || self.other.is_enabled().into_diagnostic()?)
        }

        /// Whether a registration at the *current* privilege level exists.
        pub fn is_current_level_enabled(&self) -> Result<bool> {
            self.current.is_enabled().into_diagnostic()
        }

        /// Whether a registration at the *other* privilege level exists.
        /// Used to warn the user about cross-level mismatches.
        /// On macOS, includes legacy entries for non-root callers (they are at a
        /// different privilege level) but not for root callers (legacy is same level).
        pub fn is_other_level_enabled(&self) -> Result<bool> {
            #[cfg(target_os = "macos")]
            return Ok(self.other.is_enabled().into_diagnostic()?
                || (!nix::unistd::Uid::effective().is_root()
                    && self.legacy.is_enabled().into_diagnostic()?));

            #[cfg(not(target_os = "macos"))]
            self.other.is_enabled().into_diagnostic()
        }

        /// Remove legacy macOS LaunchAgentSystem entry if present and caller is root.
        /// Idempotent — safe to call on every enable path, including retries after
        /// partial migration (new entry written but legacy removal failed).
        ///
        /// `migrated`: true when called after writing a new LaunchDaemonSystem entry
        /// (full migration); false when just removing a stale leftover.
        #[cfg(target_os = "macos")]
        pub fn cleanup_legacy(&self, migrated: bool) -> Result<()> {
            if nix::unistd::Uid::effective().is_root()
                && self.legacy.is_enabled().into_diagnostic()?
            {
                self.legacy.disable().into_diagnostic()?;
                if migrated {
                    info!(
                        "migrated legacy system-level launch entry from /Library/LaunchAgents/ to /Library/LaunchDaemons/"
                    );
                } else {
                    info!("removed legacy system-level launch entry from /Library/LaunchAgents/");
                }
            }
            Ok(())
        }

        /// Register at the current privilege level.
        ///
        /// Returns an error if a registration at the other privilege level already
        /// exists, preventing user-level and system-level entries from coexisting.
        ///
        /// On macOS, migrates any legacy LaunchAgentSystem entry (from pre-1.0.3)
        /// to the correct LaunchDaemonSystem entry.
        pub fn enable(&self) -> Result<()> {
            self.validate_executable()?;
            // For root, legacy will be migrated so only check non-legacy other level.
            // For non-root, legacy cannot be migrated and is also a conflict.
            #[cfg(target_os = "macos")]
            let other_conflict = if nix::unistd::Uid::effective().is_root() {
                self.other.is_enabled().into_diagnostic()?
            } else {
                self.is_other_level_enabled()?
            };

            #[cfg(not(target_os = "macos"))]
            let other_conflict = self.other.is_enabled().into_diagnostic()?;

            if other_conflict {
                miette::bail!(
                    "boot start is already registered at the other privilege level; \
                    run `pitchfork boot disable` (with appropriate privileges) to remove \
                    it first"
                );
            }

            write_registration(&self.current)?;

            #[cfg(target_os = "macos")]
            self.cleanup_legacy(true)?;

            Ok(())
        }

        /// Rewrite the existing registration at the current privilege level,
        /// updating its binary path and recorded invoking user.
        pub fn refresh(&self) -> Result<()> {
            self.validate_executable()?;
            write_registration(&self.current)?;

            #[cfg(target_os = "macos")]
            self.cleanup_legacy(false)?;

            Ok(())
        }

        /// Remove registrations at *both* levels so cross-level leftovers are also
        /// cleaned up. Also removes legacy macOS LaunchAgentSystem entries when
        /// running as root. Returns Ok even if some entries could not be removed
        /// due to insufficient privileges — callers should check is_enabled()
        /// afterwards to detect incomplete cleanup.
        pub fn disable(&self) -> Result<()> {
            if self.current.is_enabled().into_diagnostic()? {
                self.current.disable().into_diagnostic()?;
            }
            if self.other.is_enabled().into_diagnostic()? {
                self.other.disable().into_diagnostic()?;
            }
            #[cfg(target_os = "macos")]
            if nix::unistd::Uid::effective().is_root()
                && self.legacy.is_enabled().into_diagnostic()?
            {
                self.legacy.disable().into_diagnostic()?;
            }
            Ok(())
        }

        /// Check whether the registered boot binary path matches the selected
        /// boot executable. If stale (binary moved after a package-manager upgrade),
        /// re-register at the current privilege level so the next boot uses the
        /// correct path.
        ///
        /// This is a no-op when boot start is not enabled, or when the registered
        /// path already matches. Errors are logged and swallowed — this is a
        /// best-effort self-heal that must not block supervisor startup.
        pub fn check_and_reregister_if_stale(&self) {
            if let Err(e) = self.validate_executable() {
                warn!("cannot repair boot registration: {e}");
                return;
            }
            let current_bin = &self.app_path;

            let registered = match self.current.get_registered_app_path() {
                Ok(Some(path)) => path,
                Ok(None) => return, // not registered, nothing to do
                Err(e) => {
                    warn!("failed to read registered boot path: {e}");
                    return;
                }
            };

            if registered == *current_bin {
                return; // path matches, all good
            }

            info!(
                "boot registration points to stale binary path '{registered}', \
                re-registering with current path '{current_bin}'"
            );

            // Keep the invoking user the registration already records: this
            // runs in whatever supervisor happens to start first, which must
            // neither add a user to a root-shell registration nor drop one.
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            let preserved = if nix::unistd::Uid::effective().is_root() {
                let registered = registered_system_invoking_user();
                if registered == self.invoking_user {
                    None
                } else {
                    match Self::with_invoking_user(registered) {
                        Ok(manager) => Some(manager),
                        Err(e) => {
                            warn!("failed to prepare boot re-registration: {e}");
                            return;
                        }
                    }
                }
            } else {
                None
            };
            #[cfg(windows)]
            let preserved: Option<Self> = None;
            let launcher = preserved.as_ref().map_or(&self.current, |m| &m.current);

            // Re-register by overwriting the existing registration file.
            // Calling enable() directly (without disable first) ensures that
            // if it fails, the stale registration is still present rather than
            // missing entirely — a stale path is better than no path.
            if let Err(e) = write_registration(launcher) {
                warn!("failed to re-register boot start with current path: {e}");
                return;
            }

            info!("boot registration updated to current binary path");
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    mod tests {
        use super::BootManager;
        use std::os::unix::fs::{PermissionsExt, symlink};

        #[test]
        fn refresh_preserves_executable_when_invoking_user_changes() {
            // Root selects /etc regardless of HOME; never exercise that route.
            assert!(
                !nix::unistd::Uid::effective().is_root(),
                "run this test as non-root"
            );
            if std::env::var_os("PITCHFORK_BOOT_METADATA_TEST_CHILD").is_none() {
                let home = tempfile::tempdir().unwrap();
                let bin = home.path().join("bin");
                std::fs::create_dir(&bin).unwrap();
                let systemctl = bin.join("systemctl");
                std::fs::write(&systemctl, "#!/bin/sh\ncase \"$*\" in\n'--user daemon-reload'|'--user enable pitchfork.service') exit 0 ;;\n*) exit 99 ;;\nesac\n").unwrap();
                std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
                symlink(std::env::current_exe().unwrap(), bin.join("stable")).unwrap();
                // Isolate settings/environment lazies from all other unit tests.
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "boot_manager::imp::tests::refresh_preserves_executable_when_invoking_user_changes", "--nocapture"])
                    .current_dir(home.path())
                    .env("HOME", home.path())
                    .env("PATH", &bin)
                    .env("PITCHFORK_CONFIG_DIR", home.path())
                    .env("PITCHFORK_BOOT_EXECUTABLE", bin.join("stable"))
                    .env("PITCHFORK_BOOT_METADATA_TEST_CHILD", "1")
                    .output().unwrap();
                assert!(
                    output.status.success(),
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                return;
            }

            // Use real BootManager registration with user-level fixture paths.
            // This covers metadata writing, not root privilege routing or its
            // automatic existing-account preservation branch.
            let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
            let stable = home.join("bin/stable");
            let unit = home.join(".config/systemd/user/pitchfork.service");
            for user in [Some("alice"), Some("bob"), None] {
                let manager = BootManager::with_invoking_user(user.map(String::from)).unwrap();
                manager.refresh().unwrap();
                let contents = std::fs::read_to_string(&unit).unwrap();
                let expected = match user {
                    Some(user) => format!(
                        "ExecStart={} supervisor run --boot --invoking-user {user}\n",
                        stable.display()
                    ),
                    None => format!("ExecStart={} supervisor run --boot\n", stable.display()),
                };
                assert!(contents.contains(&expected), "{contents}");
            }
        }
    }
}

// ─── Unsupported platforms ────────────────────────────────────────────────

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    use crate::Result;

    pub struct BootManager;

    impl BootManager {
        pub fn new() -> Result<Self> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            );
        }

        pub fn is_enabled(&self) -> Result<bool> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            );
        }

        pub fn is_current_level_enabled(&self) -> Result<bool> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            );
        }

        pub fn is_other_level_enabled(&self) -> Result<bool> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            );
        }

        pub fn enable(&self) -> Result<()> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            );
        }

        pub fn refresh(&self) -> Result<()> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            );
        }

        pub fn invoking_user(&self) -> Option<&str> {
            None
        }

        pub fn is_system_level_enabled(&self) -> Result<bool> {
            Ok(false)
        }

        pub fn system_invoking_user(&self) -> Option<String> {
            None
        }

        pub fn is_current_level_up_to_date(&self) -> Result<bool> {
            Ok(false)
        }

        pub fn disable(&self) -> Result<()> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            );
        }
    }
}

pub use imp::BootManager;

/// Command line of a systemd unit's `ExecStart=`, split as the unit was
/// written: pitchfork's service arguments never need quoting.
#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn systemd_exec_start(unit: &str) -> Option<Vec<String>> {
    unit.lines()
        .find_map(|line| line.trim().strip_prefix("ExecStart="))
        .map(|command| command.split_whitespace().map(String::from).collect())
}

/// `ProgramArguments` of a launchd plist.
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
fn launchd_program_arguments(plist: &[u8]) -> Option<Vec<String>> {
    let value = plist::Value::from_reader(std::io::Cursor::new(plist)).ok()?;
    value
        .as_dictionary()?
        .get("ProgramArguments")?
        .as_array()?
        .iter()
        .map(|arg| arg.as_string().map(String::from))
        .collect()
}

/// The launchd keys that make up a registration's restart policy.
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
const LAUNCHD_RESTART_KEYS: [&str; 2] = ["KeepAlive", "ThrottleInterval"];

/// `plist` with the restart policy (`KeepAlive`, `ThrottleInterval`) of
/// `previous`, the registration it replaces, or `None` when `previous` has
/// none to carry over (or either cannot be parsed). Rewriting a registration,
/// as repairing a stale executable path does, must not drop a policy the user
/// set on it.
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
fn launchd_with_restart_policy_of(plist: &[u8], previous: &[u8]) -> Option<Vec<u8>> {
    let previous = plist::Value::from_reader(std::io::Cursor::new(previous)).ok()?;
    let previous = previous.as_dictionary()?;
    let mut value = plist::Value::from_reader(std::io::Cursor::new(plist)).ok()?;
    let dict = value.as_dictionary_mut()?;
    let mut changed = false;
    for key in LAUNCHD_RESTART_KEYS {
        if let Some(policy) = previous.get(key)
            && dict.get(key) != Some(policy)
        {
            dict.insert(key.into(), policy.clone());
            changed = true;
        }
    }
    if !changed {
        return None;
    }
    let mut out = Vec::new();
    plist::to_writer_xml(&mut out, &value).ok()?;
    Some(out)
}

/// `plist` with launchd's crash-restart keys added, or `None` when it already
/// has a `KeepAlive` (or cannot be parsed). `SuccessfulExit = false` restarts
/// the supervisor after a crash or SIGKILL but not after a clean exit, so
/// `pitchfork supervisor stop` and `launchctl bootout` still stay stopped. The
/// 10s throttle matches the systemd unit's `RestartSec=10`.
///
/// Only a plist with no `KeepAlive` at all gets them: one written before
/// crash-restart was added. A `KeepAlive` of any other value, like a
/// `ThrottleInterval` already there, is local launchd policy the user set on
/// the registration, and is kept.
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
fn launchd_with_keep_alive(plist: &[u8]) -> Option<Vec<u8>> {
    let mut value = plist::Value::from_reader(std::io::Cursor::new(plist)).ok()?;
    let dict = value.as_dictionary_mut()?;
    if dict.contains_key("KeepAlive") {
        return None;
    }
    let mut keep_alive = plist::Dictionary::new();
    keep_alive.insert("SuccessfulExit".into(), plist::Value::Boolean(false));
    dict.insert("KeepAlive".into(), plist::Value::Dictionary(keep_alive));
    if !dict.contains_key("ThrottleInterval") {
        dict.insert("ThrottleInterval".into(), plist::Value::Integer(10.into()));
    }
    let mut out = Vec::new();
    plist::to_writer_xml(&mut out, &value).ok()?;
    Some(out)
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::imp::service_args;
    use super::{
        launchd_program_arguments, launchd_with_keep_alive, launchd_with_restart_policy_of,
        systemd_exec_start,
    };
    use crate::env::invoking_user_arg;

    fn argv(args: Option<Vec<String>>) -> Vec<std::ffi::OsString> {
        args.unwrap().into_iter().map(Into::into).collect()
    }

    /// The system unit as `sudo pitchfork boot enable` writes it on Linux.
    #[test]
    fn invoking_user_is_read_back_from_systemd_unit() {
        let unit = "[Unit]\nDescription=pitchfork\nAfter=multi-user.target\n\n\
            [Service]\nType=simple\n\
            ExecStart=/usr/local/bin/pitchfork supervisor run --boot --invoking-user alice\n\
            Restart=on-failure\n";
        let args = systemd_exec_start(unit);
        assert_eq!(invoking_user_arg(argv(args)).as_deref(), Some("alice"));

        let legacy = "[Service]\nExecStart=/usr/local/bin/pitchfork supervisor run --boot\n";
        assert_eq!(invoking_user_arg(argv(systemd_exec_start(legacy))), None);
        assert_eq!(systemd_exec_start("[Service]\n"), None);
    }

    /// The LaunchDaemon as `sudo pitchfork boot enable` writes it on macOS.
    #[test]
    fn invoking_user_is_read_back_from_launchd_plist() {
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>pitchfork</string>
	<key>ProgramArguments</key>
	<array>
		<string>/opt/homebrew/bin/pitchfork</string>
		<string>supervisor</string>
		<string>run</string>
		<string>--boot</string>
		<string>--invoking-user</string>
		<string>alice</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>SessionCreate</key>
	<true/>
</dict>
</plist>"#;
        let args = launchd_program_arguments(plist);
        assert_eq!(invoking_user_arg(argv(args)).as_deref(), Some("alice"));
        assert_eq!(launchd_program_arguments(b"not a plist"), None);
    }

    #[test]
    fn service_args_without_invoking_user_keep_plain_boot_command() {
        assert_eq!(service_args(None), ["supervisor", "run", "--boot"]);
    }

    #[test]
    fn service_args_record_invoking_user() {
        let args = service_args(Some("alice"));
        assert_eq!(
            args,
            ["supervisor", "run", "--boot", "--invoking-user", "alice"]
        );
        // The service command line must yield the same user when the
        // supervisor resolves paths from argv at startup.
        let argv = std::iter::once("pitchfork".to_string())
            .chain(args)
            .map(std::ffi::OsString::from);
        assert_eq!(
            crate::env::invoking_user_arg(argv).as_deref(),
            Some("alice")
        );
    }

    #[test]
    fn launchd_plist_gains_crash_restart_keys_once() {
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Label</key><string>pitchfork</string>
    <key>ProgramArguments</key><array><string>/bin/pitchfork</string><string>supervisor</string></array>
    <key>RunAtLoad</key><true/>
</dict>
</plist>"#;
        let updated = launchd_with_keep_alive(plist).unwrap();
        let value = plist::Value::from_reader(std::io::Cursor::new(&updated)).unwrap();
        let dict = value.as_dictionary().unwrap();
        let keep_alive = dict.get("KeepAlive").unwrap().as_dictionary().unwrap();
        assert_eq!(
            keep_alive.get("SuccessfulExit").unwrap().as_boolean(),
            Some(false)
        );
        assert_eq!(
            dict.get("ThrottleInterval").unwrap().as_signed_integer(),
            Some(10)
        );
        assert_eq!(dict.get("RunAtLoad").unwrap().as_boolean(), Some(true));
        assert_eq!(launchd_program_arguments(&updated).unwrap().len(), 2);
        assert_eq!(launchd_with_keep_alive(&updated), None);
        assert_eq!(launchd_with_keep_alive(b"not a plist"), None);
    }

    /// A rewritten registration, as `auto-launcher` writes it, takes back
    /// the restart policy of the one it replaces, and then needs no default.
    #[test]
    fn launchd_rewrite_keeps_the_previous_restart_policy() {
        let rewritten = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Label</key><string>pitchfork</string>
    <key>ProgramArguments</key><array><string>/new/pitchfork</string></array>
</dict>
</plist>"#;
        let previous = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Label</key><string>pitchfork</string>
    <key>ProgramArguments</key><array><string>/old/pitchfork</string></array>
    <key>KeepAlive</key><true/>
    <key>ThrottleInterval</key><integer>30</integer>
</dict>
</plist>"#;
        let restored = launchd_with_restart_policy_of(rewritten, previous).unwrap();
        let value = plist::Value::from_reader(std::io::Cursor::new(&restored)).unwrap();
        let dict = value.as_dictionary().unwrap();
        assert_eq!(dict.get("KeepAlive").unwrap().as_boolean(), Some(true));
        assert_eq!(
            dict.get("ThrottleInterval").unwrap().as_signed_integer(),
            Some(30)
        );
        assert_eq!(
            launchd_program_arguments(&restored).unwrap(),
            vec!["/new/pitchfork".to_string()]
        );
        assert_eq!(launchd_with_keep_alive(&restored), None);

        // A previous registration with no policy leaves the defaults to come.
        let no_policy = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Label</key><string>pitchfork</string></dict></plist>"#;
        assert_eq!(launchd_with_restart_policy_of(rewritten, no_policy), None);
        assert_eq!(
            launchd_with_restart_policy_of(rewritten, b"not a plist"),
            None
        );
        assert!(launchd_with_keep_alive(rewritten).is_some());
    }

    /// A `KeepAlive` the user set on the registration is their launchd
    /// policy, not a registration from before crash-restart was added.
    #[test]
    fn launchd_plist_keeps_a_local_keep_alive_policy() {
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Label</key><string>pitchfork</string>
    <key>ProgramArguments</key><array><string>/bin/pitchfork</string><string>supervisor</string></array>
    <key>KeepAlive</key><true/>
    <key>ExitTimeOut</key><integer>240</integer>
</dict>
</plist>"#;
        assert_eq!(launchd_with_keep_alive(plist), None);

        // Without a KeepAlive, a ThrottleInterval already there is kept too.
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Label</key><string>pitchfork</string>
    <key>ThrottleInterval</key><integer>30</integer>
</dict>
</plist>"#;
        let updated = launchd_with_keep_alive(plist).unwrap();
        let value = plist::Value::from_reader(std::io::Cursor::new(&updated)).unwrap();
        let dict = value.as_dictionary().unwrap();
        assert!(dict.get("KeepAlive").unwrap().as_dictionary().is_some());
        assert_eq!(
            dict.get("ThrottleInterval").unwrap().as_signed_integer(),
            Some(30)
        );
    }
}
