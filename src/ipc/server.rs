use crate::ipc::{IpcRequest, IpcResponse, deserialize, fs_name, serialize};
use crate::settings::settings;
use crate::{Result, env};
use interprocess::local_socket::ListenerOptions;
use interprocess::local_socket::tokio::{RecvHalf, SendHalf};
use interprocess::local_socket::traits::tokio::Listener;
use interprocess::local_socket::traits::tokio::Stream;
use miette::{IntoDiagnostic, bail, miette};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::oneshot;

/// Rate limiter for IPC connections to prevent local DoS attacks.
/// Uses a sliding window algorithm to limit requests per second.
struct RateLimiter {
    /// Timestamps of recent requests within the window
    requests: Vec<Instant>,
    /// Maximum requests allowed per window
    max_requests: usize,
    /// Window duration in milliseconds
    window_ms: u64,
}

impl RateLimiter {
    fn new(max_requests: usize, window_ms: u64) -> Self {
        Self {
            requests: Vec::with_capacity(max_requests),
            max_requests,
            window_ms,
        }
    }

    /// Check if a request is allowed. Returns true if allowed, false if rate limited.
    fn check(&mut self) -> bool {
        let now = Instant::now();
        let window = std::time::Duration::from_millis(self.window_ms);

        // Remove expired timestamps
        self.requests.retain(|&t| now.duration_since(t) < window);

        if self.requests.len() >= self.max_requests {
            false
        } else {
            self.requests.push(now);
            true
        }
    }
}

pub struct IpcServer {
    // clients: Mutex<HashMap<String, interprocess::local_socket::tokio::Stream>>,
    rx: Receiver<(IpcRequest, Sender<IpcResponse>)>,
}

