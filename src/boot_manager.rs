use crate::{env, Result};
use auto_launch::{AutoLaunch, AutoLaunchBuilder, LinuxLaunchMode, MacOSLaunchMode};
use miette::IntoDiagnostic;

pub struct BootManager {
    auto_launch: AutoLaunch,
}

impl BootManager {
    pub fn new() -> Result<Self> {
        let app_name = "pitchfork";
        let app_path = env::PITCHFORK_BIN.to_string_lossy().to_string();

        let auto_launch = AutoLaunchBuilder::new()
            .set_app_name(app_name)
            .set_app_path(&app_path)
            .set_macos_launch_mode(MacOSLaunchMode::LaunchAgent)
            .set_linux_launch_mode(LinuxLaunchMode::Systemd)
            .set_args(&["supervisor", "run", "--boot"])
            .build()
            .into_diagnostic()?;

        Ok(Self { auto_launch })
    }

    pub fn is_enabled(&self) -> Result<bool> {
        self.auto_launch.is_enabled().into_diagnostic()
    }

    pub fn enable(&self) -> Result<()> {
        self.auto_launch.enable().into_diagnostic()?;
        Ok(())
    }

    pub fn disable(&self) -> Result<()> {
        self.auto_launch.disable().into_diagnostic()?;
        Ok(())
    }
}
