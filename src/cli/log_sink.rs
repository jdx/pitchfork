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
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
pub struct LogSink {
    /// Qualified id of the daemon whose output this is
    #[usage(long)]
    daemon_id: String,

    /// Log format to parse lines with (`json`, `logfmt`, `auto`, or `text`)
    #[usage(long, default = "text")]
    log_format: String,

    /// Regex whose first match means the daemon is ready
    ///
    /// Set for a daemon configured with `ready_output`. The supervisor cannot
    /// match it itself — this process holds the output — so the match is
    /// reported back over IPC.
    #[usage(long)]
    ready_pattern: Option<String>,

    /// Token identifying the start attempt this sink belongs to
    ///
    /// Quoted back when reporting a match. The supervisor drops reports whose
    /// token is no longer current, so a sink still draining a failed attempt
    /// cannot mark that daemon's retry ready.
    #[usage(long, default_value_t = 0, default = "0")]
    relay_token: u64,

    /// Report lines so the supervisor can fire the daemon's `on_output` hook
    ///
    /// Without `--output-filter` or `--output-regex` every line qualifies,
    /// which is what a hook with no pattern asks for.
    #[usage(long)]
    report_output: bool,

    /// Only report lines containing this substring
    #[usage(long)]
    output_filter: Option<String>,

    /// Only report lines matching this regex
    #[usage(long)]
    output_regex: Option<String>,

    /// Shortest gap between reported lines, in milliseconds
    #[usage(long, default_value_t = 1000, default = "1000")]
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
    Report(Report),
}

