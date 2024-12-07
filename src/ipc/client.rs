use tokio::sync::Mutex;
use crate::ipc::{fs_name, IpcMessage};
use crate::{env, ipc, Result};
use interprocess::local_socket::tokio::{RecvHalf, SendHalf};
use interprocess::local_socket::traits::tokio::Stream;
use interprocess::local_socket::{GenericFilePath, ToFsName};
use tokio::io::{AsyncWriteExt, BufReader};
use uuid::Uuid;

pub struct IpcClient {
    id: String,
    recv: BufReader<RecvHalf>,
    send: Mutex<SendHalf>,
}

impl IpcClient {
    pub async fn connect() -> Result<Self> {
        // ensure nobody else can connect to the IPC server at the same time
        let _fslock = xx::fslock::get(&*env::IPC_SOCK_MAIN, false)?;
        let conn =
            interprocess::local_socket::tokio::Stream::connect(fs_name(&env::IPC_SOCK_MAIN)?)
                .await?;
        debug!("Connected to IPC main");
        let (recv, send) = conn.split();
        let recv = BufReader::new(recv);
        let id = Uuid::new_v4().to_string();
        let client = IpcClient { id, recv, send: Mutex::new(send) };
        client.send(IpcMessage::Connect(client.id.clone())).await?;
        Ok(client)
    }

    pub async fn send(&self, msg: IpcMessage) -> Result<()> {
        let mut msg = if *env::IPC_JSON {
            serde_json::to_vec(&msg)?
        } else {
            rmp_serde::to_vec(&msg)?
        };
        // if msg.contains(&b'\n') {
        //     panic!("IPC message contains newline");
        // }
        msg.push(0);
        let mut send = self.send.lock().await;
        send.write_all(&msg).await?;
        Ok(())
    }
}
