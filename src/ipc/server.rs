use crate::ipc::{IpcRequest, IpcResponse, deserialize, fs_name, serialize};
use crate::{Result, env};
use interprocess::local_socket::ListenerOptions;
use interprocess::local_socket::tokio::{RecvHalf, SendHalf};
use interprocess::local_socket::traits::tokio::Listener;
use interprocess::local_socket::traits::tokio::Stream;
use miette::{IntoDiagnostic, bail, miette};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{Receiver, Sender};

pub struct IpcServer {
    // clients: Mutex<HashMap<String, interprocess::local_socket::tokio::Stream>>,
    rx: Receiver<(IpcRequest, Sender<IpcResponse>)>,
}

impl IpcServer {
    pub fn new() -> Result<Self> {
        xx::file::mkdirp(&*env::IPC_SOCK_DIR)?;
        let _ = xx::file::remove_file(&*env::IPC_SOCK_MAIN);
        let opts = ListenerOptions::new().name(fs_name("main")?);
        debug!("Listening on {}", env::IPC_SOCK_MAIN.display());
        let (tx, rx) = tokio::sync::mpsc::channel(1);

        // Set restrictive umask before creating socket to avoid TOCTOU race condition.
        // This ensures the socket is created with 0600 permissions from the start.
        #[cfg(unix)]
        let old_umask = unsafe { libc::umask(0o077) };

        let listener = opts.create_tokio().into_diagnostic()?;

        // Restore original umask
        #[cfg(unix)]
        unsafe {
            libc::umask(old_umask);
        }

        tokio::spawn(async move {
            loop {
                if let Err(err) = Self::listen(&listener, tx.clone()).await {
                    error!("ipc server {:?}", err);
                    continue;
                }
            }
        });
        let server = Self { rx };
        Ok(server)
    }

    async fn send(send: &mut SendHalf, msg: IpcResponse) -> Result<()> {
        let mut msg = serialize(&msg)?;
        if msg.contains(&0) {
            bail!("IPC message contains null byte");
        }
        msg.push(0);
        if let Err(err) = send.write_all(&msg).await {
            trace!("Failed to send message: {:?}", err);
        }
        Ok(())
    }

    async fn read_message(recv: &mut BufReader<RecvHalf>) -> Result<Option<IpcRequest>> {
        let mut bytes = Vec::new();
        recv.read_until(0, &mut bytes).await.into_diagnostic()?;
        if bytes.is_empty() {
            return Ok(None);
        }
        Ok(Some(deserialize(&bytes)?))
    }

    fn read_messages_chan(recv: RecvHalf) -> Receiver<IpcRequest> {
        let mut recv = BufReader::new(recv);
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            loop {
                let msg = match Self::read_message(&mut recv).await {
                    Ok(Some(msg)) => {
                        trace!("Received message: {:?}", msg);
                        msg
                    }
                    Ok(None) => {
                        trace!("Client disconnected");
                        break;
                    }
                    Err(err) => {
                        error!("Failed to deserialize message: {:?}", err);
                        continue;
                    }
                };
                if let Err(err) = tx.send(msg).await {
                    warn!("Failed to emit message: {:?}", err);
                }
            }
        });
        rx
    }

    fn send_messages_chan(mut send: SendHalf) -> Sender<IpcResponse> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            loop {
                let msg = match rx.recv().await {
                    Some(msg) => {
                        trace!("Sending message: {:?}", msg);
                        msg
                    }
                    None => {
                        trace!("IPC channel closed");
                        break;
                    }
                };
                if let Err(err) = Self::send(&mut send, msg).await {
                    warn!("Failed to send message: {:?}", err);
                }
            }
        });
        tx
    }

    pub async fn read(&mut self) -> Result<(IpcRequest, Sender<IpcResponse>)> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| miette!("IPC channel closed"))
    }

    async fn listen(
        listener: &interprocess::local_socket::tokio::Listener,
        tx: Sender<(IpcRequest, Sender<IpcResponse>)>,
    ) -> Result<()> {
        let stream = listener.accept().await.into_diagnostic()?;
        trace!("Client accepted");
        let (recv, send) = stream.split();
        let mut incoming_chan = Self::read_messages_chan(recv);
        let outgoing_chan = Self::send_messages_chan(send);
        tokio::spawn(async move {
            while let Some(req) = incoming_chan.recv().await {
                if let Err(err) = tx.send((req, outgoing_chan.clone())).await {
                    debug!("Failed to send message: {:?}", err);
                }
            }
        });
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
