use crate::Result;
use crate::daemon::{Daemon, RunOptions};
use crate::daemon_id::DaemonId;
use crate::env;
#[cfg(unix)]
use crate::error::IpcError;
use interprocess::local_socket::Name;
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use miette::{Context, IntoDiagnostic};
#[cfg(unix)]
use std::path::Path;
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
    /// Shut the supervisor down gracefully, as on SIGTERM, for
    /// `supervisor stop`. Answered with `ShuttingDown` once new starts are
    /// frozen; an older supervisor answers `Error`, and is sent the signal.
    Shutdown,
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
    /// These daemons are being started explicitly: none of them is to be
    /// stopped for inactivity from now on, even if the proxy started it.
    /// Sent before the start itself, and answered once any idle stop already
    /// under way for one of them has finished. Appended to preserve the wire
    /// indexes of existing variants.
    ClaimDaemons {
        ids: Vec<DaemonId>,
    },
    /// The supervisor's records of these daemons, whatever their status.
    /// Missing ids are left out. Appended to preserve the wire indexes of
    /// existing variants.
    GetDaemons {
        ids: Vec<DaemonId>,
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
    /// The supervisor is stopping its daemons and will exit; `budget_ms` is
    /// how long that may take, after which it may be killed.
    ShuttingDown {
        budget_ms: u64,
    },
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
        /// Ports resolved by the failed attempt, so in-process retry hooks
        /// observe the attempt's actual (post-bump) ports.
        #[serde(default)]
        resolved_ports: Vec<u16>,
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
    /// Records answering `GetDaemons`.
    Daemons(Vec<Daemon>),
}

/// Bytes a socket path may occupy in `sockaddr_un.sun_path` on this platform
/// (108 on Linux, 104 on macOS and the BSDs). Matches the check `interprocess`
/// makes when it builds the address, so an over-long path is caught here first.
#[cfg(unix)]
const SOCKET_PATH_CAPACITY: usize = {
    // SAFETY: `sockaddr_un` is plain data, for which all-zero bytes are valid.
    let sun = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
    sun.sun_path.len()
};

/// Fail with an actionable error when a socket path cannot fit in `sun_path`.
///
/// Without this the connection attempt fails, five retries later, with
/// `local socket name length exceeds capacity of sun_path of sockaddr_un`,
/// which names neither the path nor what to change.
#[cfg(unix)]
fn check_socket_path(path: &Path, capacity: usize) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let len = path.as_os_str().as_bytes().len();
    if len <= capacity {
        return Ok(());
    }
    let over = len - capacity;
    let help = format!(
        "the socket is {path} and is {over} byte(s) over the limit.\n\
         Set PITCHFORK_STATE_DIR (or XDG_STATE_HOME) to a shorter directory, for example \
         PITCHFORK_STATE_DIR=/tmp/pitchfork, so that <state dir>/sock/main.sock is at most \
         {capacity} bytes.",
        path = path.display()
    );
    Err(IpcError::SocketPathTooLong {
        path: path.to_path_buf(),
        len,
        limit: capacity,
        help,
    }
    .into())
}

fn fs_name(name: &str) -> Result<Name<'_>> {
    // Unix: use a filesystem path for the AF_UNIX socket.
    #[cfg(unix)]
    {
        let path = env::IPC_SOCK_DIR.join(name).with_extension("sock");
        check_socket_path(&path, SOCKET_PATH_CAPACITY)?;
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
        pipe_name
            .to_ns_name::<GenericNamespaced>()
            .into_diagnostic()
    }
}

/// Human-readable location of the supervisor's IPC endpoint, for messages.
pub(crate) fn socket_display() -> String {
    #[cfg(unix)]
    {
        env::IPC_SOCK_MAIN.display().to_string()
    }
    #[cfg(windows)]
    {
        "the supervisor named pipe".to_string()
    }
}

/// Whether a supervisor is accepting connections on the IPC socket right now.
///
/// This is the ground truth for "is a supervisor running": the state-file
/// record can be lost or rewritten while the supervisor keeps serving, and a
/// supervisor started on that basis would take the socket over and leave the
/// original running but unreachable. A stale socket file left by a crashed
/// supervisor refuses connections, so it does not count.
pub(crate) async fn supervisor_listening() -> bool {
    use interprocess::local_socket::traits::tokio::Stream as _;
    let Ok(name) = fs_name("main") else {
        return false;
    };
    let connect = interprocess::local_socket::tokio::Stream::connect(name);
    // A live supervisor accepts at once (the kernel queues the connection);
    // the timeout only guards against a platform where connecting blocks.
    match tokio::time::timeout(std::time::Duration::from_secs(1), connect).await {
        Ok(Ok(_)) => true,
        Ok(Err(err)) => {
            trace!("no supervisor listening on the IPC socket: {err}");
            false
        }
        Err(_) => {
            debug!("timed out probing the IPC socket; treating it as not listening");
            false
        }
    }
}

