use crate::Result;
use crate::cli::json_output::{JsonSupervisorStatus, print_json};
use crate::ipc::client::IpcClient;
use crate::procs::PROCS;

/// Gets the status of the pitchfork daemon
#[derive(Debug, usage_rs::Args)]
#[usage()]
pub struct Status {
    /// Output in JSON format
    #[usage(long)]
    json: bool,
}

impl Status {
    pub async fn run(&self) -> Result<()> {
        if self.json {
            return print_json(&self.status_json().await);
        }
        let ipc = IpcClient::connect(false).await?;
        info!("Pitchfork daemon is running");
        if let Some(url) = ipc.get_web_url().await? {
            info!("Web UI: {url}");
        }
        Ok(())
    }

    async fn status_json(&self) -> JsonSupervisorStatus {
        match IpcClient::connect(false).await {
            Ok(ipc) => match ipc.get_web_url().await {
                Ok(web_ui) => JsonSupervisorStatus {
                    status: "up",
                    web_ui,
                    error: None,
                },
                Err(err) => JsonSupervisorStatus {
                    status: "up",
                    web_ui: None,
                    error: Some(format!("failed to get web UI URL: {err}")),
                },
            },
            Err(err) => {
                debug!("failed to connect to supervisor: {err:?}");
                // Connecting can fail even while the supervisor is running
                // (permission denied on the socket, stale socket, I/O errors).
                // Only report "down" when the supervisor process is confirmed
                // gone; otherwise report "unknown" with what failed.
                match super::existing_supervisor_pid() {
                    Ok(Some(pid)) if PROCS.is_running(pid) => JsonSupervisorStatus {
                        status: "unknown",
                        web_ui: None,
                        error: Some(format!(
                            "supervisor process is running but IPC connection failed: {err}"
                        )),
                    },
                    Ok(_) => JsonSupervisorStatus {
                        status: "down",
                        web_ui: None,
                        error: None,
                    },
                    Err(state_err) => JsonSupervisorStatus {
                        status: "unknown",
                        web_ui: None,
                        error: Some(format!("failed to read supervisor state: {state_err}")),
                    },
                }
            }
        }
    }
}
