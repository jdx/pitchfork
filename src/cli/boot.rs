use crate::boot_manager::BootManager;
use crate::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[clap(about = "Enable or disable boot start")]
pub struct Boot {
    #[clap(subcommand)]
    command: BootCommands,
}

#[derive(Debug, clap::Subcommand)]
enum BootCommands {
    /// Enable boot start for pitchfork supervisor
    Enable(BootEnable),
    /// Disable boot start for pitchfork supervisor
    Disable(BootDisable),
    /// Check boot start status
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
