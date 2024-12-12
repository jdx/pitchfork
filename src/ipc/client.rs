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
        unreachable!()
    }

    pub async fn send(&self, msg: IpcRequest) -> Result<()> {
        let mut msg = serialize(&msg)?;
        if msg.contains(&0) {
            panic!("IPC message contains null");
        }
        msg.push(0);
        let mut send = self.send.lock().await;
        send.write_all(&msg).await.into_diagnostic()?;
        Ok(())
    }

    pub async fn read(&self) -> Option<IpcResponse> {
        let mut recv = self.recv.lock().await;
        let mut bytes = Vec::new();
        if let Err(err) = recv.read_until(0, &mut bytes).await.into_diagnostic() {
            warn!("Failed to read IPC message: {}", err);
        }
        if bytes.is_empty() {
            None
        } else {
            match deserialize(&bytes) {
                Ok(msg) => Some(msg),
                Err(err) => {
                    warn!("Failed to deserialize IPC message: {}", err);
                    None
                }
            }
        }
    }

    async fn request(&self, msg: IpcRequest) -> Result<IpcResponse> {
        self.send(msg).await?;
        loop {
            if let Some(msg) = self.read().await {
                return Ok(msg);
            }
        }
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
            rsp => unreachable!("unexpected response: {rsp:?}"),
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
            rsp => unreachable!("unexpected response: {rsp:?}"),
        }
    }

    pub async fn run(&self, opts: RunOptions) -> Result<()> {
        info!("starting daemon {}", opts.id);
        let rsp = self.request(IpcRequest::Run(opts.clone())).await?;
        match rsp {
            IpcResponse::DaemonStart { daemon } => {
                info!("started daemon {}", daemon);
            }
            IpcResponse::DaemonAlreadyRunning => {
                warn!("daemon {} already running", opts.id);
            }
            IpcResponse::DaemonFailed { error } => {
                bail!("failed to start daemon {}: {error}", opts.id);
            }
            rsp => unreachable!("unexpected response: {rsp:?}"),
        }
        Ok(())
    }

    pub async fn active_daemons(&self) -> Result<Vec<Daemon>> {
        let rsp = self.request(IpcRequest::GetActiveDaemons).await?;
        match rsp {
            IpcResponse::ActiveDaemons(daemons) => Ok(daemons),
            rsp => unreachable!("unexpected response: {rsp:?}"),
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
            rsp => unreachable!("unexpected response: {rsp:?}"),
        }
        Ok(())
    }

    pub async fn clean(&self) -> Result<()> {
        let rsp = self.request(IpcRequest::Clean).await?;
        match rsp {
            IpcResponse::Ok => {
                trace!("cleaned");
            }
            rsp => unreachable!("unexpected response: {rsp:?}"),
        }
        Ok(())
    }

    pub async fn get_disabled_daemons(&self) -> Result<Vec<String>> {
        let rsp = self.request(IpcRequest::GetDisabledDaemons).await?;
        match rsp {
            IpcResponse::DisabledDaemons(daemons) => Ok(daemons),
            rsp => unreachable!("unexpected response: {rsp:?}"),
        }
    }
}
