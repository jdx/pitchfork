use crate::Result;
use crate::daemon_id::DaemonId;
use crate::log_parse::ParsedLog;
use crate::log_store::LogStore;
use crate::log_store::sqlite::LOG_STORE;
use tokio::io::AsyncReadExt;

/// Number of parsed lines to accumulate before writing them as one batch.
const BATCH_SIZE: usize = 100;

/// Longest a parsed line waits in the batch before being written.
const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// How many parsed lines may be queued for writing before reading slows down.
///
/// Reading and writing run separately so a slow write cannot stop the pipe being
/// drained, but the queue is bounded: output that sustainably outpaces the store
/// has to push back on the daemon eventually, which is preferable to growing
/// without limit.
const QUEUE_DEPTH: usize = 8192;

/// Bytes read from the pipe at a time.
const READ_CHUNK: usize = 8192;

/// Longest run of bytes treated as a single line.
///
/// Output containing no newline must not accumulate indefinitely: a daemon
/// emitting an endless stream, or binary data, would otherwise grow the buffer
/// until the sink was killed for using too much memory — whereupon the
/// supervisor would start another sink and repeat it.
const MAX_LINE_BYTES: usize = 64 * 1024;

/// Reads a daemon's output on stdin and writes it to the log store
///
/// Spawned by the supervisor as a sibling of the daemon, holding the read end
/// of the daemon's output pipe. Keeping the reader in its own process is what
/// makes logging survive a supervisor crash: the pipe still has a reader, so
/// the daemon is neither killed by SIGPIPE nor blocked, and no output is lost.
/// Exits when the pipe reaches end of file, which happens once the daemon and
/// every descendant holding the write end have gone.
#[derive(Debug, clap::Args)]
#[clap(hide = true, verbatim_doc_comment)]
pub struct LogSink {
    /// Qualified id of the daemon whose output this is
    #[clap(long)]
    daemon_id: String,

    /// Log format to parse lines with (`json`, `logfmt`, `auto`, or `text`)
    #[clap(long, default_value = "text")]
    log_format: String,

    /// Regex whose first match means the daemon is ready
    ///
    /// Set for a daemon configured with `ready_output`. The supervisor cannot
    /// match it itself — this process holds the output — so the match is
    /// reported back over IPC.
    #[clap(long)]
    ready_pattern: Option<String>,
}

impl LogSink {
    pub async fn run(&self) -> Result<()> {
        let id = DaemonId::parse(&self.daemon_id)?;

        // A pattern that does not compile is reported and then ignored, rather
        // than failing the sink: refusing to start would leave the daemon's
        // output unread, which is far worse than a readiness check that never
        // fires. The supervisor validates patterns too, so this is a backstop.
        let ready_pattern = self.ready_pattern.as_deref().and_then(|p| {
            regex::Regex::new(p)
                .map_err(|e| error!("log sink for {id} ignoring unparsable ready pattern: {e}"))
                .ok()
        });

        // Reading and writing are separate tasks. A write to SQLite can block —
        // for as long as the store's busy timeout, if another writer holds the
        // lock — and this process is the only reader of the daemon's pipe, so a
        // write must never stop it being drained.
        let (tx, rx) = tokio::sync::mpsc::channel::<SinkEvent>(QUEUE_DEPTH);
        let writer = tokio::spawn(write_batches(id.clone(), rx));

        let read_result = read_lines(tx, &self.log_format, ready_pattern).await;

        // The sender has been dropped by now, so the writer drains its queue and
        // returns; wait for it so nothing queued is lost on exit.
        let _ = writer.await;

        read_result.map_err(|e| {
            miette::miette!("log sink for {id} could not read the daemon's output: {e}")
        })
    }
}

