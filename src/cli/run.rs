use crate::{env, Result};
use eyre::bail;
use interprocess::local_socket::traits::tokio::Stream;
use interprocess::local_socket::{GenericFilePath, ToFsName};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::try_join;

/// Runs a one-off daemon
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Run {
    /// Name of the daemon to run
    name: String,
    #[clap(trailing_var_arg = true)]
    cmd: Vec<String>,
    #[clap(short, long)]
    force: bool,
}

impl Run {
    pub async fn run(&self) -> Result<()> {
        info!("Running one-off daemon");
        if self.cmd.is_empty() {
            bail!("No command provided");
        }
        dbg!(&self);

        let conn = interprocess::local_socket::tokio::Stream::connect(
            env::IPC_SOCK_PATH.clone().to_fs_name::<GenericFilePath>()?,
        )
        .await?;
        let (recv, mut send) = conn.split();
        let mut read = tokio::io::BufReader::new(recv);
        let mut buffer = String::with_capacity(1024);
        let send = send.write_all(b"Hello from client!\n");
        let recv = read.read_line(&mut buffer);
        try_join!(recv, send)?;
        println!("Received: {}", buffer.trim());
        Ok(())
    }
}
