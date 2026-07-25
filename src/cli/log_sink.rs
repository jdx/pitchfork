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

    /// Token identifying the start attempt this sink belongs to
    ///
    /// Quoted back when reporting a match. The supervisor drops reports whose
    /// token is no longer current, so a sink still draining a failed attempt
    /// cannot mark that daemon's retry ready.
    #[clap(long, default_value_t = 0)]
    relay_token: u64,

    /// Report lines so the supervisor can fire the daemon's `on_output` hook
    ///
    /// Without `--output-filter` or `--output-regex` every line qualifies,
    /// which is what a hook with no pattern asks for.
    #[clap(long)]
    report_output: bool,

    /// Only report lines containing this substring
    #[clap(long)]
    output_filter: Option<String>,

    /// Only report lines matching this regex
    #[clap(long)]
    output_regex: Option<String>,

    /// Shortest gap between reported lines, in milliseconds
    #[clap(long, default_value_t = 1000)]
    output_debounce_ms: u64,
}

impl LogSink {
    pub async fn run(&self) -> Result<()> {
        let id = DaemonId::parse(&self.daemon_id)?;

        // A pattern that does not compile is reported and then ignored, rather
        // than failing the sink: refusing to start would leave the daemon's
        // output unread, which is far worse than a readiness check that never
        // fires. The supervisor validates patterns too, so this is a backstop.
        let compile = |what: &str, pattern: &str| {
            regex::Regex::new(pattern)
                .map_err(|e| error!("log sink for {id} ignoring unparsable {what}: {e}"))
                .ok()
        };
        let ready_pattern = self
            .ready_pattern
            .as_deref()
            .and_then(|p| compile("ready pattern", p));
        let hook = self.report_output.then(|| HookMatcher {
            filter: self.output_filter.clone(),
            regex: self
                .output_regex
                .as_deref()
                .and_then(|p| compile("output pattern", p)),
            debounce: std::time::Duration::from_millis(self.output_debounce_ms),
            last_reported: None,
        });

        // Reading and writing are separate tasks. A write to SQLite can block —
        // for as long as the store's busy timeout, if another writer holds the
        // lock — and this process is the only reader of the daemon's pipe, so a
        // write must never stop it being drained.
        let (tx, rx) = tokio::sync::mpsc::channel::<SinkEvent>(QUEUE_DEPTH);
        let writer = tokio::spawn(write_batches(id.clone(), self.relay_token, rx));

        let read_result =
            read_lines(tx, &self.log_format, ReadyMatcher::new(ready_pattern, hook)).await;

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

/// How much of a capped line is carried forward for matching.
///
/// A line longer than [`MAX_LINE_BYTES`] is emitted in pieces, and a readiness
/// pattern straddling a split would match none of them — the daemon would then
/// be killed at its readiness timeout despite having announced itself. Keeping
/// the tail of the previous piece closes that for any pattern shorter than this
/// while still bounding what is held.
const MATCH_CARRY_BYTES: usize = 4 * 1024;

/// Watches a daemon's output for the things the supervisor would look for if it
/// could still read the stream: the readiness pattern, and whatever fires the
/// `on_output` hook.
///
/// What the supervisor does about a reported line — mark the daemon ready, run
/// the hook — remains its own business.
struct ReadyMatcher {
    /// Readiness pattern, cleared once it has matched: readiness happens once.
    pattern: Option<regex::Regex>,
    /// The `on_output` hook's filter and rate limit, if the daemon has one.
    hook: Option<HookMatcher>,
    /// Tail of the previous piece of a line split at the length cap. Empty
    /// whenever the last piece ended at a real newline.
    carried: String,
}

/// The `on_output` hook's line filter and its rate limit.
struct HookMatcher {
    filter: Option<String>,
    regex: Option<regex::Regex>,
    debounce: std::time::Duration,
    last_reported: Option<std::time::Instant>,
}

impl HookMatcher {
    /// Whether this line should fire the hook, consuming the debounce window.
    ///
    /// The debounce is applied here rather than by the supervisor because this
    /// process is the one that sees every line: enforcing it here means one IPC
    /// message per window instead of one per line, which matters for a hook
    /// with no filter, where every line qualifies.
    fn matches(&mut self, clean: &str) -> bool {
        let matched = match (&self.filter, &self.regex) {
            // Mutually exclusive, and a hook setting both is rejected before it
            // ever reaches this process.
            (Some(substr), _) => clean.contains(substr.as_str()),
            (None, Some(re)) => re.is_match(clean),
            // A hook with neither fires on every line.
            (None, None) => true,
        };
        if !matched {
            return false;
        }
        let now = std::time::Instant::now();
        if self
            .last_reported
            .is_some_and(|last| now.duration_since(last) < self.debounce)
        {
            return false;
        }
        self.last_reported = Some(now);
        true
    }
}

impl ReadyMatcher {
    fn new(pattern: Option<regex::Regex>, hook: Option<HookMatcher>) -> Self {
        Self {
            pattern,
            hook,
            carried: String::new(),
        }
    }

    /// Whether anything is still being watched for. Once nothing is, matching is
    /// skipped: stripping ANSI from every line of a chatty daemon is not free.
    fn is_watching(&self) -> bool {
        self.pattern.is_some() || self.hook.is_some()
    }

    /// Test `text` — one whole line, or one piece of an over-long one — and
    /// return what matched, which is what the supervisor is told about.
    ///
    /// `split_at_cap` says another piece of the same logical line follows.
    fn consider(&mut self, text: &str, split_at_cap: bool) -> Option<String> {
        if !self.is_watching() {
            self.carried.clear();
            return None;
        }
        let clean = console::strip_ansi_codes(text);
        let candidate = if self.carried.is_empty() {
            clean.into_owned()
        } else {
            format!("{}{clean}", self.carried)
        };

        self.carried = if split_at_cap {
            let start = candidate.len().saturating_sub(MATCH_CARRY_BYTES);
            // Never split a character in half; walk forward to a boundary.
            let start = (start..candidate.len())
                .find(|i| candidate.is_char_boundary(*i))
                .unwrap_or(candidate.len());
            candidate[start..].to_string()
        } else {
            String::new()
        };

        // Both reasons to report produce the same message. The supervisor
        // re-checks the readiness pattern on what it is sent, so a line reported
        // for the hook that also announces readiness is still handled correctly.
        let mut report = false;
        if self
            .pattern
            .as_ref()
            .is_some_and(|re| re.is_match(&candidate))
        {
            self.pattern = None;
            report = true;
        }
        if let Some(hook) = self.hook.as_mut()
            && hook.matches(&candidate)
        {
            report = true;
        }
        // Send the text the patterns actually matched against, not just this
        // piece of it: the supervisor re-matches it, and hands it to the hook.
        report.then_some(candidate)
    }
}

/// Split the daemon's output into lines and queue them for writing.
///
/// Returns once the pipe reaches end of file. A read error is propagated so the
/// process can exit non-zero: exiting cleanly would tell the supervisor the
/// stream had finished and it would stop replacing this sink.
async fn read_lines(
    tx: tokio::sync::mpsc::Sender<SinkEvent>,
    log_format: &str,
    mut matcher: ReadyMatcher,
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
                queue(&tx, &mut line, log_format, &mut matcher).await?;
            } else {
                line.push(byte);
                split_at_cap = false;
                // Emit an over-long run as its own line rather than letting the
                // buffer grow without bound.
                if line.len() >= MAX_LINE_BYTES {
                    queue_capped(&tx, &mut line, log_format, &mut matcher).await?;
                    split_at_cap = true;
                }
            }
        }
    }

    // Anything written without a trailing newline is still output.
    if !line.is_empty() {
        queue(&tx, &mut line, log_format, &mut matcher).await?;
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
    matcher: &mut ReadyMatcher,
) -> std::io::Result<()> {
    let split = split_before_incomplete_char(line);
    let tail = line.split_off(split);
    let result = queue_piece(tx, line, log_format, matcher).await;
    *line = tail;
    result
}

