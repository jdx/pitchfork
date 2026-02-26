use crate::daemon::{Daemon, RunOptions};
use crate::error::IpcError;
use crate::ipc::batch::RunResult;
use crate::ipc::{IpcRequest, IpcResponse, deserialize, fs_name, serialize};
use crate::{Result, supervisor};
use exponential_backoff::Backoff;
use interprocess::local_socket::tokio::{RecvHalf, SendHalf};
use interprocess::local_socket::traits::tokio::Stream;
use miette::Context;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use uuid::Uuid;

pub struct IpcClient {
    _id: String,
    recv: Mutex<BufReader<RecvHalf>>,
    send: Mutex<SendHalf>,
}

const CONNECT_ATTEMPTS: u32 = 5;
const CONNECT_MIN_DELAY: Duration = Duration::from_millis(100);
const CONNECT_MAX_DELAY: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

impl IpcClient {
    pub async fn connect(autostart: bool) -> Result<Self> {
        if autostart {
            supervisor::start_if_not_running()?;
        }
        let id = Uuid::new_v4().to_string();
        let client = Self::connect_(&id, "main").await?;
        trace!("Connected to IPC socket");
        let rsp = client.request(IpcRequest::Connect).await?;
        if !rsp.is_ok() {
            return Err(IpcError::UnexpectedResponse {
                expected: "Ok".to_string(),
                actual: format!("{rsp:?}"),
            }
            .into());
        }
        debug!("Connected to IPC main");
        Ok(client)
    }

    async fn connect_(id: &str, name: &str) -> Result<Self> {
        for duration in Backoff::new(CONNECT_ATTEMPTS, CONNECT_MIN_DELAY, CONNECT_MAX_DELAY) {
            match interprocess::local_socket::tokio::Stream::connect(fs_name(name)?).await {
                Ok(conn) => {
                    let (recv, send) = conn.split();
                    let recv = BufReader::new(recv);

                    return Ok(Self {
                        _id: id.to_string(),
                        recv: Mutex::new(recv),
                        send: Mutex::new(send),
                    });
                }
                Err(err) => {
                    if let Some(duration) = duration {
                        debug!(
                            "Failed to connect to IPC socket: {err:?}, retrying in {duration:?}"
                        );
                        tokio::time::sleep(duration).await;
                        continue;
                    } else {
                        return Err(IpcError::ConnectionFailed {
                            attempts: CONNECT_ATTEMPTS,
                            source: Some(err),
                            help:
                                "ensure the supervisor is running with: pitchfork supervisor start"
                                    .to_string(),
                        }
                        .into());
                    }
                }
            }
        }
        Err(IpcError::ConnectionFailed {
            attempts: CONNECT_ATTEMPTS,
            source: None,
            help: "ensure the supervisor is running with: pitchfork supervisor start".to_string(),
        }
        .into())
    }

    pub async fn send(&self, msg: IpcRequest) -> Result<()> {
        let mut msg = serialize(&msg)?;
        if msg.contains(&0) {
            return Err(IpcError::InvalidMessage {
                reason: "message contains null byte".to_string(),
            }
            .into());
        }
        msg.push(0);
        let mut send = self.send.lock().await;
        send.write_all(&msg)
            .await
            .map_err(|e| IpcError::SendFailed { source: e })?;
        Ok(())
    }

