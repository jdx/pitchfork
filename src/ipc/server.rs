use crate::ipc::{deserialize, fs_name, serialize, IpcMessage};
use crate::{env, Result};
use eyre::{bail, eyre};
use interprocess::local_socket::tokio::SendHalf;
use interprocess::local_socket::traits::tokio::Listener;
use interprocess::local_socket::traits::tokio::Stream;
use interprocess::local_socket::ListenerOptions;
use std::collections::HashMap;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

pub struct IpcServer {
    clients: Mutex<HashMap<String, interprocess::local_socket::tokio::Stream>>,
    rx: tokio::sync::mpsc::Receiver<IpcMessage>,
}

impl IpcServer {
    pub async fn new() -> Result<Self> {
        xx::file::mkdirp(&*env::IPC_SOCK_DIR)?;
        let _ = fs::remove_file(&*env::IPC_SOCK_MAIN).await;
        let opts = ListenerOptions::new().name(fs_name("main")?);
        debug!("Listening on {}", env::IPC_SOCK_MAIN.display());
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let listener = opts.create_tokio()?;
        tokio::spawn(async move {
            loop {
                if let Err(err) = Self::listen(&listener, tx.clone()).await {
                    error!("ipc server {:?}", err);
                    continue;
                }
            }
        });
        let server = Self {
            clients: Default::default(),
            rx,
        };
        Ok(server)
    }

    pub async fn send(send: &mut SendHalf, msg: IpcMessage) -> Result<()> {
        let mut msg = serialize(&msg)?;
        if msg.contains(&0) {
            panic!("IPC message contains null");
        }
        msg.push(0);
        send.write_all(&msg).await?;
        Ok(())
    }

    pub async fn read(&mut self) -> Result<IpcMessage> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| eyre!("IPC channel closed"))
    }

    async fn listen(listener: &interprocess::local_socket::tokio::Listener, tx: tokio::sync::mpsc::Sender<IpcMessage>) -> Result<()> {
        let stream = listener.accept().await?;
        let (recv, mut send) = stream.split();
        let mut recv = BufReader::new(recv);
        let mut bytes = Vec::new();
        recv.read_until(0, &mut bytes).await?;
        match deserialize(&bytes)? {
            IpcMessage::Connect(id) => {
                debug!("Client connected: {}", id);
                Self::send(&mut send, IpcMessage::Response("Hello from server!".into())).await?;
                tokio::spawn(async move {
                    loop {
                        let mut bytes = Vec::new();
                        if recv.read_until(0, &mut bytes).await.is_err() {
                            break;
                        }
                        if bytes.is_empty() {
                            break;
                        }
                        let msg = match deserialize(&bytes) {
                            Ok(msg) => msg,
                            Err(err) => {
                                error!("Failed to deserialize message: {:?}", err);
                                continue;
                            }
                        };
                        if let Err(err) = tx.send(msg).await {
                            warn!("Failed to send message: {:?}", err);
                        }
                    }
                });
            },
            msg => {
             bail!("Unexpected message: {:?}", msg);
            },
        };
        Ok(())
    }

    pub fn close(&self) {
        debug!("Closing IPC server");
        let _ = std::fs::remove_file(&*env::IPC_SOCK_MAIN);
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.close();
    }
}
