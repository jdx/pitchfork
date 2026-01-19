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

Subcommands:
  enable    Register pitchfork to start on boot
  disable   Remove pitchfork from boot startup
  status    Check if boot start is currently enabled

Examples:
  pitchfork boot enable           Start pitchfork on system boot
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
On macOS, creates a LaunchAgent. On Linux, creates a systemd user service.")]
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
