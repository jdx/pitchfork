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
            let app_path = env::PITCHFORK_BIN.to_string_lossy().to_string();

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
                invoking_user,
                current,
                other,
                legacy,
            });

            #[cfg(not(target_os = "macos"))]
            Ok(Self {
                invoking_user,
                current,
                other,
            })
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

        /// Whether the current-level registration already has the current
        /// binary path and invoking user, so `enable` has nothing to change.
        pub fn is_current_level_up_to_date(&self) -> Result<bool> {
            let current_bin = env::PITCHFORK_BIN.to_string_lossy();
            let registered = self.current.get_registered_app_path().into_diagnostic()?;
            if registered.as_deref() != Some(current_bin.as_ref()) {
                return Ok(false);
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

            self.current.enable().into_diagnostic()?;

            #[cfg(target_os = "macos")]
            self.cleanup_legacy(true)?;

            Ok(())
        }

        /// Rewrite the existing registration at the current privilege level,
        /// updating its binary path and recorded invoking user.
        pub fn refresh(&self) -> Result<()> {
            self.current.enable().into_diagnostic()?;

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

        /// Check whether the registered boot binary path matches the current
        /// `PITCHFORK_BIN`. If stale (binary moved after a package-manager upgrade),
        /// re-register at the current privilege level so the next boot uses the
        /// correct path.
        ///
        /// This is a no-op when boot start is not enabled, or when the registered
        /// path already matches. Errors are logged and swallowed — this is a
        /// best-effort self-heal that must not block supervisor startup.
        pub fn check_and_reregister_if_stale(&self) {
            let current_bin = env::PITCHFORK_BIN.to_string_lossy().to_string();

            let registered = match self.current.get_registered_app_path() {
                Ok(Some(path)) => path,
                Ok(None) => return, // not registered, nothing to do
                Err(e) => {
                    warn!("failed to read registered boot path: {e}");
                    return;
                }
            };

            if registered == current_bin {
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
            if let Err(e) = launcher.enable() {
                warn!("failed to re-register boot start with current path: {e}");
                return;
            }

            info!("boot registration updated to current binary path");
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
            )
        }

        pub fn is_enabled(&self) -> Result<bool> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            )
        }

        pub fn is_current_level_enabled(&self) -> Result<bool> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            )
        }

        pub fn is_other_level_enabled(&self) -> Result<bool> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            )
        }

        pub fn enable(&self) -> Result<()> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            )
        }

        pub fn refresh(&self) -> Result<()> {
            miette::bail!(
                "boot management is not supported on this platform; \
                only macOS, Linux, and Windows are supported"
            )
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
            )
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

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::imp::service_args;
    use super::{launchd_program_arguments, systemd_exec_start};
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
}
