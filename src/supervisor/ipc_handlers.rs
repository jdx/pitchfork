//! IPC request handling and dispatch
//!
//! Handles incoming IPC requests from CLI clients and routes them to the appropriate handlers.

use super::{SUPERVISOR, Supervisor};
use crate::Result;
use crate::daemon::validate_daemon_id;
use crate::ipc::server::IpcServer;
use crate::ipc::{IpcRequest, IpcResponse};

impl Supervisor {
    /// Main IPC connection watch loop - reads and dispatches requests
    pub(crate) async fn conn_watch(&self, mut ipc: IpcServer) -> ! {
        loop {
            let (msg, send) = match ipc.read().await {
                Ok(msg) => msg,
                Err(e) => {
                    error!("failed to accept connection: {e:?}");
                    continue;
                }
            };
            debug!("received message: {msg:?}");
            tokio::spawn(async move {
                let rsp = SUPERVISOR
                    .handle_ipc(msg)
                    .await
                    .unwrap_or_else(|err| IpcResponse::Error(err.to_string()));
                if let Err(err) = send.send(rsp).await {
                    debug!("failed to send message: {err:?}");
                }
            });
        }
    }

    /// Handle a single IPC request and return the appropriate response
    pub(crate) async fn handle_ipc(&self, req: IpcRequest) -> Result<IpcResponse> {
        let rsp = match req {
            IpcRequest::Invalid { error } => {
                warn!("Invalid IPC request: {error}");
                return Ok(IpcResponse::Error(format!("Invalid request: {error}")));
            }
            IpcRequest::Connect => {
                debug!("received connect message");
                IpcResponse::Ok
            }
            IpcRequest::Stop { id } => {
                if let Err(e) = validate_daemon_id(&id) {
                    return Ok(IpcResponse::Error(e.to_string()));
                }
                self.stop(&id).await?
            }
            IpcRequest::Run(opts) => {
                if let Err(e) = validate_daemon_id(&opts.id) {
                    return Ok(IpcResponse::Error(e.to_string()));
                }
                self.run(opts).await?
            }
            IpcRequest::Enable { id } => {
                if let Err(e) = validate_daemon_id(&id) {
                    return Ok(IpcResponse::Error(e.to_string()));
                }
                if self.enable(id).await? {
                    IpcResponse::Yes
                } else {
                    IpcResponse::No
                }
            }
            IpcRequest::Disable { id } => {
                if let Err(e) = validate_daemon_id(&id) {
                    return Ok(IpcResponse::Error(e.to_string()));
                }
                if self.disable(id).await? {
                    IpcResponse::Yes
                } else {
                    IpcResponse::No
                }
            }
            IpcRequest::GetActiveDaemons => {
                let daemons = self.active_daemons().await;
                IpcResponse::ActiveDaemons(daemons)
            }
            IpcRequest::GetNotifications => {
                let notifications = self.get_notifications().await;
                IpcResponse::Notifications(notifications)
            }
            IpcRequest::UpdateShellDir { shell_pid, dir } => {
                let prev = self.get_shell_dir(shell_pid).await;
                self.set_shell_dir(shell_pid, dir.clone()).await?;
                // Cancel any pending autostops for daemons in the new directory
                self.cancel_pending_autostops_for_dir(&dir).await;
                if let Some(prev) = prev {
                    self.leave_dir(&prev).await?;
                }
                self.refresh().await?;
                IpcResponse::Ok
            }
            IpcRequest::Clean => {
                self.clean().await?;
                IpcResponse::Ok
            }
            IpcRequest::GetDisabledDaemons => {
                let disabled = self.state_file.lock().await.disabled.clone();
                IpcResponse::DisabledDaemons(disabled.into_iter().collect())
            }
        };
        Ok(rsp)
    }
}
