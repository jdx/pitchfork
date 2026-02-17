use crate::Result;
use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::{
    PitchforkToml, PitchforkTomlAuto, PitchforkTomlDaemon, Retry, namespace_from_path,
};
use std::path::PathBuf;

/// Add a new daemon to ./pitchfork.toml
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "a", verbatim_doc_comment)]
pub struct Add {
    /// ID of the daemon to add (e.g., "api" or "namespace/api")
    pub id: String,
    /// Arguments to pass to the daemon
    #[clap(allow_hyphen_values = true, trailing_var_arg = true)]
    pub args: Vec<String>,
    /// Autostart the daemon when entering the directory
    #[clap(long)]
    pub autostart: bool,
    /// Autostop the daemon when leaving the directory
    #[clap(long)]
    pub autostop: bool,
}

impl Add {
    pub async fn run(&self) -> Result<()> {
        let config_path = PathBuf::from("pitchfork.toml");
        let mut pt = PitchforkToml::read(&config_path).unwrap_or_default();
        pt.path = pt.path.or(Some(config_path.clone()));
        let mut auto = vec![];
        if self.autostart {
            auto.push(PitchforkTomlAuto::Start);
        }
        if self.autostop {
            auto.push(PitchforkTomlAuto::Stop);
        }
        // Parse the daemon ID: if qualified, use it directly; otherwise use the
        // namespace from the config file being edited (not global resolution)
        let daemon_id = if self.id.contains('/') {
            DaemonId::parse(&self.id)?
        } else {
            let namespace = namespace_from_path(&config_path);
            DaemonId::new(&namespace, &self.id)
        };
        pt.daemons.insert(
            daemon_id.clone(),
            PitchforkTomlDaemon {
                run: shell_words::join(&self.args),
                auto,
                cron: None,
                retry: Retry::default(),
                ready_delay: None,
                ready_output: None,
                ready_http: None,
                ready_port: None,
                ready_cmd: None,
                boot_start: None,
                depends: vec![],
                watch: vec![],
                dir: None,
                env: None,
                path: None,
                on_ready: None,
                on_fail: None,
                on_cron_trigger: None,
                on_retry: None,
            },
        );
        pt.write()?;
        let path_display = pt
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "pitchfork.toml".to_string());
        println!("added {} to {}", daemon_id, path_display);
        Ok(())
    }
}
