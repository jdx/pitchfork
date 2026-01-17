use crate::daemon::{Daemon, RunOptions};
use crate::ipc::{deserialize, fs_name, serialize, IpcRequest, IpcResponse};
use crate::{supervisor, Result};
use exponential_backoff::Backoff;
use interprocess::local_socket::tokio::{RecvHalf, SendHalf};
use interprocess::local_socket::traits::tokio::Stream;
use miette::{bail, ensure, IntoDiagnostic};
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
        ensure!(rsp.is_ok(), "Failed to connect to IPC main");
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
                            "Failed to connect to IPC socket: {:?}, retrying in {:?}",
                            err, duration
                        );
                        tokio::time::sleep(duration).await;
                        continue;
                    } else {
                        bail!("Failed to connect to IPC socket: {:?}", err);
                    }
                }
            }
        }
        bail!(
            "failed to connect to IPC socket after {} attempts",
            CONNECT_ATTEMPTS
        )
    }

    pub async fn send(&self, msg: IpcRequest) -> Result<()> {
        let mut msg = serialize(&msg)?;
        if msg.contains(&0) {
            bail!("IPC message contains null byte");
        }
        msg.push(0);
        let mut send = self.send.lock().await;
        send.write_all(&msg).await.into_diagnostic()?;
        Ok(())
    }

    async fn read(&self, timeout: Duration) -> Result<IpcResponse> {
        let mut recv = self.recv.lock().await;
        let mut bytes = Vec::new();
        match tokio::time::timeout(timeout, recv.read_until(0, &mut bytes)).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => bail!("failed to read IPC message: {}", err),
            Err(_) => bail!("IPC read timed out after {:?}", timeout),
        }
        if bytes.is_empty() {
            bail!("IPC connection closed unexpectedly");
        }
        deserialize(&bytes)
    }

    async fn request(&self, msg: IpcRequest) -> Result<IpcResponse> {
        self.request_with_timeout(msg, REQUEST_TIMEOUT).await
    }

    async fn request_with_timeout(
        &self,
        msg: IpcRequest,
        timeout: Duration,
    ) -> Result<IpcResponse> {
        self.send(msg).await?;
        self.read(timeout).await
    }

    pub async fn enable(&self, id: String) -> Result<bool> {
        let rsp = self.request(IpcRequest::Enable { id: id.clone() }).await?;
        match rsp {
            IpcResponse::Yes => {
                info!("enabled daemon {}", id);
                Ok(true)
            }
            IpcResponse::No => {
                info!("daemon {} already enabled", id);
                Ok(false)
            }
            rsp => bail!("unexpected IPC response: {rsp:?}"),
        }
    }

    pub async fn disable(&self, id: String) -> Result<bool> {
        let rsp = self.request(IpcRequest::Disable { id: id.clone() }).await?;
        match rsp {
            IpcResponse::Yes => {
                info!("disabled daemon {}", id);
                Ok(true)
            }
            IpcResponse::No => {
                info!("daemon {} already disabled", id);
                Ok(false)
            }
            rsp => bail!("unexpected IPC response: {rsp:?}"),
        }
    }

    pub async fn run(&self, opts: RunOptions) -> Result<(Vec<String>, Option<i32>)> {
        info!("starting daemon {}", opts.id);
        let start_time = chrono::Local::now();
        // Use longer timeout for daemon start - ready_delay can be up to 60s+
        let timeout = Duration::from_secs(opts.ready_delay.unwrap_or(3) + 60);
        let rsp = self
            .request_with_timeout(IpcRequest::Run(opts.clone()), timeout)
            .await?;
        let mut started_daemons = vec![];
        let mut exit_code = None;
        match rsp {
            IpcResponse::DaemonStart { daemon } => {
                started_daemons.push(daemon.id.clone());
                info!("started {}", daemon.id);
            }
            IpcResponse::DaemonReady { daemon } => {
                started_daemons.push(daemon.id.clone());
                info!("started {}", daemon.id);
            }
            IpcResponse::DaemonFailedWithCode { exit_code: code } => {
                let code = code.unwrap_or(1);
                exit_code = Some(code);
                error!("daemon {} failed with exit code {}", opts.id, code);

                // Print logs from the time we started this specific daemon
                if let Err(e) =
                    crate::cli::logs::print_logs_for_time_range(&opts.id, start_time, None)
                {
                    error!("Failed to print logs: {}", e);
                }
            }
            IpcResponse::DaemonAlreadyRunning => {
                warn!("daemon {} already running", opts.id);
            }
            IpcResponse::DaemonFailed { error } => {
                error!("Failed to start daemon {}: {}", opts.id, error);
                exit_code = Some(1);

                // Print logs from the time we started this specific daemon
                if let Err(e) =
                    crate::cli::logs::print_logs_for_time_range(&opts.id, start_time, None)
                {
                    error!("Failed to print logs: {}", e);
                }
            }
            rsp => bail!("unexpected IPC response: {rsp:?}"),
        }
        Ok((started_daemons, exit_code))
    }

    pub async fn active_daemons(&self) -> Result<Vec<Daemon>> {
        let rsp = self.request(IpcRequest::GetActiveDaemons).await?;
        match rsp {
            IpcResponse::ActiveDaemons(daemons) => Ok(daemons),
            rsp => bail!("unexpected IPC response: {rsp:?}"),
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
            rsp => bail!("unexpected IPC response: {rsp:?}"),
        }
        Ok(())
    }

    pub async fn clean(&self) -> Result<()> {
        let rsp = self.request(IpcRequest::Clean).await?;
        match rsp {
            IpcResponse::Ok => {
                trace!("cleaned");
            }
            rsp => bail!("unexpected IPC response: {rsp:?}"),
        }
        Ok(())
    }

    pub async fn get_disabled_daemons(&self) -> Result<Vec<String>> {
        let rsp = self.request(IpcRequest::GetDisabledDaemons).await?;
        match rsp {
            IpcResponse::DisabledDaemons(daemons) => Ok(daemons),
            rsp => bail!("unexpected IPC response: {rsp:?}"),
        }
    }

    pub async fn get_notifications(&self) -> Result<Vec<(log::LevelFilter, String)>> {
        let rsp = self.request(IpcRequest::GetNotifications).await?;
        match rsp {
            IpcResponse::Notifications(notifications) => Ok(notifications),
            rsp => bail!("unexpected IPC response: {rsp:?}"),
        }
    }

    pub async fn stop(&self, id: String) -> Result<()> {
        let rsp = self.request(IpcRequest::Stop { id: id.clone() }).await?;
        match rsp {
            IpcResponse::Ok => {
                info!("stopped daemon {}", id);
                Ok(())
            }
            IpcResponse::DaemonAlreadyStopped => {
                warn!("daemon {} is not running", id);
                Ok(())
            }
            rsp => bail!("unexpected IPC response: {rsp:?}"),
        }
    }
}
