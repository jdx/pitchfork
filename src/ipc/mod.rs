use crate::Result;
use crate::daemon::{Daemon, RunOptions};
use crate::daemon_id::DaemonId;
use crate::env;
use interprocess::local_socket::Name;
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use miette::{Context, IntoDiagnostic};
use std::path::PathBuf;

pub(crate) mod batch;
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
    /// Versioned connect handshake (v2): client sends its version so the supervisor can
    /// detect mismatches. Kept as a separate variant so the wire format of `Connect`
    /// (unit variant) stays unchanged for backward compatibility with older supervisors.
    ConnectV2 {
        version: String,
    },
    Clean,
    Stop {
        id: DaemonId,
    },
    GetActiveDaemons,
    GetDisabledDaemons,
    Run(RunOptions),
    Enable {
        id: DaemonId,
    },
    Disable {
        id: DaemonId,
    },
    UpdateShellDir {
        shell_pid: u32,
        dir: PathBuf,
    },
    GetNotifications,
    /// Notify the supervisor that the slug registry has changed (e.g. `proxy add/remove`).
    /// The supervisor should re-read slugs and update mDNS records accordingly.
    SyncMdns,
    /// Notify the supervisor that settings have changed.
    /// The supervisor should reload settings from config files.
    ReloadConfig,
    /// Enter or replace a project session for a host PID in a directory.
    ProjectEnter {
        pid: u32,
        dir: PathBuf,
    },
    /// Leave a project session for a host PID in a directory.
    ProjectLeave {
        pid: u32,
        dir: PathBuf,
    },
    /// List all tracked project sessions with live liveness status filled in
    /// by the supervisor.
    GetProjectSessions,
    /// A daemon's log sink reporting a line the supervisor needs to act on:
    /// one matching the readiness pattern, one that should fire the
    /// `on_output` hook, or both.
    ///
    /// The sink owns the daemon's output stream, so it does the matching and
    /// tells the supervisor rather than the other way around. Sent by
    /// `pitchfork log-sink`, not by any user-facing command.
    SinkOutputLine {
        id: DaemonId,
        /// Identifies the start attempt this sink belongs to, so a report from
        /// a sink still draining a previous attempt cannot satisfy a retry.
        token: u64,
        /// Whether this line passed the `on_output` hook's filter and debounce.
        /// A line reported only because it matched the readiness pattern must
        /// not fire a hook that filters for something else.
        fires_hook: bool,
        line: String,
    },
    /// Ask the supervisor for the URL of the web UI, if it is running.
    /// Reflects the actual bound address, not static config.
    GetWebUrl,
    /// Remove stopped daemon registrations matching the supplied filters.
    /// Appended to preserve the wire indexes of existing variants.
    CleanFiltered {
        namespaces: Vec<String>,
        daemons: Vec<DaemonId>,
        prune: bool,
    },
    /// Invalid request (failed to deserialize)
    #[serde(skip)]
    Invalid {
        error: String,
    },
}

/// A snapshot of a single project session, returned by `GetProjectSessions`.
///
/// `liveness_title` is the title recorded at enter time. `alive` and
/// `current_title` are filled in by the supervisor from its `PROCS` singleton
/// so the client does not need process introspection.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectSessionInfo {
    pub pid: u32,
    pub directory: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub liveness_title: Option<String>,
    pub alive: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub current_title: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::Display, strum::EnumIs)]
pub enum IpcResponse {
    Ok,
    /// Successful connect handshake, includes supervisor version for mismatch detection
    ConnectOk {
        version: String,
    },
    Yes,
    No,
    Error(String),
    Notifications(Vec<(log::LevelFilter, String)>),
    ActiveDaemons(Vec<Daemon>),
    DisabledDaemons(Vec<DaemonId>),
    DaemonAlreadyRunning,
    DaemonStart {
        daemon: Daemon,
    },
    DaemonFailed {
        error: String,
    },
    /// Port conflict detected with detailed process information
    PortConflict {
        port: u16,
        process: String,
        pid: u32,
    },
    /// No available ports found after exhausting auto-bump attempts
    NoAvailablePort {
        start_port: u16,
        attempts: u32,
    },
    DaemonReady {
        daemon: Daemon,
    },
    DaemonFailedWithCode {
        exit_code: Option<i32>,
    },
    /// Process was not running but had a PID record (unexpected exit)
    DaemonWasNotRunning,
    /// mDNS sync completed (or was a no-op if LAN mode is disabled)
    MdnsSynced,
    /// Settings reloaded from config files
    ConfigReloaded,
    /// URL of the running web UI, or `None` if the web UI is not running
    WebUrl {
        url: Option<String>,
    },
    /// Failed to kill the process (still running)
    DaemonStopFailed {
        error: String,
    },
    /// Daemon exists but is not running (no PID)
    DaemonNotRunning,
    DaemonNotFound,
    /// Snapshot of all project sessions (response to `GetProjectSessions`).
    ProjectSessions(Vec<ProjectSessionInfo>),
    /// Number of daemon registrations removed by `CleanFiltered`.
    Cleaned {
        count: u64,
    },
}
fn fs_name(name: &str) -> Result<Name<'_>> {
    // Unix: use a filesystem path for the AF_UNIX socket.
    #[cfg(unix)]
    {
        let path = env::IPC_SOCK_DIR.join(name).with_extension("sock");
        let fs_name = path.to_fs_name::<GenericFilePath>().into_diagnostic()?;
        Ok(fs_name)
    }
    // Windows: named pipes use a flat namespace (\\.\pipe\<name>) that
    // cannot contain path separators. Derive a unique pipe name from the
    // state directory to preserve test isolation when multiple supervisors
    // run concurrently with different PITCHFORK_STATE_DIR values.
    //
    // Use a hash of the state directory path rather than character replacement
    // to guarantee injectivity: `C:\a.b` and `C:\a\b` would both flatten to
    // `C--a-b` with the old approach, causing pipe name collisions.
    #[cfg(windows)]
    {
        let state_dir = env::PITCHFORK_STATE_DIR.to_string_lossy();
        // FNV-1a hash: deterministic, stable across Rust versions.
        // DefaultHasher's algorithm is not guaranteed stable, which would
        // break IPC if the CLI and supervisor were ever compiled with
        // different toolchains.
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in state_dir.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let pipe_name = format!("pitchfork-{hash:016x}-{name}");
        Ok(pipe_name
            .to_ns_name::<GenericNamespaced>()
            .into_diagnostic()?)
    }
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
    trace!("msg: {preview:?}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtered_clean_ipc_round_trips() {
        let request = IpcRequest::CleanFiltered {
            namespaces: vec!["worktree".to_string()],
            daemons: vec![DaemonId::new("worktree", "api")],
            prune: true,
        };
        let mut bytes = serialize(&request).unwrap();
        bytes.push(b'\n');
        let decoded: IpcRequest = deserialize(&bytes).unwrap();
        match decoded {
            IpcRequest::CleanFiltered {
                namespaces,
                daemons,
                prune,
            } => {
                assert_eq!(namespaces, ["worktree"]);
                assert_eq!(daemons, [DaemonId::new("worktree", "api")]);
                assert!(prune);
            }
            other => panic!("unexpected request: {other:?}"),
        }

        let mut bytes = serialize(&IpcResponse::Cleaned { count: 3 }).unwrap();
        bytes.push(b'\n');
        let decoded: IpcResponse = deserialize(&bytes).unwrap();
        assert!(matches!(decoded, IpcResponse::Cleaned { count: 3 }));
    }
}
