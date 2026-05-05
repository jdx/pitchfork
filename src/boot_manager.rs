use crate::{Result, env};
#[cfg(target_os = "linux")]
use auto_launcher::LinuxLaunchMode;
#[cfg(target_os = "macos")]
use auto_launcher::MacOSLaunchMode;
use auto_launcher::{AutoLaunch, AutoLaunchBuilder};
use miette::IntoDiagnostic;

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn build_launcher(
    app_path: &str,
    #[cfg(target_os = "macos")] macos_mode: MacOSLaunchMode,
    #[cfg(target_os = "linux")] linux_mode: LinuxLaunchMode,
) -> Result<AutoLaunch> {
    let mut builder = AutoLaunchBuilder::new();
    builder
        .set_app_name("pitchfork")
        .set_app_path(app_path)
        .set_args(&["supervisor", "run", "--boot"]);

    #[cfg(target_os = "macos")]
    builder.set_macos_launch_mode(macos_mode);

    #[cfg(target_os = "linux")]
    builder.set_linux_launch_mode(linux_mode);

    builder.build().into_diagnostic()
}

pub struct BootManager {
    /// The launcher matching the current privilege level (used for enable).
    current: AutoLaunch,
    /// The other level's launcher (used to detect cross-level registrations).
    other: AutoLaunch,
    /// Legacy macOS LaunchAgentSystem entry (pre-1.0.3 used /Library/LaunchAgents/
    /// instead of /Library/LaunchDaemons/ for root). Kept only for migration/cleanup.
    #[cfg(target_os = "macos")]
    legacy: AutoLaunch,
}

impl BootManager {
    pub fn new() -> Result<Self> {
        let app_path = env::PITCHFORK_BIN.to_string_lossy().to_string();

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
                build_launcher(&app_path, current_mode)?,
                build_launcher(&app_path, other_mode)?,
                build_launcher(&app_path, MacOSLaunchMode::LaunchAgentSystem)?,
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
                build_launcher(&app_path, current_mode)?,
                build_launcher(&app_path, other_mode)?,
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

        // Unsupported platforms: auto_launcher only supports macOS, Linux, and Windows.
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        compile_error!("pitchfork boot management is only supported on macOS, Linux, and Windows");

        #[cfg(target_os = "macos")]
        return Ok(Self {
            current,
            other,
            legacy,
        });

        #[cfg(not(target_os = "macos"))]
        Ok(Self { current, other })
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
        if nix::unistd::Uid::effective().is_root() && self.legacy.is_enabled().into_diagnostic()? {
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
        if nix::unistd::Uid::effective().is_root() && self.legacy.is_enabled().into_diagnostic()? {
            self.legacy.disable().into_diagnostic()?;
        }
        Ok(())
    }
}