/// Something for the writer task to do, in the order the reader saw it.
///
/// Reporting a readiness match travels the same queue as the lines rather than
/// jumping ahead of them, so the line that triggered the match is always in the
/// log store by the time the supervisor hears about it — `collect_startup_logs`
/// and `pitchfork logs` would otherwise be able to miss it.
enum SinkEvent {
    Line(ParsedLog),
    ReadyMatch(String),
}

/// Split the daemon's output into lines and queue them for writing.
///
/// Returns once the pipe reaches end of file. A read error is propagated so the
/// process can exit non-zero: exiting cleanly would tell the supervisor the
/// stream had finished and it would stop replacing this sink.
async fn read_lines(
    tx: tokio::sync::mpsc::Sender<SinkEvent>,
    log_format: &str,
    mut ready_pattern: Option<regex::Regex>,
) -> std::io::Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut chunk = vec![0u8; READ_CHUNK];
    let mut line: Vec<u8> = Vec::with_capacity(256);
    // Whether the last line was emitted because it reached the cap rather than
    // because it ended. A newline arriving straight afterwards terminates the
    // line already written, so it must not produce an empty one.
    let mut split_at_cap = false;

    loop {
        let read = stdin.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        for &byte in &chunk[..read] {
            if byte == b'\n' {
                if split_at_cap && line.is_empty() {
                    split_at_cap = false;
                    continue;
                }
                split_at_cap = false;
                queue(&tx, &mut line, log_format, &mut ready_pattern).await?;
            } else {
                line.push(byte);
                split_at_cap = false;
                // Emit an over-long run as its own line rather than letting the
                // buffer grow without bound.
                if line.len() >= MAX_LINE_BYTES {
                    queue_capped(&tx, &mut line, log_format, &mut ready_pattern).await?;
                    split_at_cap = true;
                }
            }
        }
    }

    // Anything written without a trailing newline is still output.
    if !line.is_empty() {
        queue(&tx, &mut line, log_format, &mut ready_pattern).await?;
    }
    Ok(())
}

/// Emit a line that has reached the length cap, keeping any trailing bytes that
/// form an incomplete character.
///
/// Splitting purely by byte count would cut a multi-byte character in half, and
/// converting each half on its own turns one valid character into two
/// replacement characters.
async fn queue_capped(
    tx: &tokio::sync::mpsc::Sender<SinkEvent>,
    line: &mut Vec<u8>,
    log_format: &str,
    ready_pattern: &mut Option<regex::Regex>,
) -> std::io::Result<()> {
    let split = split_before_incomplete_char(line);
    let tail = line.split_off(split);
    let result = queue(tx, line, log_format, ready_pattern).await;
    *line = tail;
    result
}

/// Length to cut `bytes` at so no character is left half-written.
///
/// Decided by inspecting the final bytes rather than by asking `from_utf8` where
/// the string stops being valid: that reports the *first* problem, so a single
/// invalid byte earlier in the line would hide an unfinished character at the
/// end, and the character would be split after all.
fn split_before_incomplete_char(bytes: &[u8]) -> usize {
    let len = bytes.len();
    // A character is at most four bytes, so only the last few can be unfinished.
    for i in (len.saturating_sub(4)..len).rev() {
        let byte = bytes[i];
        if byte & 0b1100_0000 == 0b1000_0000 {
            continue; // a continuation byte; keep looking back for its lead
        }
        let expected = match byte {
            0x00..=0x7f => 1,
            b if b >> 5 == 0b110 => 2,
            b if b >> 4 == 0b1110 => 3,
            b if b >> 3 == 0b11110 => 4,
            // Not a valid lead byte at all, so nothing is pending; the lossy
            // conversion will render it.
            _ => 1,
        };
        return if i + expected > len && i > 0 { i } else { len };
    }
    len
}

