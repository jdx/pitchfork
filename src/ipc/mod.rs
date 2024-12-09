use crate::env;
use crate::state_file::StateFileDaemon;
use crate::Result;
use interprocess::local_socket::{GenericFilePath, Name, ToFsName};
use miette::IntoDiagnostic;

pub(crate) mod client;
pub(crate) mod server;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::Display, strum::EnumIs)]
pub enum IpcMessage {
    Connect(String),
    ConnectOK,
    Run(String, Vec<String>),
    Stop(String),
    DaemonAlreadyRunning(String),
    DaemonAlreadyStopped(String),
    DaemonStart(StateFileDaemon),
    DaemonStop { name: String },
    DaemonFailed { name: String, error: String },
    Response(String),
}

pub fn fs_name(name: &str) -> Result<Name> {
    let path = env::IPC_SOCK_DIR.join(name).with_extension("sock");
    let fs_name = path.to_fs_name::<GenericFilePath>().into_diagnostic()?;
    Ok(fs_name)
}

pub fn serialize(msg: &IpcMessage) -> Result<Vec<u8>> {
    let msg = if *env::IPC_JSON {
        serde_json::to_vec(msg).into_diagnostic()?
    } else {
        rmp_serde::to_vec(msg).into_diagnostic()?
    };
    Ok(msg)
}

pub fn deserialize(bytes: &[u8]) -> Result<IpcMessage> {
    let mut bytes = bytes.to_vec();
    bytes.pop();
    trace!("msg: {:?}", std::str::from_utf8(&bytes));
    let msg = if *env::IPC_JSON {
        serde_json::from_slice(&bytes).into_diagnostic()?
    } else {
        rmp_serde::from_slice(&bytes).into_diagnostic()?
    };
    Ok(msg)
}