/// A line the supervisor needs to see, and why.
#[derive(Debug, PartialEq, Eq)]
struct Report {
    text: String,
    /// Whether it passed the `on_output` hook's filter and debounce. False for
    /// a line reported only because it matched the readiness pattern — firing a
    /// hook that filters for something else would be wrong.
    fires_hook: bool,
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
    fn consider(&mut self, text: &str, split_at_cap: bool) -> Option<Report> {
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

        // A line can qualify for either reason, or both. The supervisor
        // re-checks the readiness pattern on what it is sent, but it cannot
        // re-check the hook without redoing the debounce, so that answer is
        // carried explicitly.
        let mut ready_matched = false;
        if self
            .pattern
            .as_ref()
            .is_some_and(|re| re.is_match(&candidate))
        {
            self.pattern = None;
            ready_matched = true;
        }
        let fires_hook = self
            .hook
            .as_mut()
            .is_some_and(|hook| hook.matches(&candidate));

        // Send the text the patterns actually matched against, not just this
        // piece of it: the supervisor re-matches it, and hands it to the hook.
        (ready_matched || fires_hook).then_some(Report {
            text: candidate,
            fires_hook,
        })
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
    let text = decode_line(line);
    let text = text.trim_end_matches('\r');
    let parsed = crate::log_parse::parse(text, log_format);
    let report = matcher.consider(text, true);
    line.clear();

    tx.send(SinkEvent::Line(parsed))
        .await
        .map_err(|_| std::io::Error::other("log writer stopped"))?;
    if let Some(report) = report {
        tx.send(SinkEvent::Report(report))
            .await
            .map_err(|_| std::io::Error::other("log writer stopped"))?;
    }
    Ok(())
}

/// Length to cut `bytes` at so no character is left half-written, in whichever
/// encoding [`decode_line`] will read the piece in.
fn split_before_incomplete_char(bytes: &[u8]) -> usize {
    #[cfg(windows)]
    return split_before_incomplete_char_in(bytes, console_code_page());
    #[cfg(not(windows))]
    split_before_incomplete_utf8(bytes)
}

/// [`split_before_incomplete_char`] for a console using `code_page`.
#[cfg(windows)]
fn split_before_incomplete_char_in(bytes: &[u8], code_page: u32) -> usize {
    let split = split_before_incomplete_utf8(bytes);
    if uses_code_page(&bytes[..split]) {
        // Hold back whatever either encoding would leave unfinished. A line
        // read in the code page can still carry UTF-8 — a stray byte decides
        // how it is read, not what it contains — and a byte carried over to
        // the next piece is decoded there, so holding one back loses nothing.
        return split.min(split_before_incomplete_dbcs(bytes, code_page));
    }
    split
}

/// Length to cut `bytes` at so no UTF-8 character is left half-written.
///
/// Decided by inspecting the final bytes rather than by asking `from_utf8` where
/// the string stops being valid: that reports the *first* problem, so a single
/// invalid byte earlier in the line would hide an unfinished character at the
/// end, and the character would be split after all.
fn split_before_incomplete_utf8(bytes: &[u8]) -> usize {
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

/// Text of one line of daemon output.
///
/// UTF-8 is taken as it is. Anything else is not assumed to be damaged UTF-8:
/// on Windows, console programs — `cmd` among them — write in the console's
/// code page, so a Japanese system hands over Shift_JIS, and reading that as
/// UTF-8 would store nothing but replacement characters.
fn decode_line(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return std::borrow::Cow::Borrowed(text);
    }
    #[cfg(windows)]
    if uses_code_page(bytes)
        && let Some(text) = decode_in_code_page(bytes, console_code_page())
    {
        return std::borrow::Cow::Owned(text);
    }
    String::from_utf8_lossy(bytes)
}

/// Whether [`decode_line`] reads `bytes` in the console code page.
///
/// Only when they are not UTF-8, and not mostly UTF-8 either: a UTF-8 line with
/// a stray invalid byte — or with another stream's output spliced into it —
/// would have all of its text garbled by decoding it in the code page, where
/// the lossy conversion loses just the stray bytes.
///
/// Mostly means more characters read as multi-byte UTF-8 than runs of bytes
/// that cannot. Code page text does sometimes form a valid UTF-8 character by
/// chance, but around it are far more bytes that do not.
#[cfg(windows)]
fn uses_code_page(bytes: &[u8]) -> bool {
    if std::str::from_utf8(bytes).is_ok() {
        return false;
    }
    let (mut multi_byte, mut invalid) = (0usize, 0usize);
    for chunk in bytes.utf8_chunks() {
        multi_byte += chunk.valid().chars().filter(|c| !c.is_ascii()).count();
        invalid += usize::from(!chunk.invalid().is_empty());
    }
    multi_byte <= invalid
}

/// Code page the daemon's console programs write in.
///
/// This process is started the same way as the daemon, so it shares the
/// daemon's console or gets one set up alike. Without a console at all, the
/// OEM code page is what a new console would have used.
#[cfg(windows)]
fn console_code_page() -> u32 {
    match unsafe { windows_sys::Win32::System::Console::GetConsoleOutputCP() } {
        0 => unsafe { windows_sys::Win32::Globalization::GetOEMCP() },
        code_page => code_page,
    }
}

/// Decode `bytes` from `code_page`, or `None` when Windows cannot.
///
/// A UTF-8 code page gets `None` too: the bytes are already known not to be
/// valid UTF-8, and the lossy conversion handles them just as well.
#[cfg(windows)]
fn decode_in_code_page(bytes: &[u8], code_page: u32) -> Option<String> {
    use windows_sys::Win32::Globalization::{CP_UTF8, MultiByteToWideChar};

    if code_page == CP_UTF8 {
        return None;
    }
    let len = i32::try_from(bytes.len()).ok()?;
    // The first call measures, the second converts.
    let wide_len =
        unsafe { MultiByteToWideChar(code_page, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0) };
    if wide_len <= 0 {
        return None;
    }
    let mut wide = vec![0u16; wide_len as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            len,
            wide.as_mut_ptr(),
            wide_len,
        )
    };
    if written <= 0 {
        return None;
    }
    wide.truncate(written as usize);
    Some(String::from_utf16_lossy(&wide))
}

