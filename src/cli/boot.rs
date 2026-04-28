use crate::Result;
use crate::boot_manager::BootManager;
use clap::Parser;

#[derive(Debug, Parser)]
#[clap(
    about = "Enable or disable boot start",
    long_about = "\
Enable or disable boot start

Manages whether pitchfork supervisor starts automatically when the system
boots. Uses platform-specific mechanisms (launchd on macOS, systemd on Linux).

When run as root (or via sudo), registers a system-level entry that starts
pitchfork for all users:
  macOS: /Library/LaunchAgents/pitchfork.plist
  Linux: /etc/systemd/system/pitchfork.service

When run as a normal user, registers a user-level entry:
  macOS: ~/Library/LaunchAgents/pitchfork.plist
  Linux: ~/.config/systemd/user/pitchfork.service

To run the supervisor as root but keep state files and IPC sockets in a
specific user's home directory, set `settings.supervisor.user` in the global
pitchfork configuration (~/.config/pitchfork/config.toml or
/etc/pitchfork/config.toml).

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
    #[clap(subcommand)]
    command: BootCommands,
}

#[derive(Debug, clap::Subcommand)]
enum BootCommands {
    /// Enable boot start for pitchfork supervisor
    #[clap(long_about = "\
Enable boot start for pitchfork supervisor

Registers pitchfork to start automatically when the system boots.

When run as root (or via sudo): creates a system-level entry
  macOS: /Library/LaunchAgents/pitchfork.plist
  Linux: /etc/systemd/system/pitchfork.service

When run as a normal user: creates a user-level entry
  macOS: ~/Library/LaunchAgents/pitchfork.plist
  Linux: ~/.config/systemd/user/pitchfork.service

If you want the supervisor to run as root but keep state files and IPC sockets
under a specific user's home directory, configure `settings.supervisor.user`
in your pitchfork configuration.")]
    Enable(BootEnable),
    /// Disable boot start for pitchfork supervisor
    #[clap(long_about = "\
Disable boot start for pitchfork supervisor

Removes the boot start registration. Pitchfork will no longer start
automatically on system boot.")]
    Disable(BootDisable),
    /// Check boot start status
    #[clap(long_about = "\
Check boot start status

Reports whether pitchfork is configured to start on system boot.")]
    Status(BootStatus),
}

#[derive(Debug, Parser)]
pub struct BootEnable {}

#[derive(Debug, Parser)]
pub struct BootDisable {}

#[derive(Debug, Parser)]
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

        if boot_manager.is_enabled()? {
            println!("Boot start is already enabled");
            return Ok(());
        }

        boot_manager.enable()?;
        info!("✓ Boot start enabled");

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
        info!("✓ Boot start disabled");

        Ok(())
    }
}

impl BootStatus {
    async fn run(&self) -> Result<()> {
        let boot_manager = BootManager::new()?;
        let is_enabled = boot_manager.is_enabled()?;

        if is_enabled {
            info!("Boot start is enabled");
        } else {
            info!("Boot start is disabled");
        }

        Ok(())
    }
}
