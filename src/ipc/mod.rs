use crate::env;
use crate::Result;
use interprocess::local_socket::{GenericFilePath, Name, ToFsName};

pub(crate) mod client;
pub(crate) mod server;

#[derive(Debug, serde::Serialize, serde::Deserialize, strum::Display, strum::EnumIs)]
pub enum IpcMessage {
    Connect(String),
    ConnectOK,
    Run(String, Vec<String>),
    Started(String),
    Response(String),
}

pub fn fs_name(name: &str) -> Result<Name> {
    let path = env::IPC_SOCK_DIR.join(name).with_extension("sock");
    dbg!(&path);
    let fs_name = path.to_fs_name::<GenericFilePath>()?;
    Ok(fs_name)
}

pub fn serialize(msg: &IpcMessage) -> Result<Vec<u8>> {
    let msg = if *env::IPC_JSON {
        serde_json::to_vec(msg)?
    } else {
        rmp_serde::to_vec(msg)?
    };
    Ok(msg)
}

pub fn deserialize(bytes: &[u8]) -> Result<IpcMessage> {
    let msg = if *env::IPC_JSON {
        serde_json::from_slice(bytes)?
    } else {
        rmp_serde::from_slice(bytes)?
    };
    Ok(msg)
}
