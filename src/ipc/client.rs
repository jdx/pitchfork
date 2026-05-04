use crate::daemon::{Daemon, RunOptions};
use crate::daemon_id::DaemonId;
use crate::error::IpcError;
use crate::ipc::batch::RunResult;
use crate::ipc::{IpcRequest, IpcResponse, deserialize, fs_name, serialize};
use crate::settings::settings;
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

impl IpcClient {
    pub async fn connect(autostart: bool) -> Result<Self> {
        if autostart {
            supervisor::start_if_not_running()?;
        }
        let id = Uuid::new_v4().to_string();
        let client = Self::connect_(&id, "main").await?;
        trace!("Connected to IPC socket");
        let client_version = env!("CARGO_PKG_VERSION").to_string();

        // Try ConnectV2 first (supervisor that knows about it will return ConnectOk with its version).
        // If the supervisor is older and doesn't recognize ConnectV2, it will return Error,
        // and we fall back to the legacy Connect handshake.
        let rsp = client
            .request(IpcRequest::ConnectV2 {
                version: client_version.clone(),
            })
            .await?;
        match rsp {
            IpcResponse::ConnectOk {
                version: supervisor_version,
            } => {
                if supervisor_version != client_version {
                    warn!(
                        "CLI version {client_version} differs from supervisor version {supervisor_version}. \
                         Restart the supervisor with: pitchfork supervisor start --force"
                    );
                }
            }
            IpcResponse::Error(_) => {
                // Old supervisor doesn't recognize ConnectV2 — fall back to legacy Connect
                debug!("Supervisor did not recognize ConnectV2, falling back to legacy Connect");
                let rsp = client.request(IpcRequest::Connect).await?;
                if !rsp.is_ok() {
                    return Err(IpcError::UnexpectedResponse {
                        expected: "Ok".to_string(),
                        actual: format!("{rsp:?}"),
                    }
                    .into());
                }
                warn!(
                    "Supervisor is running an older version. \
                     Restart the supervisor with: pitchfork supervisor start --force"
                );
            }
            _ => {
                return Err(IpcError::UnexpectedResponse {
                    expected: "ConnectOk or Error".to_string(),
                    actual: format!("{rsp:?}"),
                }
                .into());
            }
        }
        debug!("Connected to IPC main");
        Ok(client)
    }

    async fn connect_(id: &str, name: &str) -> Result<Self> {
        let s = settings();
        let connect_attempts = u32::try_from(s.ipc.connect_attempts).unwrap_or_else(|_| {
            warn!(
                "ipc.connect_attempts value {} is out of range (0-{}), clamping to 5",
                s.ipc.connect_attempts,
                u32::MAX
            );
            5
        });
        let connect_attempts = if connect_attempts == 0 {
            warn!("ipc.connect_attempts is 0; defaulting to 1");
            1
        } else {
            connect_attempts
        };
        let connect_min_delay = s.ipc_connect_min_delay();
        let connect_max_delay = s.ipc_connect_max_delay();

        // Compute timeout from backoff parameters: sum the worst-case delays
        // for each attempt (exponential backoff capped at connect_max_delay),
        // plus a 1s buffer for connection overhead.
        let connect_timeout = {
            let mut total = Duration::from_secs(1); // buffer
            let mut delay = connect_min_delay;
            for _ in 0..connect_attempts {
                total += delay;
                delay = (delay * 2).min(connect_max_delay);
            }
            total
        };

        tokio::time::timeout(connect_timeout, async {
            for duration in Backoff::new(connect_attempts, connect_min_delay, connect_max_delay) {
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
                                attempts: connect_attempts,
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
                attempts: connect_attempts,
                source: None,
                help: "ensure the supervisor is running with: pitchfork supervisor start"
                    .to_string(),
            }
            .into())
        })
        .await
        .unwrap_or_else(|_| {
            Err(IpcError::ConnectionFailed {
                attempts: connect_attempts,
                source: None,
                help: format!(
                    "connection timed out after {connect_timeout:?}; ensure the supervisor is running with: pitchfork supervisor start"
                ),
            }
            .into())
        })
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
        self.request_with_timeout(msg, settings().ipc_request_timeout())
            .await
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

    pub async fn enable(&self, id: DaemonId) -> Result<bool> {
        let id_str = id.qualified();
        let rsp = self.request(IpcRequest::Enable { id: id.clone() }).await?;
        match rsp {
            IpcResponse::Yes => {
                info!("Enabled daemon {id_str}");
                Ok(true)
            }
            IpcResponse::No => {
                info!("Daemon {id_str} already enabled");
                Ok(false)
            }
            IpcResponse::Error(error) => Err(miette::miette!(error)),
            rsp => Err(Self::unexpected_response("Yes or No", &rsp).into()),
        }
    }

    pub async fn disable(&self, id: DaemonId) -> Result<bool> {
        let id_str = id.qualified();
        let rsp = self.request(IpcRequest::Disable { id: id.clone() }).await?;
        match rsp {
            IpcResponse::Yes => {
                info!("Disabled daemon {id_str}");
                Ok(true)
            }
            IpcResponse::No => {
                info!("Daemon {id_str} already disabled");
                Ok(false)
            }
            IpcResponse::Error(error) => Err(miette::miette!(error)),
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
                debug!("Started {}", daemon.id);
                Ok(RunResult {
                    started: true,
                    exit_code: None,
                    start_time,
                    resolved_ports: daemon.resolved_port.clone(),
                })
            }
            IpcResponse::DaemonReady { daemon } => {
                debug!("Started {}", daemon.id);
                Ok(RunResult {
                    started: true,
                    exit_code: None,
                    start_time,
                    resolved_ports: daemon.resolved_port.clone(),
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
            IpcResponse::PortConflict { port, process, pid } => {
                error!(
                    "Failed to start daemon {}: port {} is already in use by process '{}' (PID: {})",
                    opts.id, port, process, pid
                );
                Ok(RunResult {
                    started: false,
                    exit_code: Some(1),
                    start_time,
                    resolved_ports: Vec::new(),
                })
            }
            IpcResponse::NoAvailablePort {
                start_port,
                attempts,
            } => {
                error!(
                    "Failed to start daemon {}: could not find an available port after {} attempts starting from {}",
                    opts.id, attempts, start_port
                );
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

    pub async fn get_disabled_daemons(&self) -> Result<Vec<DaemonId>> {
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
    pub async fn stop(&self, id: DaemonId) -> Result<bool> {
        let id_str = id.qualified();
        let rsp = self.request(IpcRequest::Stop { id: id.clone() }).await?;
        match rsp {
            IpcResponse::Ok => {
                info!("Stopped daemon {id_str}");
                Ok(true)
            }
            IpcResponse::DaemonNotRunning => {
                warn!("Daemon {id_str} is not running");
                Ok(false)
            }
            IpcResponse::DaemonNotFound => {
                warn!("Daemon {id_str} not found");
                Ok(false)
            }
            IpcResponse::DaemonWasNotRunning => {
                warn!("Daemon {id_str} was not running (process may have exited unexpectedly)");
                Ok(false)
            }
            IpcResponse::DaemonStopFailed { error } => {
                error!("Failed to stop daemon {id_str}: {error}");
                Err(crate::error::DaemonError::StopFailed {
                    id: id_str.clone(),
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
