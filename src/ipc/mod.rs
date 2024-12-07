use std::path::Path;
use interprocess::local_socket::{GenericFilePath, Name, ToFsName};

pub(crate) mod client;
pub(crate) mod server;

#[derive(Debug, serde::Serialize, serde::Deserialize, strum::Display)]
pub enum IpcMessage {
    Connect(String),
    Response(String),
}

pub fn fs_name(path: &Path) -> eyre::Result<Name> {
    let fs_name = path.to_fs_name::<GenericFilePath>()?;
    Ok(fs_name)
}