/// Encode an IPC message as JSON.
///
/// Messages are framed by a trailing NUL byte, which JSON text never contains.
/// A binary encoding would, so it cannot use this framing.
fn serialize<T: serde::Serialize>(msg: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(msg)
        .into_diagnostic()
        .wrap_err("failed to serialize IPC message as JSON")
}

fn deserialize<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let mut bytes = bytes.to_vec();
    bytes.pop();
    let preview = std::str::from_utf8(&bytes).unwrap_or("<binary>");
    trace!("msg: {preview:?}");
    serde_json::from_slice(&bytes)
        .into_diagnostic()
        .wrap_err("failed to deserialize IPC JSON response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn socket_path_over_sun_path_capacity_names_path_length_and_fix() {
        let fits = PathBuf::from(format!("/{}", "a".repeat(SOCKET_PATH_CAPACITY - 1)));
        assert_eq!(fits.as_os_str().len(), SOCKET_PATH_CAPACITY);
        assert!(check_socket_path(&fits, SOCKET_PATH_CAPACITY).is_ok());

        let long = PathBuf::from(format!(
            "/{}/sock/main.sock",
            "a".repeat(SOCKET_PATH_CAPACITY)
        ));
        let err = check_socket_path(&long, SOCKET_PATH_CAPACITY).unwrap_err();
        let len = long.as_os_str().len();
        let message = err.to_string();
        assert!(message.contains(&format!("{len} bytes")), "{message}");
        assert!(
            message.contains(&format!("allows {SOCKET_PATH_CAPACITY}")),
            "{message}"
        );
        let help = err.help().expect("help text").to_string();
        assert!(help.contains(&long.display().to_string()), "{help}");
        assert!(help.contains("PITCHFORK_STATE_DIR"), "{help}");
        assert!(help.contains("XDG_STATE_HOME"), "{help}");
        assert!(
            matches!(
                err.downcast_ref::<IpcError>(),
                Some(IpcError::SocketPathTooLong { len: l, limit, .. })
                    if *l == len && *limit == SOCKET_PATH_CAPACITY
            ),
            "{err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn socket_path_capacity_matches_the_platform() {
        // 108 on Linux, 104 on macOS and the BSDs.
        assert!((100..=108).contains(&SOCKET_PATH_CAPACITY));
    }

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

    fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(value: &T) -> T {
        let mut bytes = serialize(value).unwrap();
        bytes.push(b'\n');
        deserialize(&bytes).unwrap()
    }

    #[test]
    fn claim_daemons_ipc_round_trips() {
        let ids = vec![DaemonId::new("proj", "api"), DaemonId::new("proj", "db")];
        match round_trip(&IpcRequest::ClaimDaemons { ids: ids.clone() }) {
            IpcRequest::ClaimDaemons { ids: decoded } => assert_eq!(decoded, ids),
            other => panic!("unexpected request: {other:?}"),
        }
    }

    /// The idle-shutdown ownership crosses IPC in both structs, alongside
    /// fields that are skipped when empty.
    #[test]
    fn proxy_idle_timeout_survives_the_ipc_encoding() {
        let daemon = Daemon {
            id: DaemonId::new("proj", "api"),
            proxy_idle_timeout_ms: Some(900_000),
            ..Default::default()
        };
        match round_trip(&IpcResponse::ActiveDaemons(vec![daemon])) {
            IpcResponse::ActiveDaemons(daemons) => {
                assert_eq!(daemons[0].proxy_idle_timeout_ms, Some(900_000));
                assert!(!daemons[0].oneshot);
            }
            other => panic!("unexpected response: {other:?}"),
        }

        let opts = RunOptions {
            id: DaemonId::new("proj", "api"),
            proxy_idle_timeout_ms: Some(900_000),
            ..Default::default()
        };
        match round_trip(&IpcRequest::Run(opts)) {
            IpcRequest::Run(opts) => assert_eq!(opts.proxy_idle_timeout_ms, Some(900_000)),
            other => panic!("unexpected request: {other:?}"),
        }
    }

    /// A start replaces the saved record only when it says so: a request from
    /// an older client, which does not send the flag, keeps merging.
    #[test]
    fn replaces_saved_record_is_opt_in_across_ipc() {
        let full = RunOptions {
            id: DaemonId::new("proj", "api"),
            replaces_saved_record: true,
            ..Default::default()
        };
        match round_trip(&IpcRequest::Run(full)) {
            IpcRequest::Run(opts) => assert!(opts.replaces_saved_record),
            other => panic!("unexpected request: {other:?}"),
        }

        let older_client = RunOptions {
            id: DaemonId::new("proj", "api"),
            ..Default::default()
        };
        let bytes = serialize(&IpcRequest::Run(older_client)).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("replaces_saved_record"));
        match round_trip(&IpcRequest::Run(RunOptions {
            id: DaemonId::new("proj", "api"),
            ..Default::default()
        })) {
            IpcRequest::Run(opts) => assert!(!opts.replaces_saved_record),
            other => panic!("unexpected request: {other:?}"),
        }
    }
}
