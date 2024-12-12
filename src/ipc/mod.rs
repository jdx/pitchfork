use crate::daemon::{Daemon, RunOptions};
use crate::env;
use crate::Result;
use interprocess::local_socket::{GenericFilePath, Name, ToFsName};
use miette::IntoDiagnostic;
use std::path::PathBuf;

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
    DaemonStart(Daemon),
    DaemonStop { name: String },
    DaemonFailed { name: String, error: String },
    Response(String),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::Display, strum::EnumIs)]
pub enum IpcRequest {
    Connect,
    Clean,
    Stop { id: String },
    GetActiveDaemons,
    GetDisabledDaemons,
    Run(RunOptions),
    Enable { id: String },
    Disable { id: String },
    UpdateShellDir { shell_pid: u32, dir: PathBuf },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::Display, strum::EnumIs)]
pub enum IpcResponse {
    Ok,
    Yes,
    No,
    Error(String),
    ActiveDaemons(Vec<Daemon>),
    DisabledDaemons(Vec<String>),
    DaemonAlreadyStopped,
    DaemonAlreadyRunning,
    DaemonStart { daemon: Daemon },
    DaemonFailed { error: String },
}

fn fs_name(name: &str) -> Result<Name> {
    let path = env::IPC_SOCK_DIR.join(name).with_extension("sock");
    let fs_name = path.to_fs_name::<GenericFilePath>().into_diagnostic()?;
    Ok(fs_name)
}

fn serialize<T: serde::Serialize>(msg: &T) -> Result<Vec<u8>> {
    let msg = if *env::IPC_JSON {
        serde_json::to_vec(msg).into_diagnostic()?
    } else {
        rmp_serde::to_vec(msg).into_diagnostic()?
    };
    Ok(msg)
}

fn deserialize<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let mut bytes = bytes.to_vec();
    bytes.pop();
    trace!("msg: {:?}", std::str::from_utf8(&bytes).unwrap_or_default());
    let msg = if *env::IPC_JSON {
        serde_json::from_slice(&bytes).into_diagnostic()?
    } else {
        rmp_serde::from_slice(&bytes).into_diagnostic()?
    };
    Ok(msg)
}