/// Length to cut `bytes` at so no double-byte character of `code_page` is left
/// half-written.
///
/// A trail byte can have the same value as a lead byte, so looking at the last
/// byte alone cannot tell which it is; only walking from the start of the line
/// can. Single-byte code pages have no lead bytes, so they never cut short.
#[cfg(windows)]
fn split_before_incomplete_dbcs(bytes: &[u8], code_page: u32) -> usize {
    use windows_sys::Win32::Globalization::IsDBCSLeadByteEx;

    let mut i = 0;
    while i < bytes.len() {
        let lead = unsafe { IsDBCSLeadByteEx(code_page, bytes[i]) } != 0;
        i += if lead { 2 } else { 1 };
    }
    // Stepping past the end means the last byte opened a character.
    if i > bytes.len() && bytes.len() > 1 {
        bytes.len() - 1
    } else {
        bytes.len()
    }
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
    // Never fails: a daemon emitting a stray non-UTF-8 byte must not be able to
    // stop its own logging.
    let text = decode_line(line);
    let text = text.trim_end_matches('\r');
    let parsed = crate::log_parse::parse(text, log_format);
    // Strip ANSI before matching so a pattern works whether or not the daemon
    // colours its output, matching what in-process capture did.
    let report = matcher.consider(text, false);
    line.clear();

    tx.send(SinkEvent::Line(parsed))
        .await
        .map_err(|_| std::io::Error::other("log writer stopped"))?;
    if let Some(report) = report {
        tx.send(SinkEvent::Report(report))
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
                SinkEvent::Report(report) => {
                    // Everything up to and including the reported line goes to
                    // the store before the supervisor is told, so whatever it
                    // does next can already read it.
                    flush(&id, &mut batch).await;
                    report_line(&id, relay_token, report).await;
                }
            }
        }
        flush(&id, &mut batch).await;
        if closed {
            break;
        }
    }
}