/// Parse `line` and hand it to the writer, clearing it either way.
///
/// A closed queue means the writer task is gone, which is a failure rather than
/// the end of the stream: reporting it as success would tell the supervisor this
/// sink had reached end of file, and it would stop replacing it while the daemon
/// was still writing.
async fn queue(
    tx: &tokio::sync::mpsc::Sender<SinkEvent>,
    line: &mut Vec<u8>,
    log_format: &str,
    ready_pattern: &mut Option<regex::Regex>,
) -> std::io::Result<()> {
    // Convert lossily: a daemon emitting a stray non-UTF-8 byte must not be able
    // to stop its own logging.
    let text = String::from_utf8_lossy(line);
    let text = text.trim_end_matches('\r');
    let parsed = crate::log_parse::parse(text, log_format);
    // Strip ANSI before matching so a pattern works whether or not the daemon
    // colours its output, matching what in-process capture did.
    let matched = ready_pattern.as_ref().and_then(|re| {
        let clean = console::strip_ansi_codes(text);
        re.is_match(&clean).then(|| clean.into_owned())
    });
    line.clear();

    tx.send(SinkEvent::Line(parsed))
        .await
        .map_err(|_| std::io::Error::other("log writer stopped"))?;
    if let Some(matched) = matched {
        // Only the first match means anything; readiness happens once.
        *ready_pattern = None;
        tx.send(SinkEvent::ReadyMatch(matched))
            .await
            .map_err(|_| std::io::Error::other("log writer stopped"))?;
    }
    Ok(())
}

/// Write queued lines in batches until the queue closes.
async fn write_batches(id: DaemonId, mut rx: tokio::sync::mpsc::Receiver<SinkEvent>) {
    let mut events: Vec<SinkEvent> = Vec::with_capacity(BATCH_SIZE);
    let mut batch: Vec<ParsedLog> = Vec::with_capacity(BATCH_SIZE);
    let mut flush_interval = tokio::time::interval(FLUSH_INTERVAL);
    flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let closed = tokio::select! {
            received = rx.recv_many(&mut events, BATCH_SIZE) => received == 0,
            _ = flush_interval.tick() => false,
        };
        for event in events.drain(..) {
            match event {
                SinkEvent::Line(parsed) => batch.push(parsed),
                SinkEvent::ReadyMatch(line) => {
                    // Everything up to and including the matching line goes to
                    // the store before the supervisor is told, so whatever it
                    // does next can already read it.
                    flush(&id, &mut batch).await;
                    report_ready_match(&id, line).await;
                }
            }
        }
        flush(&id, &mut batch).await;
        if closed {
            break;
        }
    }
}

/// Tell the supervisor that the daemon's output matched its readiness pattern.
///
/// Failure is logged and dropped. There is no supervisor to retry against if it
/// has crashed — and if it has, this daemon's readiness is no longer anyone's
/// concern — while the daemon's output keeps being captured either way.
async fn report_ready_match(id: &DaemonId, line: String) {
    // `autostart: false` — a sink must never bring a supervisor into being.
    match crate::ipc::client::IpcClient::connect(false).await {
        Ok(client) => {
            if let Err(e) = client.sink_ready_match(id.clone(), line).await {
                warn!("log sink for {id} could not report its readiness match: {e}");
            }
        }
        Err(e) => {
            warn!("log sink for {id} could not reach the supervisor to report readiness: {e}");
        }
    }
}

/// Write one batch, off the runtime so the SQLite call cannot stall other tasks.
async fn flush(id: &DaemonId, batch: &mut Vec<ParsedLog>) {
    if batch.is_empty() {
        return;
    }
    let daemon_id = id.clone();
    let entries = std::mem::take(batch);
    let written = tokio::task::spawn_blocking(move || {
        LOG_STORE.append_structured_batch(&daemon_id, &entries)
    })
    .await;
    if let Ok(Err(e)) = written {
        // Nothing useful to do but report it: the supervisor is not necessarily
        // alive to be told, and dropping a batch is preferable to stalling the
        // daemon behind a pipe nobody is draining.
        error!("log sink failed to write batch for {id}: {e}");
    }
}
