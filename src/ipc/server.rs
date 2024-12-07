use crate::{env, Result};
use interprocess::local_socket::{GenericFilePath, ListenerOptions, Name, ToFsName};
use std::path::Path;
use tokio::fs;

pub async fn listen() -> Result<interprocess::local_socket::tokio::Listener> {
    let _ = fs::remove_file(&*env::IPC_SOCK_PATH).await;
    let opts = ListenerOptions::new().name(fs_name(&env::IPC_SOCK_PATH)?);
    let listener = opts.create_tokio()?;
    Ok(listener)
}

fn fs_name(path: &Path) -> Result<Name> {
    let fs_name = path.to_fs_name::<GenericFilePath>()?;
    Ok(fs_name)
}