/// Hand the supervisor a line it needs to act on.
///
/// Failure is logged and dropped. There is no supervisor to retry against if it
/// has crashed — and if it has, nothing is waiting on this daemon's readiness or
/// hooks — while the daemon's output keeps being captured either way.
async fn report_line(id: &DaemonId, relay_token: u64, report: Report) {
    // `autostart: false` — a sink must never bring a supervisor into being.
    match crate::ipc::client::IpcClient::connect(false).await {
        Ok(client) => {
            if let Err(e) = client
                .sink_output_line(id.clone(), relay_token, report.fires_hook, report.text)
                .await
            {
                warn!("log sink for {id} could not report a line of output: {e}");
            }
        }
        Err(e) => {
            warn!("log sink for {id} could not reach the supervisor to report output: {e}");
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
            m.consider("READY to serve", false)
                .map(|r| r.text)
                .as_deref(),
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
        let report = m
            .consider("READY....", false)
            .expect("should match across the split");
        assert!(
            report.text.contains("SERVER READY"),
            "reported {:?}",
            report.text
        );
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
        let report = m.consider("ALERT disk full", false).expect("should report");
        assert_eq!(report.text, "ALERT disk full");
        assert!(report.fires_hook);
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
    fn a_readiness_only_line_does_not_fire_the_hook() {
        // A daemon can have both, filtered differently. The line that announces
        // readiness is nothing to do with the hook, and reporting it must not
        // be taken as permission to run the hook's command.
        let mut m = ReadyMatcher::new(
            Some(regex::Regex::new("READY").unwrap()),
            Some(HookMatcher {
                filter: Some("ALERT".to_string()),
                regex: None,
                debounce: std::time::Duration::from_millis(0),
                last_reported: None,
            }),
        );
        let report = m.consider("READY to serve", false).expect("should report");
        assert!(!report.fires_hook, "readiness line must not fire the hook");
        // ...and a line matching both says so.
        let mut m = ReadyMatcher::new(
            Some(regex::Regex::new("READY").unwrap()),
            Some(HookMatcher {
                filter: Some("READY".to_string()),
                regex: None,
                debounce: std::time::Duration::from_millis(0),
                last_reported: None,
            }),
        );
        assert!(
            m.consider("READY", false)
                .expect("should report")
                .fires_hook
        );
    }

    #[test]
    fn strips_ansi_before_matching() {
        let mut m = matcher("^READY$");
        assert!(m.consider("\x1b[32mREADY\x1b[0m", false).is_some());
    }

    #[test]
    fn decodes_utf8_unchanged() {
        assert_eq!(
            super::decode_line("起動しました".as_bytes()),
            "起動しました"
        );
    }

    /// What `cmd /C exec` writes on a Japanese system: "'exec' は、内部コマンド".
    const CP932_CMD_ERROR: &[u8] =
        b"'exec' \x82\xcd\x81\x41\x93\xe0\x95\x94\x83\x52\x83\x7d\x83\x93\x83\x68";

    #[cfg(windows)]
    #[test]
    fn decodes_the_console_code_page() {
        assert_eq!(
            super::decode_in_code_page(CP932_CMD_ERROR, 932).as_deref(),
            Some("'exec' は、内部コマンド")
        );
    }

    #[cfg(windows)]
    #[test]
    fn leaves_a_utf8_code_page_to_the_lossy_conversion() {
        assert_eq!(super::decode_in_code_page(b"\xff", 65001), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn replaces_non_utf8_outside_windows() {
        assert_eq!(super::decode_line(b"ok \xff"), "ok \u{fffd}");
    }

    /// The second line of the same message: "操作可能なプログラムまたはバッチ
    /// ファイルとして認識されていません。"
    const CP932_CMD_ERROR_2: &[u8] = b"\x91\x80\x8d\xec\x89\xc2\x94\x5c\x82\xc8\x83\x76\x83\x8d\x83\x4f\x83\x89\x83\x80\x82\xdc\x82\xbd\x82\xcd\x83\x6f\x83\x62\x83\x60\x20\x83\x74\x83\x40\x83\x43\x83\x8b\x82\xc6\x82\xb5\x82\xc4\x94\x46\x8e\xaf\x82\xb3\x82\xea\x82\xc4\x82\xa2\x82\xdc\x82\xb9\x82\xf1\x81\x42";

    #[test]
    fn keeps_utf8_text_around_a_stray_byte() {
        // Decoding this in a code page would garble every character; only the
        // stray byte should be lost.
        let mut line = "起動しました ".as_bytes().to_vec();
        line.push(0xff);
        assert_eq!(super::decode_line(&line), "起動しました \u{fffd}");
    }

    #[cfg(windows)]
    #[test]
    fn reads_code_page_text_in_the_code_page() {
        // The first line happens to contain a valid UTF-8 character (0xCD 0x81).
        assert!(super::uses_code_page(CP932_CMD_ERROR));
        assert!(super::uses_code_page(CP932_CMD_ERROR_2));
        assert_eq!(
            super::decode_in_code_page(CP932_CMD_ERROR_2, 932).as_deref(),
            Some("操作可能なプログラムまたはバッチ ファイルとして認識されていません。")
        );
    }

    #[cfg(windows)]
    #[test]
    fn splits_before_an_unfinished_double_byte_character() {
        // "aaaあ" in CP932, cut by the length cap between あ's two bytes.
        let line = b"aaa\x82\xa0";
        let split = super::split_before_incomplete_dbcs(&line[..4], 932);
        assert_eq!(split, 3);
        let first = super::decode_in_code_page(&line[..split], 932).unwrap();
        let rest = super::decode_in_code_page(&line[split..], 932).unwrap();
        assert_eq!(first + &rest, "aaaあ");
    }

    #[cfg(windows)]
    #[test]
    fn a_line_read_in_the_code_page_keeps_an_unfinished_utf8_character() {
        // A stray byte sends the piece to the code page, but the line ends in
        // the first byte of "é" (0xC3 0xA9), which is a whole character in
        // CP932. Cutting there would still split the UTF-8 character.
        let line = b"ab\xffcd\xc3";
        assert!(super::uses_code_page(&line[..5]));
        assert_eq!(super::split_before_incomplete_char_in(line, 932), 5);
        // A double-byte character left unfinished is still held back too.
        assert_eq!(
            super::split_before_incomplete_char_in(b"ab\xffcd\x82", 932),
            5
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_trail_byte_that_looks_like_a_lead_byte_is_not_cut() {
        // 0x82 is a lead byte, but here the second one completes the first.
        assert_eq!(super::split_before_incomplete_dbcs(b"a\x82\x82", 932), 3);
        // ...and here the first pair is whole, leaving the last 0x82 unfinished.
        assert_eq!(super::split_before_incomplete_dbcs(b"\x82\x9f\x82", 932), 2);
        // A single-byte code page has nothing to finish.
        assert_eq!(super::split_before_incomplete_dbcs(b"aa\x82", 1252), 3);
    }

    #[test]
    fn never_fails_on_non_utf8() {
        // Whatever the code page, the line is still stored.
        assert!(super::decode_line(CP932_CMD_ERROR).starts_with("'exec' "));
    }
}
