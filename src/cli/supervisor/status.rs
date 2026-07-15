use crate::Result;
use crate::cli::json_output::{JsonSupervisorStatus, print_json};
use crate::ipc::client::IpcClient;
use crate::procs::PROCS;

/// Gets the status of the pitchfork daemon
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Status {
    /// Output in JSON format
    #[clap(long)]
    json: bool,
}

impl Status {
    pub async fn run(&self) -> Result<()> {
        if self.json {
            return print_json(&self.status_json().await);
        }
        let ipc = IpcClient::connect(false).await?;
        info!("Pitchfork daemon is running");
        if let Some(url) = ipc.get_web_url().await.unwrap_or_default() {
            info!("Web UI: {url}");
        }
        Ok(())
    }

    async fn status_json(&self) -> JsonSupervisorStatus {
        match IpcClient::connect(false).await {
            Ok(ipc) => JsonSupervisorStatus {
                status: "up",
                web_ui: ipc.get_web_url().await.unwrap_or_default(),
                error: None,
            },
            Err(err) => {
                debug!("failed to connect to supervisor: {err:?}");
                // Connecting can fail even while the supervisor is running
                // (permission denied on the socket, stale socket, I/O errors).
                // Only report "down" when the supervisor process is actually
                // gone; otherwise report "unknown" with the connect error.
                let running = super::existing_supervisor_pid()
                    .ok()
                    .flatten()
                    .is_some_and(|pid| PROCS.is_running(pid));
                if running {
                    JsonSupervisorStatus {
                        status: "unknown",
                        web_ui: None,
                        error: Some(format!(
                            "supervisor process is running but IPC connection failed: {err}"
                        )),
                    }
                } else {
                    JsonSupervisorStatus {
                        status: "down",
                        web_ui: None,
                        error: None,
                    }
                }
            }
        }
    }
}