    async fn read(&self, timeout: Duration) -> Result<IpcResponse> {
        let mut recv = self.recv.lock().await;
        let mut bytes = Vec::new();
        match tokio::time::timeout(timeout, recv.read_until(0, &mut bytes)).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                return Err(IpcError::ReadFailed { source: err }.into());
            }
            Err(_) => {
                return Err(IpcError::Timeout {
                    seconds: timeout.as_secs(),
                }
                .into());
            }
        }
        if bytes.is_empty() {
            return Err(IpcError::ConnectionClosed.into());
        }
        deserialize(&bytes).wrap_err("failed to deserialize IPC response")
    }

    pub(crate) async fn request(&self, msg: IpcRequest) -> Result<IpcResponse> {
        self.request_with_timeout(msg, REQUEST_TIMEOUT).await
    }

    pub(crate) fn unexpected_response(expected: &str, actual: &IpcResponse) -> IpcError {
        IpcError::UnexpectedResponse {
            expected: expected.to_string(),
            actual: format!("{actual:?}"),
        }
    }

    pub(crate) async fn request_with_timeout(
        &self,
        msg: IpcRequest,
        timeout: Duration,
    ) -> Result<IpcResponse> {
        self.send(msg).await?;
        self.read(timeout).await
    }

    // =========================================================================
    // Low-level IPC operations
    // =========================================================================

    pub async fn enable(&self, id: String) -> Result<bool> {
        let rsp = self.request(IpcRequest::Enable { id: id.clone() }).await?;
        match rsp {
            IpcResponse::Yes => {
                info!("Enabled daemon {id}");
                Ok(true)
            }
            IpcResponse::No => {
                info!("Daemon {id} already enabled");
                Ok(false)
            }
            rsp => Err(Self::unexpected_response("Yes or No", &rsp).into()),
        }
    }

    pub async fn disable(&self, id: String) -> Result<bool> {
        let rsp = self.request(IpcRequest::Disable { id: id.clone() }).await?;
        match rsp {
            IpcResponse::Yes => {
                info!("Disabled daemon {id}");
                Ok(true)
            }
            IpcResponse::No => {
                info!("Daemon {id} already disabled");
                Ok(false)
            }
            rsp => Err(Self::unexpected_response("Yes or No", &rsp).into()),
        }
    }

    /// Run a single daemon with the given options (low-level operation)
    pub async fn run(&self, opts: RunOptions) -> Result<RunResult> {
        let start_time = chrono::Local::now();
        // Use longer timeout for daemon start - ready_delay can be up to 60s+
        let timeout = Duration::from_secs(opts.ready_delay.unwrap_or(3) + 60);
        let rsp = self
            .request_with_timeout(IpcRequest::Run(opts.clone()), timeout)
            .await?;

        match rsp {
            IpcResponse::DaemonStart { daemon } => {
                info!("Started {}", daemon.id);
                Ok(RunResult {
                    started: true,
                    exit_code: None,
                    start_time,
                    resolved_ports: daemon.port.clone(),
                })
            }
            IpcResponse::DaemonReady { daemon } => {
                info!("Started {}", daemon.id);
                Ok(RunResult {
                    started: true,
                    exit_code: None,
                    start_time,
                    resolved_ports: daemon.port.clone(),
                })
            }
            IpcResponse::DaemonFailedWithCode { exit_code } => {
                let code = exit_code.unwrap_or(1);
                error!("Daemon {} failed with exit code {}", opts.id, code);

                // Print logs from the time we started this specific daemon
                if let Err(e) =
                    crate::cli::logs::print_logs_for_time_range(&opts.id, start_time, None)
                {
                    error!("Failed to print logs: {e}");
                }
                Ok(RunResult {
                    started: false,
                    exit_code: Some(code),
                    start_time,
                    resolved_ports: Vec::new(),
                })
            }
            IpcResponse::DaemonAlreadyRunning => {
                warn!("Daemon {} already running", opts.id);
                Ok(RunResult {
                    started: false,
                    exit_code: None,
                    start_time,
                    resolved_ports: Vec::new(),
                })
            }
            IpcResponse::DaemonFailed { error } => {
                error!("Failed to start daemon {}: {}", opts.id, error);

                // Print logs from the time we started this specific daemon
                if let Err(e) =
                    crate::cli::logs::print_logs_for_time_range(&opts.id, start_time, None)
                {
                    error!("Failed to print logs: {e}");
                }
                Ok(RunResult {
                    started: false,
                    exit_code: Some(1),
                    start_time,
                    resolved_ports: Vec::new(),
                })
            }
            rsp => Err(Self::unexpected_response("DaemonStart or DaemonReady", &rsp).into()),
        }
    }

    pub async fn active_daemons(&self) -> Result<Vec<Daemon>> {
        let rsp = self.request(IpcRequest::GetActiveDaemons).await?;
        match rsp {
            IpcResponse::ActiveDaemons(daemons) => Ok(daemons),
            rsp => Err(Self::unexpected_response("ActiveDaemons", &rsp).into()),
        }
    }

    pub async fn update_shell_dir(&self, shell_pid: u32, dir: PathBuf) -> Result<()> {
        let rsp = self
            .request(IpcRequest::UpdateShellDir {
                shell_pid,
                dir: dir.clone(),
            })
            .await?;
        match rsp {
            IpcResponse::Ok => {
                trace!("updated shell dir for pid {shell_pid} to {}", dir.display());
            }
            rsp => return Err(Self::unexpected_response("Ok", &rsp).into()),
        }
        Ok(())
    }

    pub async fn clean(&self) -> Result<()> {
        let rsp = self.request(IpcRequest::Clean).await?;
        match rsp {
            IpcResponse::Ok => {
                info!("Cleaned up stopped/failed daemons");
            }
            rsp => return Err(Self::unexpected_response("Ok", &rsp).into()),
        }
        Ok(())
    }

    pub async fn get_disabled_daemons(&self) -> Result<Vec<String>> {
        let rsp = self.request(IpcRequest::GetDisabledDaemons).await?;
        match rsp {
            IpcResponse::DisabledDaemons(daemons) => Ok(daemons),
            rsp => Err(Self::unexpected_response("DisabledDaemons", &rsp).into()),
        }
    }

    pub async fn get_notifications(&self) -> Result<Vec<(log::LevelFilter, String)>> {
        let rsp = self.request(IpcRequest::GetNotifications).await?;
        match rsp {
            IpcResponse::Notifications(notifications) => Ok(notifications),
            rsp => Err(Self::unexpected_response("Notifications", &rsp).into()),
        }
    }

    /// Stop a single daemon (low-level operation)
    pub async fn stop(&self, id: String) -> Result<bool> {
        let rsp = self.request(IpcRequest::Stop { id: id.clone() }).await?;
        match rsp {
            IpcResponse::Ok => {
                info!("Stopped daemon {id}");
                Ok(true)
            }
            IpcResponse::DaemonNotRunning => {
                warn!("Daemon {id} is not running");
                Ok(false)
            }
            IpcResponse::DaemonNotFound => {
                warn!("Daemon {id} not found");
                Ok(false)
            }
            IpcResponse::DaemonWasNotRunning => {
                warn!("Daemon {id} was not running (process may have exited unexpectedly)");
                Ok(false)
            }
            IpcResponse::DaemonStopFailed { error } => {
                error!("Failed to stop daemon {id}: {error}");
                Err(crate::error::DaemonError::StopFailed {
                    id: id.clone(),
                    error,
                }
                .into())
            }
            rsp => Err(Self::unexpected_response(
                "Ok, DaemonNotRunning, DaemonNotFound, DaemonWasNotRunning, or DaemonStopFailed",
                &rsp,
            )
            .into()),
        }
    }
}
