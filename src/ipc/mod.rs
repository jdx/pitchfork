use crate::Result;
use crate::daemon::{Daemon, RunOptions};
use crate::env;
use interprocess::local_socket::{GenericFilePath, Name, ToFsName};
use miette::{Context, IntoDiagnostic};
use std::path::PathBuf;

pub(crate) mod client;
pub(crate) mod server;

// #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::Display, strum::EnumIs)]
// pub enum IpcMessage {
//     Connect(String),
//     ConnectOK,
//     Run(String, Vec<String>),
//     Stop(String),
//     DaemonAlreadyRunning(String),
//     DaemonAlreadyStopped(String),
//     DaemonStart(Daemon),
//     DaemonStop { name: String },
//     DaemonFailed { name: String, error: String },
//     Response(String),
// }

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::Display, strum::EnumIs)]
#[allow(clippy::large_enum_variant)]
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
    GetNotifications,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::Display, strum::EnumIs)]
pub enum IpcResponse {
    Ok,
    Yes,
    No,
    Error(String),
    Notifications(Vec<(log::LevelFilter, String)>),
    ActiveDaemons(Vec<Daemon>),
    DisabledDaemons(Vec<String>),
    DaemonAlreadyRunning,
    DaemonStart {
        daemon: Daemon,
    },
    DaemonFailed {
        error: String,
    },
    DaemonReady {
        daemon: Daemon,
    },
    DaemonFailedWithCode {
        exit_code: Option<i32>,
    },
    /// Process was not running but had a PID record (unexpected exit)
    DaemonWasNotRunning,
    /// Failed to kill the process (still running)
    DaemonStopFailed {
        error: String,
    },
    /// Daemon exists but is not running (no PID)
    DaemonNotRunning,
    DaemonNotFound,
}
fn fs_name(name: &str) -> Result<Name<'_>> {
    let path = env::IPC_SOCK_DIR.join(name).with_extension("sock");
    let fs_name = path.to_fs_name::<GenericFilePath>().into_diagnostic()?;
    Ok(fs_name)
}

fn serialize<T: serde::Serialize>(msg: &T) -> Result<Vec<u8>> {
    if *env::IPC_JSON {
        serde_json::to_vec(msg)
            .into_diagnostic()
            .wrap_err("failed to serialize IPC message as JSON")
    } else {
        rmp_serde::to_vec(msg)
            .into_diagnostic()
            .wrap_err("failed to serialize IPC message as MessagePack")
    }
}

fn deserialize<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let mut bytes = bytes.to_vec();
    bytes.pop();
    let preview = std::str::from_utf8(&bytes).unwrap_or("<binary>");
    trace!("msg: {:?}", preview);
    if *env::IPC_JSON {
        serde_json::from_slice(&bytes)
            .into_diagnostic()
            .wrap_err("failed to deserialize IPC JSON response")
    } else {
        rmp_serde::from_slice(&bytes)
            .into_diagnostic()
            .wrap_err("failed to deserialize IPC MessagePack response")
    }
}