/// Handle for triggering graceful shutdown of the IPC server
pub struct IpcServerHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl IpcServerHandle {
    /// Signal the IPC server to shut down gracefully
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl IpcServer {
    pub async fn new() -> Result<(Self, IpcServerHandle)> {
        // Unix: bind to a per-process claim path first, then hard-link it onto
        // the canonical socket path (see `publish_claim`) — that link is the
        // atomic ownership claim. Windows: named pipes exist in a flat kernel
        // namespace — no files to create or clean up.
        #[cfg(unix)]
        // No dots: `fs_name` passes the name through `with_extension`, which
        // would replace anything after a dot instead of appending the suffix.
        let bind_name = format!("main-claim-{}", std::process::id());
        #[cfg(windows)]
        let bind_name = "main".to_string();
        #[cfg(unix)]
        let claim_path = env::IPC_SOCK_DIR.join(format!("{bind_name}.sock"));

        #[cfg(unix)]
        {
            xx::file::mkdirp(&*env::IPC_SOCK_DIR)?;
            // Only a crashed predecessor with this exact pid can have left a
            // claim file behind; clear it so the bind below succeeds.
            let _ = xx::file::remove_file(&claim_path);
        }

        let opts = ListenerOptions::new().name(fs_name(&bind_name)?);
        #[cfg(windows)]
        debug!("Listening on named pipe");
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        // Set restrictive umask before creating socket to avoid TOCTOU race condition.
        // This ensures the socket is created with 0600 permissions from the start.
        // Note: IpcServer::new() is called during supervisor startup before other async
        // tasks are spawned, so the brief umask change won't affect concurrent operations.
        #[cfg(unix)]
        let old_umask = unsafe { libc::umask(0o077) };

        let listener_result = opts.create_tokio();

        // Always restore original umask, even if socket creation failed
        #[cfg(unix)]
        unsafe {
            libc::umask(old_umask);
        }

        let listener = listener_result.into_diagnostic()?;

        // Publish the bound claim socket onto the canonical path. Failing here
        // (a live supervisor owns the path) aborts startup before the caller
        // records anything in the state file.
        #[cfg(unix)]
        {
            Self::publish_claim(&claim_path).await?;
            debug!("Listening on {}", env::IPC_SOCK_MAIN.display());
        }

        // When the supervisor is started as root, the socket file and directory
        // are owned by root with restrictive permissions (0600/0700). Non-root CLI
        // clients and configured daemon users need to connect to this socket.
        //
        // Prefer `[settings.supervisor] user`, then SUDO_UID/SUDO_GID, so
        // permissions stay tight (0700/0600) while the intended runtime user
        // owns the socket.
        #[cfg(unix)]
        {
            if let Some((uid, gid)) = crate::supervisor::state_owner_ids() {
                let _ = chown_path(&env::IPC_SOCK_DIR, uid, gid);
                let _ = chown_path(&env::IPC_SOCK_MAIN, uid, gid);
                debug!("chowned IPC socket to uid={uid} gid={gid}");
            }
        }

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = &mut shutdown_rx => {
                        debug!("IPC server received shutdown signal");
                        break;
                    }
                    result = listener.accept() => {
                        match result {
                            Ok(stream) => {
                                trace!("Client accepted");
                                let (recv, send) = stream.split();
                                let mut incoming_chan = Self::read_messages_chan(recv);
                                let outgoing_chan = Self::send_messages_chan(send);
                                let tx = tx.clone();
                                tokio::spawn(async move {
                                    while let Some(req) = incoming_chan.recv().await {
                                        if let Err(err) = tx.send((req, outgoing_chan.clone())).await {
                                            debug!("Failed to send message: {err:?}");
                                            break;
                                        }
                                    }
                                    trace!("IPC connection handler task terminated cleanly");
                                });
                            }
                            Err(err) => {
                                error!("ipc server accept error: {err:?}");
                            }
                        }
                    }
                }
            }
            // Clean up socket file on graceful shutdown (Unix only)
            #[cfg(unix)]
            let _ = std::fs::remove_file(&*env::IPC_SOCK_MAIN);
            debug!("IPC server shut down cleanly");
        });
        let server = Self { rx };
        let handle = IpcServerHandle {
            shutdown_tx: Some(shutdown_tx),
        };
        Ok((server, handle))
    }

    async fn send(send: &mut SendHalf, msg: IpcResponse) -> Result<()> {
        let mut msg = serialize(&msg)?;
        if msg.contains(&0) {
            bail!("IPC message contains null byte");
        }
        msg.push(0);
        send.write_all(&msg).await.into_diagnostic()?;
        Ok(())
    }

    /// Read raw bytes from socket until null terminator (without deserializing)
    async fn read_raw_message(recv: &mut BufReader<RecvHalf>) -> Result<Option<Vec<u8>>> {
        let mut bytes = Vec::new();
        recv.read_until(0, &mut bytes).await.into_diagnostic()?;
        if bytes.is_empty() {
            return Ok(None);
        }
        Ok(Some(bytes))
    }

    fn read_messages_chan(recv: RecvHalf) -> Receiver<IpcRequest> {
        let mut recv = BufReader::new(recv);
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            // Rate limit: use configured requests per configured window
            // This is generous for normal CLI usage but prevents flooding
            let s = settings();
            let window_ms = u64::try_from(s.ipc_rate_limit_window().as_millis()).unwrap_or(1000);
            let window_ms = if window_ms < 100 {
                warn!(
                    "ipc.rate_limit_window is {window_ms}ms which is too small (< 100ms), \
                    clamping to 100ms to avoid effectively disabling rate limiting"
                );
                100
            } else {
                window_ms
            };
            let max_requests = match usize::try_from(s.ipc.rate_limit) {
                Ok(0) => {
                    warn!("ipc.rate_limit is 0, which would block all IPC requests; clamping to 1");
                    1
                }
                Ok(n) => n,
                Err(_) => {
                    warn!(
                        "ipc.rate_limit value {} is out of range, clamping to 100",
                        s.ipc.rate_limit
                    );
                    100
                }
            };
            let mut rate_limiter = RateLimiter::new(max_requests, window_ms);

            loop {
                // Check rate limit BEFORE reading to avoid wasting CPU on deserialization
                // when rate limited. We still need to drain the socket to prevent buffer
                // buildup, but we skip the costly deserialization step.
                let is_rate_limited = !rate_limiter.check();

                // Read raw bytes from socket
                let bytes = match Self::read_raw_message(&mut recv).await {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) => {
                        trace!("Client disconnected");
                        break;
                    }
                    Err(err) => {
                        // I/O errors are not rate-limited (they indicate connection issues)
                        debug!("Failed to read from socket: {err:?}");
                        break;
                    }
                };

                // If rate limited, drop the message without deserializing
                if is_rate_limited {
                    warn!("IPC client rate limited, dropping message");
                    continue;
                }

                // Deserialize the message
                let msg = match deserialize(&bytes) {
                    Ok(msg) => {
                        trace!("Received message: {msg:?}");
                        msg
                    }
                    Err(err) => {
                        // Send an Invalid request so the handler can respond with an error
                        warn!("Failed to deserialize message: {err:?}");
                        IpcRequest::Invalid {
                            error: format!("{err:#}"),
                        }
                    }
                };

                if let Err(err) = tx.send(msg).await {
                    warn!("Failed to emit message: {err:?}");
                    break;
                }
            }
            trace!("IPC read task terminated cleanly");
        });
        rx
    }

    fn send_messages_chan(mut send: SendHalf) -> Sender<IpcResponse> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            loop {
                let msg = match rx.recv().await {
                    Some(msg) => {
                        trace!("Sending message: {msg:?}");
                        msg
                    }
                    None => {
                        trace!("IPC channel closed");
                        break;
                    }
                };
                if let Err(err) = Self::send(&mut send, msg).await {
                    // Broken-pipe / reset is expected when a client disconnects normally
                    // Traverse the error source chain to find the original io::Error
                    // since miette wraps it in a DiagnosticError
                    use std::error::Error as StdError;
                    let is_disconnect = {
                        let mut cur: Option<&dyn StdError> = Some(err.as_ref() as &dyn StdError);
                        let mut found = false;
                        while let Some(e) = cur {
                            if let Some(io) = e.downcast_ref::<std::io::Error>() {
                                found = matches!(
                                    io.kind(),
                                    std::io::ErrorKind::BrokenPipe
                                        | std::io::ErrorKind::ConnectionReset
                                );
                                break;
                            }
                            cur = e.source();
                        }
                        found
                    };
                    if is_disconnect {
                        debug!("IPC client disconnected: {err:?}");
                    } else {
                        warn!("Failed to send message: {err:?}");
                    }
                    break;
                }
            }
            trace!("IPC send task terminated cleanly");
        });
        tx
    }

    /// Hard-link the freshly bound claim socket onto the canonical IPC socket
    /// path, atomically claiming supervisor ownership.
    ///
    /// `link(2)` fails with `EEXIST` when any file already owns the path,
    /// which is the mutual exclusion against a concurrent fresh claim. A live
    /// incumbent is detected through that failure plus a connection probe —
    /// a socket on the canonical path always belongs to a listening process,
    /// because the link only happens after bind + listen — and startup
    /// refuses to take over. Only a refused connection proves an incumbent
    /// dead, and the stale-file replacement (probe → remove → link retry)
    /// runs under an inter-process lock: the probe/remove pair is not atomic,
    /// so without serialization a contender suspended between its own probe
    /// and its removal could unlink a live socket another contender published
    /// in between.
    ///
    /// The protocol runs on the blocking pool via `spawn_blocking`: the
    /// inter-process lock blocks until the holder releases, which must never
    /// stall a Tokio worker thread.
    #[cfg(unix)]
    async fn publish_claim(claim_path: &std::path::Path) -> Result<()> {
        let claim_path = claim_path.to_owned();
        let main_sock = env::IPC_SOCK_MAIN.to_path_buf();
        tokio::task::spawn_blocking(move || Self::publish_claim_blocking(claim_path, main_sock))
            .await
            .into_diagnostic()?
    }

    #[cfg(unix)]
    fn publish_claim_blocking(
        claim_path: std::path::PathBuf,
        main_sock: std::path::PathBuf,
    ) -> Result<()> {
        fn discard_claim(claim_path: &std::path::Path) {
            let _ = xx::file::remove_file(claim_path);
        }

        enum Owner {
            Live,
            Dead,
            Unknown(std::io::Error),
        }

        fn probe_owner(main_sock: &std::path::Path) -> Owner {
            match std::os::unix::net::UnixStream::connect(main_sock) {
                Ok(_) => Owner::Live,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    Owner::Dead
                }
                Err(e) => Owner::Unknown(e),
            }
        }

        // Fast path: the canonical path is free, and link(2) claims it
        // atomically against every contender.
        match std::fs::hard_link(&claim_path, &main_sock) {
            Ok(()) => {
                discard_claim(&claim_path);
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                discard_claim(&claim_path);
                return Err(e).into_diagnostic();
            }
        }

        // The path is owned. Serialize the replacement protocol across
        // contenders (same flock scheme as the state file). The lock is held
        // for a few syscalls only.
        let _claim_lock =
            xx::fslock::get(&main_sock, false)?.expect("fslock::get returns a lock unless forced");

        for _ in 0..3 {
            match probe_owner(&main_sock) {
                // Never silently take over a socket owned by a live
                // supervisor: unlinking the file would strand the running
                // instance while we steal new clients (split brain).
                Owner::Live => {
                    discard_claim(&claim_path);
                    bail!(
                        "another pitchfork supervisor is listening on {}; refusing to take over \
                         (use `pitchfork supervisor stop` first, or `--force` to replace it)",
                        main_sock.display()
                    );
                }
                // e.g. permission denied: the owner cannot be proven dead, so
                // leave the file alone and fail startup.
                Owner::Unknown(e) => {
                    discard_claim(&claim_path);
                    bail!(
                        "cannot probe existing IPC socket {}: {e}",
                        main_sock.display()
                    );
                }
                Owner::Dead => {
                    // Refused connection (or no file at all) proves the path
                    // stale; replace it.
                    debug!("removing stale IPC socket file {}", main_sock.display());
                    let _ = xx::file::remove_file(&main_sock);
                    match std::fs::hard_link(&claim_path, &main_sock) {
                        Ok(()) => {
                            discard_claim(&claim_path);
                            return Ok(());
                        }
                        // A lock-free fresh claimer won the freed path in
                        // between; the next iteration's probe decides
                        // whether it is live.
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(e) => {
                            discard_claim(&claim_path);
                            return Err(e).into_diagnostic();
                        }
                    }
                }
            }
        }
        discard_claim(&claim_path);
        bail!(
            "failed to claim IPC socket path {} after repeated attempts",
            main_sock.display()
        );
    }

    pub async fn read(&mut self) -> Result<(IpcRequest, Sender<IpcResponse>)> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| miette!("IPC channel closed"))
    }

    pub fn close(&self) {
        debug!("Closing IPC server");
        // Synchronous by necessity: this runs from `Drop` and signal-handler
        // contexts, which cannot await. The graceful shutdown path removes
        // the socket inside the accept task instead.
        #[cfg(unix)]
        let _ = std::fs::remove_file(&*env::IPC_SOCK_MAIN);
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.close();
    }
}

/// `chown` a single path using libc. Returns Ok(()) on success.
#[cfg(unix)]
fn chown_path(path: &std::path::Path, uid: u32, gid: u32) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let ret = unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
