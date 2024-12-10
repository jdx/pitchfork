use crate::ipc::{deserialize, fs_name, serialize, IpcMessage};
use crate::Result;
use exponential_backoff::Backoff;
use interprocess::local_socket::tokio::{RecvHalf, SendHalf};
use interprocess::local_socket::traits::tokio::Stream;
use miette::{bail, IntoDiagnostic};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use uuid::Uuid;

pub struct IpcClient {
    id: String,
    recv: Mutex<BufReader<RecvHalf>>,
    send: Mutex<SendHalf>,
}

const CONNECT_ATTEMPTS: u32 = 5;
const CONNECT_MIN_DELAY: Duration = Duration::from_millis(100);
const CONNECT_MAX_DELAY: Duration = Duration::from_secs(1);

impl IpcClient {
    pub async fn connect() -> Result<Self> {
        let id = Uuid::new_v4().to_string();
        let client = Self::connect_(&id, "main").await?;
        trace!("Connected to IPC socket");
        client.send(IpcMessage::Connect(client.id.clone())).await?;
        let msg = client.read().await.unwrap();
        assert!(msg.is_connect_ok());
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
                        id: id.to_string(),
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

    pub async fn send(&self, msg: IpcMessage) -> Result<()> {
        let mut msg = serialize(&msg)?;
        if msg.contains(&0) {
            panic!("IPC message contains null");
        }
        msg.push(0);
        let mut send = self.send.lock().await;
        send.write_all(&msg).await.into_diagnostic()?;
        Ok(())
    }

    pub async fn read(&self) -> Option<IpcMessage> {
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
}
