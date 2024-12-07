use crate::{env, Result};
use interprocess::local_socket::{ListenerOptions, Name, ToFsName};
use interprocess::local_socket::traits::tokio::Listener;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use crate::ipc::{fs_name, IpcMessage};

pub struct IpcServer {
    listener: interprocess::local_socket::tokio::Listener,
}

impl IpcServer {
    pub async fn listen() -> Result<Self> {
        xx::file::mkdirp(&*env::IPC_SOCK_DIR)?;
        let _ = fs::remove_file(&*env::IPC_SOCK_MAIN).await;
        let opts = ListenerOptions::new().name(fs_name(&env::IPC_SOCK_MAIN)?);
        debug!("Listening on {}", env::IPC_SOCK_MAIN.display());
        let listener = opts.create_tokio()?;
        let server = Self { listener };
        Ok(server)
    }

    pub async fn read(&self) -> Result<IpcMessage> {
        let stream = self.listener.accept().await?;
        let mut recv = BufReader::new(&stream);
        let mut bytes = Vec::new();
        recv.read_until(0, &mut bytes).await?;
        if *env::IPC_JSON {
            Ok(serde_json::from_slice(&bytes)?)
        } else {
            Ok(rmp_serde::from_slice(&bytes)?)
        }
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
