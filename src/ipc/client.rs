use crate::ipc::{deserialize, fs_name, serialize, IpcMessage};
use crate::Result;
use interprocess::local_socket::tokio::{RecvHalf, SendHalf};
use interprocess::local_socket::traits::tokio::Stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use uuid::Uuid;

pub struct IpcClient {
    id: String,
    recv: Mutex<BufReader<RecvHalf>>,
    send: Mutex<SendHalf>,
}

impl IpcClient {
    pub async fn connect() -> Result<Self> {
        let id = Uuid::new_v4().to_string();
        // // ensure nobody else can connect to the IPC main sock at the same time
        // let _fslock = xx::fslock::get(&env::IPC_SOCK_MAIN, false)?;
        let client = Self::connect_(&id, "main").await?;
        debug!("Connected to IPC main");
        client.send(IpcMessage::Connect(client.id.clone())).await?;
        // let msg = client.read().await?;
        // let client = Self::connect_(&id, &id).await?;
        // debug!("Connected to IPC sub");
        Ok(client)
    }

    async fn connect_(id: &str, name: &str) -> Result<Self> {
        let conn = interprocess::local_socket::tokio::Stream::connect(fs_name(name)?).await?;
        let (recv, send) = conn.split();
        let recv = BufReader::new(recv);
        Ok(Self {
            id: id.to_string(),
            recv: Mutex::new(recv),
            send: Mutex::new(send),
        })
    }

    pub async fn send(&self, msg: IpcMessage) -> Result<()> {
        let mut msg = serialize(&msg)?;
        if msg.contains(&0) {
            panic!("IPC message contains null");
        }
        msg.push(0);
        let mut send = self.send.lock().await;
        send.write_all(&msg).await?;
        Ok(())
    }

    pub async fn read(&self) -> Result<IpcMessage> {
        let mut recv = self.recv.lock().await;
        let mut bytes = Vec::new();
        recv.read_until(0, &mut bytes).await?;
        deserialize(&bytes)
    }
}
