use crate::Result;
use crate::cli::supervisor::kill_or_stop;
use crate::daemon_id::DaemonId;
use crate::ipc::client::IpcClient;
use crate::procs::PROCS;
use crate::settings::settings;
use crate::state_file::StateFile;
use crate::{env, supervisor};

/// Starts the internal pitchfork daemon in the background
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Start {
    /// kill existing daemon
    #[clap(short, long)]
    force: bool,
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        if self.force {
            let sf = StateFile::read(&*env::PITCHFORK_STATE_FILE)?;
            if let Some(d) = sf.daemons.get(&DaemonId::pitchfork())
                && let Some(pid) = d.pid
            {
                if !kill_or_stop(pid, true).await? {
                    return Ok(());
                }
                // Wait briefly for the old process to fully exit
                for _ in 0..20 {
                    if !PROCS.is_running(pid) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                // Start a fresh supervisor in the background
                supervisor::start_in_background()?;
            }
        }
        IpcClient::connect(true).await?;
        info!("Supervisor started");

        let s = settings();
        if s.proxy.enable && s.proxy.https {
            // Only prompt to trust the cert if it hasn't been generated yet.
            // Once the cert exists the user has already been through the trust
            // flow (or is using a custom cert), so repeating the hint on every
            // `supervisor start` would be noisy.
            let cert_path = if s.proxy.tls_cert.is_empty() {
                crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem")
            } else {
                std::path::PathBuf::from(&s.proxy.tls_cert)
            };
            if !cert_path.exists() {
                warn!(
                    "HTTPS proxy is enabled. To trust the self-signed certificate, run: pitchfork proxy trust"
                );
            }
        }

        Ok(())
    }
}
