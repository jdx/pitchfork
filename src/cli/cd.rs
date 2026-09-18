use crate::daemon_id::DaemonId;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::{PitchforkToml, PitchforkTomlAuto};
use crate::{Result, env};
use duct::cmd;
use itertools::Itertools;
use miette::IntoDiagnostic;
use std::collections::HashSet;

#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
pub struct Cd {
    #[usage(long)]
    shell_pid: u32,
}

impl Cd {
    pub async fn run(&self) -> Result<()> {
        if let Ok(ipc) = IpcClient::connect(true).await {
            ipc.update_shell_dir(self.shell_pid, env::CWD.clone())
                .await?;

            let pt = PitchforkToml::all_merged()?;
            let to_start = pt
                .daemons
                .iter()
                .filter(|(_id, d)| d.auto.contains(&PitchforkTomlAuto::Start))
                .map(|(id, _d)| id.clone())
                .collect_vec();
            if to_start.is_empty() {
                return Ok(());
            }
            let mut args = vec![
                "start".into(),
                "--on-directory-enter".into(),
                "--shell-pid".into(),
                self.shell_pid.to_string(),
            ];

            let active_daemons: HashSet<DaemonId> = ipc
                .active_daemons()
                .await?
                .into_iter()
                .map(|d| d.id)
                .collect();
            // `--on-directory-enter` also drops a completed task reached as a
            // dependency; this only avoids spawning `start` at all when every
            // requested daemon is already settled. Config decides what counts
            // as a oneshot, since a daemon that used to be one keeps a stale
            // flag in state until its next run.
            let completed = crate::daemon_list::completed_oneshots();
            for id in &to_start {
                let settled =
                    completed.contains(id) && pt.daemons.get(id).is_some_and(|d| d.is_oneshot());
                if active_daemons.contains(id) || settled {
                    continue;
                }
                args.push(id.qualified());
            }
            if args.len() > 4 {
                cmd(&*env::PITCHFORK_BIN, args).run().into_diagnostic()?;
            }
            super::drain_notifications(&ipc).await;
        } else {
            debug!("No daemon running");
        }
        Ok(())
    }
}
