use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::{PitchforkToml, PitchforkTomlAuto};
use crate::{env, Result};
use duct::cmd;
use itertools::Itertools;
use log::LevelFilter;
use miette::IntoDiagnostic;
use std::collections::HashSet;

#[derive(Debug, clap::Args)]
#[clap(hide = true, verbatim_doc_comment)]
pub struct Cd {
    #[clap(long)]
    shell_pid: u32,
}

impl Cd {
    pub async fn run(&self) -> Result<()> {
        if let Ok(ipc) = IpcClient::connect(true).await {
            ipc.update_shell_dir(self.shell_pid, env::CWD.clone())
                .await?;

            let pt = PitchforkToml::all_merged();
            let to_start = pt
                .daemons
                .into_iter()
                .filter(|(_id, d)| d.auto.contains(&PitchforkTomlAuto::Start))
                .map(|(id, _d)| id)
                .collect_vec();
            if to_start.is_empty() {
                return Ok(());
            }
            let mut args = vec![
                "start".into(),
                "--shell-pid".into(),
                self.shell_pid.to_string(),
            ];

            let active_daemons: HashSet<String> = ipc
                .active_daemons()
                .await?
                .into_iter()
                .map(|d| d.id)
                .collect();
            for id in &to_start {
                if active_daemons.contains(id) {
                    continue;
                }
                args.push(id.clone());
            }
            if args.len() > 3 {
                cmd(&*env::PITCHFORK_BIN, args).run().into_diagnostic()?;
            }
            for (level, msg) in ipc.get_notifications().await? {
                match level {
                    LevelFilter::Trace => trace!("{}", msg),
                    LevelFilter::Debug => debug!("{}", msg),
                    LevelFilter::Info => info!("{}", msg),
                    LevelFilter::Warn => warn!("{}", msg),
                    LevelFilter::Error => error!("{}", msg),
                    _ => {}
                }
            }
        } else {
            debug!("No daemon running");
        }
        Ok(())
    }
}
