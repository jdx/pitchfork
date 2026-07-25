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
}

impl LogSink {
    pub async fn run(&self) -> Result<()> {
        let id = DaemonId::parse(&self.daemon_id)?;

        // Reading and writing are separate tasks. A write to SQLite can block —
        // for as long as the store's busy timeout, if another writer holds the
        // lock — and this process is the only reader of the daemon's pipe, so a
        // write must never stop it being drained.
        let (tx, rx) = tokio::sync::mpsc::channel::<ParsedLog>(QUEUE_DEPTH);
        let writer = tokio::spawn(write_batches(id.clone(), rx));

        let read_result = read_lines(tx, &self.log_format).await;

        // The sender has been dropped by now, so the writer drains its queue and
        // returns; wait for it so nothing queued is lost on exit.
        let _ = writer.await;

        read_result.map_err(|e| {
            miette::miette!("log sink for {id} could not read the daemon's output: {e}")
        })
    }
}

/// Split the daemon's output into lines and queue them for writing.
///
/// Returns once the pipe reaches end of file. A read error is propagated so the
/// process can exit non-zero: exiting cleanly would tell the supervisor the
/// stream had finished and it would stop replacing this sink.
async fn read_lines(
    tx: tokio::sync::mpsc::Sender<ParsedLog>,
    log_format: &str,
) -> std::io::Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut chunk = vec![0u8; READ_CHUNK];
    let mut line: Vec<u8> = Vec::with_capacity(256);

    loop {
        let read = stdin.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        for &byte in &chunk[..read] {
            if byte == b'\n' {
                if queue(&tx, &mut line, log_format).await.is_err() {
                    return Ok(());
                }
            } else {
                line.push(byte);
                // Emit an over-long run as its own line rather than letting the
                // buffer grow without bound.
                if line.len() >= MAX_LINE_BYTES && queue(&tx, &mut line, log_format).await.is_err()
                {
                    return Ok(());
                }
            }
        }
    }

    // Anything written without a trailing newline is still output.
    if !line.is_empty() {
        let _ = queue(&tx, &mut line, log_format).await;
    }
    Ok(())
}

/// Parse `line` and hand it to the writer, clearing it either way.
async fn queue(
    tx: &tokio::sync::mpsc::Sender<ParsedLog>,
    line: &mut Vec<u8>,
    log_format: &str,
) -> std::result::Result<(), ()> {
    // Convert lossily: a daemon emitting a stray non-UTF-8 byte must not be able
    // to stop its own logging.
    let text = String::from_utf8_lossy(line);
    let parsed = crate::log_parse::parse(text.trim_end_matches('\r'), log_format);
    line.clear();
    tx.send(parsed).await.map_err(|_| ())
}

/// Write queued lines in batches until the queue closes.
async fn write_batches(id: DaemonId, mut rx: tokio::sync::mpsc::Receiver<ParsedLog>) {
    let mut batch: Vec<ParsedLog> = Vec::with_capacity(BATCH_SIZE);
    let mut flush_interval = tokio::time::interval(FLUSH_INTERVAL);
    flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let closed = tokio::select! {
            received = rx.recv_many(&mut batch, BATCH_SIZE) => received == 0,
            _ = flush_interval.tick() => false,
        };
        flush(&id, &mut batch).await;
        if closed {
            break;
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