/// Queue one piece of a line that hit the length cap, telling the matcher that
/// the rest of the logical line is still to come.
async fn queue_piece(
    tx: &tokio::sync::mpsc::Sender<SinkEvent>,
    line: &mut Vec<u8>,
    log_format: &str,
    matcher: &mut ReadyMatcher,
) -> std::io::Result<()> {
    let text = String::from_utf8_lossy(line);
    let text = text.trim_end_matches('\r');
    let parsed = crate::log_parse::parse(text, log_format);
    let matched = matcher.consider(text, true);
    line.clear();

    tx.send(SinkEvent::Line(parsed))
        .await
        .map_err(|_| std::io::Error::other("log writer stopped"))?;
    if let Some(matched) = matched {
        tx.send(SinkEvent::ReadyMatch(matched))
            .await
            .map_err(|_| std::io::Error::other("log writer stopped"))?;
    }
    Ok(())
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
    matcher: &mut ReadyMatcher,
) -> std::io::Result<()> {
    // Convert lossily: a daemon emitting a stray non-UTF-8 byte must not be able
    // to stop its own logging.
    let text = String::from_utf8_lossy(line);
    let text = text.trim_end_matches('\r');
    let parsed = crate::log_parse::parse(text, log_format);
    // Strip ANSI before matching so a pattern works whether or not the daemon
    // colours its output, matching what in-process capture did.
    let matched = matcher.consider(text, false);
    line.clear();

    tx.send(SinkEvent::Line(parsed))
        .await
        .map_err(|_| std::io::Error::other("log writer stopped"))?;
    if let Some(matched) = matched {
        tx.send(SinkEvent::ReadyMatch(matched))
            .await
            .map_err(|_| std::io::Error::other("log writer stopped"))?;
    }
    Ok(())
}

