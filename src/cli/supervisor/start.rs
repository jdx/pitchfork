use crate::Result;
use crate::cli::supervisor::{
    KillOrStopOutcome, resolve_existing_supervisor, unidentified_supervisor_error,
};
use crate::ipc::client::IpcClient;
use crate::ipc::socket_display;
use crate::settings::settings;
use crate::supervisor;

/// Starts the internal pitchfork daemon in the background
#[derive(Debug, usage_rs::Args)]
#[usage()]
pub struct Start {
    /// kill existing daemon
    #[usage(short, long)]
    force: bool,
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        let (existing_pid, outcome) = resolve_existing_supervisor(self.force).await?;

        match outcome {
            KillOrStopOutcome::Unidentified if self.force => {
                return Err(unidentified_supervisor_error());
            }
            KillOrStopOutcome::Unidentified => {
                warn!(
                    "Pitchfork supervisor is already running (listening on {}).",
                    socket_display()
                );
                return Ok(());
            }
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
                // The old supervisor keeps serving IPC until it has stopped
                // its daemons. Wait for it to let go of the socket, so the
                // connect below reaches the new supervisor, not the old one.
                supervisor::wait_for_ipc_socket_release().await?;
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
