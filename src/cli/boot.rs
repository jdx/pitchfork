use crate::Result;
use crate::boot_manager::BootManager;

#[derive(Debug, usage_rs::Args)]
#[usage(
    about = "Enable or disable boot start",
    long_about = "\
Enable or disable boot start

Manages whether pitchfork supervisor starts automatically when the system
boots. Uses platform-specific mechanisms (launchd on macOS, systemd on Linux).

When run as root (or via sudo), registers a system-level entry that starts
pitchfork for all users:

    macOS: /Library/LaunchDaemons/pitchfork.plist
    Linux: /etc/systemd/system/pitchfork.service

When run as a normal user, registers a user-level entry:

    macOS: ~/Library/LaunchAgents/pitchfork.plist
    Linux: ~/.config/systemd/user/pitchfork.service

A system-level entry registered through sudo records the invoking user
(`supervisor run --boot --invoking-user <user>`). At boot, the root supervisor
behaves as if that user had started it with sudo: it reads their
~/.config/pitchfork/config.toml, keeps state files and IPC sockets in their
home directory, and runs daemons as that user unless `settings.supervisor.user`
or a daemon's `user` says otherwise. A system-level entry registered from a
root login shell records no user and runs entirely as root.

Running `enable` again rewrites an existing entry, for example to record the
invoking user in an entry created by an older version.

Subcommands:

    enable    Register pitchfork to start on boot
    disable   Remove pitchfork from boot startup
    status    Check if boot start is currently enabled

Examples:

    pitchfork boot enable           Start pitchfork on system boot (user-level)
    sudo pitchfork boot enable      Start pitchfork on system boot (system-level)
    pitchfork boot disable          Don't start pitchfork on boot
    pitchfork boot status           Check boot start status"
)]
pub struct Boot {
    #[usage(subcommand)]
    command: BootCommands,
}

#[derive(Debug, usage_rs::Subcommands)]
enum BootCommands {
    /// Enable boot start for pitchfork supervisor
    #[usage(long_help = "\
Enable boot start for pitchfork supervisor

Registers pitchfork to start automatically when the system boots.

When run as root (or via sudo): creates a system-level entry

    macOS: /Library/LaunchDaemons/pitchfork.plist
    Linux: /etc/systemd/system/pitchfork.service

When run as a normal user: creates a user-level entry

    macOS: ~/Library/LaunchAgents/pitchfork.plist
    Linux: ~/.config/systemd/user/pitchfork.service

Through sudo, the system-level entry records the invoking user so the root
supervisor uses that user's configuration, state directory, and identity at
boot, as `sudo pitchfork supervisor start` would. Set `settings.supervisor.user`
to choose a different user for state and daemons.

If an entry already exists at this level, it is rewritten with the current
binary path and invoking user.")]
    Enable(BootEnable),
    /// Disable boot start for pitchfork supervisor
    #[usage(long_help = "\
Disable boot start for pitchfork supervisor

Removes the boot start registration. Pitchfork will no longer start
automatically on system boot.")]
    Disable(BootDisable),
    /// Check boot start status
    #[usage(long_help = "\
Check boot start status

Reports whether pitchfork is configured to start on system boot.")]
    Status(BootStatus),
}

#[derive(Debug, usage_rs::Args)]
pub struct BootEnable {}

#[derive(Debug, usage_rs::Args)]
pub struct BootDisable {}

#[derive(Debug, usage_rs::Args)]
pub struct BootStatus {}

impl Boot {
    pub async fn run(&self) -> Result<()> {
        match &self.command {
            BootCommands::Enable(cmd) => cmd.run().await,
            BootCommands::Disable(cmd) => cmd.run().await,
            BootCommands::Status(cmd) => cmd.run().await,
        }
    }
}

impl BootEnable {
    async fn run(&self) -> Result<()> {
        let boot_manager = BootManager::new()?;

        if boot_manager.is_current_level_enabled()? {
            if boot_manager.is_current_level_up_to_date()? {
                // Even if already enabled, clean up any leftover legacy entry
                // from a partial migration on a previous attempt.
                #[cfg(target_os = "macos")]
                boot_manager.cleanup_legacy(false)?;
                println!("Boot start is already enabled");
            } else {
                // An entry from an older version, or one registered from a
                // different account, gets the current binary path and
                // invoking user.
                boot_manager.refresh()?;
                println!("Boot start is already enabled; registration updated");
                println!(
                    "The running service keeps its old settings until it is reloaded or the system restarts"
                );
            }
        } else {
            // enable() will error if the other privilege level is already registered.
            boot_manager.enable()?;
            info!("✓ Boot start enabled");
        }

        if let Some(user) = boot_manager.invoking_user() {
            info!(
                "the system service runs on behalf of {user}: it reads their pitchfork \
                configuration and runs daemons as {user} unless configured otherwise"
            );
        }

        Ok(())
    }
}

impl BootDisable {
    async fn run(&self) -> Result<()> {
        let boot_manager = BootManager::new()?;

        if !boot_manager.is_enabled()? {
            warn!("Boot start is already disabled");
            return Ok(());
        }

        boot_manager.disable()?;

        // disable() skips entries the caller lacks privileges to remove;
        // verify the result so we never report a false success.
        if boot_manager.is_enabled()? {
            miette::bail!(
                "boot start could not be fully disabled; \
                a system-level entry may require elevated privileges to remove"
            );
        }

        info!("✓ Boot start disabled");
        Ok(())
    }
}

impl BootStatus {
    async fn run(&self) -> Result<()> {
        let boot_manager = BootManager::new()?;
        let current_enabled = boot_manager.is_current_level_enabled()?;
        let other_enabled = boot_manager.is_other_level_enabled()?;

        match (current_enabled, other_enabled) {
            (true, true) => warn!(
                "Boot start is enabled at both user and system level; \
                run `pitchfork boot disable` (with appropriate privileges) to remove the unwanted entry"
            ),
            (true, false) => info!("Boot start is enabled"),
            (false, true) => warn!(
                "Boot start is registered at the other privilege level only; \
                run `pitchfork boot disable` (with appropriate privileges) to clean it up"
            ),
            (false, false) if boot_manager.is_enabled()? => info!("Boot start is enabled"),
            (false, false) => info!("Boot start is disabled"),
        }

        if boot_manager.is_system_level_enabled()? {
            match boot_manager.system_invoking_user() {
                Some(user) => info!(
                    "The system-level entry runs on behalf of {user}, using their configuration and state"
                ),
                None => info!(
                    "The system-level entry records no invoking user and runs with root's configuration; \
                    run `sudo pitchfork boot enable` from your account to use your configuration"
                ),
            }
        }

        Ok(())
    }
}
