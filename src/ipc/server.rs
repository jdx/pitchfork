use std::collections::HashMap;
use eyre::eyre;
use crate::ipc::{deserialize, fs_name, serialize, IpcMessage};
use crate::{env, Result};
use interprocess::local_socket::traits::tokio::Listener;
use interprocess::local_socket::{ListenerOptions, Name, ToFsName};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
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
                let msg = Self::listen(&listener).await;
                match msg {
                    Ok(msg) => {
                        tx.send(msg).await.unwrap();
                    }
                    Err(e) => {
                        error!("IPC error: {}", e);
                    }
                }
            }
        });
        let server = Self { clients: Default::default(), rx };
        Ok(server)
    }
    
    // pub async fn send(&self, msg: IpcMessage) -> Result<()> {
    //     let mut msg = serialize(&msg)?;
    //     if msg.contains(&0) {
    //         panic!("IPC message contains null");
    //     }
    //     msg.push(0);
    //     send.write_all(&msg).await?;
    //     Ok(())
    // }
    
    pub async fn read(&mut self) -> Result<IpcMessage> {
        self.rx.recv().await.ok_or_else(|| eyre!("IPC channel closed"))
    }

    async fn listen(listener: &interprocess::local_socket::tokio::Listener) -> Result<IpcMessage> {
        let stream = listener.accept().await?;
        let mut recv = BufReader::new(&stream);
        let mut bytes = Vec::new();
        recv.read_until(0, &mut bytes).await?;
        deserialize(&bytes)
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