/// Write queued lines in batches until the queue closes.
async fn write_batches(
    id: DaemonId,
    relay_token: u64,
    mut rx: tokio::sync::mpsc::Receiver<SinkEvent>,
) {
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
                    report_ready_match(&id, relay_token, line).await;
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
async fn report_ready_match(id: &DaemonId, relay_token: u64, line: String) {
    // `autostart: false` — a sink must never bring a supervisor into being.
    match crate::ipc::client::IpcClient::connect(false).await {
        Ok(client) => {
            if let Err(e) = client.sink_ready_match(id.clone(), relay_token, line).await {
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

#[cfg(test)]
mod tests {
    use super::{HookMatcher, MATCH_CARRY_BYTES, ReadyMatcher};

    fn matcher(pattern: &str) -> ReadyMatcher {
        ReadyMatcher::new(Some(regex::Regex::new(pattern).unwrap()), None)
    }

    fn hook_matcher(filter: Option<&str>, debounce_ms: u64) -> ReadyMatcher {
        ReadyMatcher::new(
            None,
            Some(HookMatcher {
                filter: filter.map(str::to_string),
                regex: None,
                debounce: std::time::Duration::from_millis(debounce_ms),
                last_reported: None,
            }),
        )
    }

    #[test]
    fn reports_the_first_match_and_then_stops_looking() {
        let mut m = matcher("READY");
        assert_eq!(m.consider("starting up", false), None);
        assert_eq!(
            m.consider("READY to serve", false).as_deref(),
            Some("READY to serve")
        );
        // Readiness happens once; later matches are somebody else's business.
        assert_eq!(m.consider("READY again", false), None);
    }

    #[test]
    fn matches_a_pattern_split_across_the_line_cap() {
        // The daemon emitted one enormous line whose announcement straddles the
        // point where the sink had to cut it.
        let mut m = matcher("SERVER READY");
        assert_eq!(m.consider("....SERVER ", true), None);
        let matched = m
            .consider("READY....", false)
            .expect("should match across the split");
        assert!(matched.contains("SERVER READY"), "reported {matched:?}");
    }

    #[test]
    fn does_not_match_across_a_completed_line() {
        // Two separate lines are not one line: a pattern spanning them must not
        // match, or "SERVER" at the end of one line plus "READY" at the start of
        // the next would look like an announcement.
        let mut m = matcher("SERVER READY");
        assert_eq!(m.consider("SERVER ", false), None);
        assert_eq!(m.consider("READY", false), None);
    }

    #[test]
    fn carries_a_bounded_amount_of_a_capped_line() {
        let mut m = matcher("nothing-matches-this");
        m.consider(&"x".repeat(MATCH_CARRY_BYTES * 3), true);
        assert!(m.carried.len() <= MATCH_CARRY_BYTES);
    }

    #[test]
    fn carrying_never_splits_a_character() {
        // A carry boundary landing mid-character would panic on the slice, or
        // corrupt the text a pattern is matched against.
        let mut m = matcher("nothing-matches-this");
        m.consider(&"é".repeat(MATCH_CARRY_BYTES), true);
        assert!(m.carried.chars().all(|c| c == 'é'));
    }

    #[test]
    fn hook_reports_matching_lines_within_the_debounce_window() {
        let mut m = hook_matcher(Some("ALERT"), 0);
        assert_eq!(m.consider("nothing here", false), None);
        assert_eq!(
            m.consider("ALERT disk full", false).as_deref(),
            Some("ALERT disk full")
        );
        // Unlike readiness, a hook keeps firing.
        assert!(m.consider("ALERT again", false).is_some());
    }

    #[test]
    fn hook_debounce_suppresses_a_second_line_in_the_same_window() {
        let mut m = hook_matcher(None, 60_000);
        assert!(m.consider("first", false).is_some());
        // Every line matches a hook with no filter, so only the window stops it.
        assert_eq!(m.consider("second", false), None);
    }

    #[test]
    fn strips_ansi_before_matching() {
        let mut m = matcher("^READY$");
        assert!(m.consider("\x1b[32mREADY\x1b[0m", false).is_some());
    }
}
