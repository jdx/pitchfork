use crate::Result;
use crate::cli::supervisor::{KillOrStopOutcome, resolve_existing_supervisor};
use crate::ipc::client::IpcClient;
use crate::procs::PROCS;
use crate::settings::settings;
use crate::supervisor;

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
        let (existing_pid, outcome) = resolve_existing_supervisor(self.force).await?;

        match outcome {
            KillOrStopOutcome::StillRunning => {
                // --force was not passed and the supervisor is already running.
                let pid = existing_pid.expect("StillRunning implies a pid exists");
                warn!(
                    "Pitchfork supervisor is already running with pid {pid}. Use `--force` to restart it."
                );
                return Ok(());
            }
            KillOrStopOutcome::Killed => {
                let pid = existing_pid.expect("Killed implies a pid exists");
                // Wait briefly for the old process to fully exit
                for _ in 0..20 {
                    if !PROCS.is_running(pid) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                info!("Killed existing supervisor with pid {pid}");
            }
            KillOrStopOutcome::AlreadyDead => {}
        }

        // Start a fresh supervisor in the background.
        supervisor::start_in_background()?;
        // Use autostart=false since we just spawned the supervisor above.
        // Passing true would cause connect() to call start_if_not_running(),
        // which races with the freshly spawned process writing its state file
        // and may spawn a second supervisor.
        IpcClient::connect(false).await?;
        info!("Supervisor started");

        let s = settings();
        if s.proxy.enable && s.proxy.https {
            let cert_path = if s.proxy.tls_cert.is_empty() {
                crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem")
            } else {
                std::path::PathBuf::from(&s.proxy.tls_cert)
            };
            if cert_path.exists() && !crate::proxy::trust::is_ca_trusted(&cert_path) {
                warn!(
                    "HTTPS proxy is enabled but the CA is not trusted. \
                     Run: pitchfork proxy trust"
                );
            }
        }

        Ok(())
    }
}
