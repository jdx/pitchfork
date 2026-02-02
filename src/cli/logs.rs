use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::PitchforkToml;
use crate::ui::style::edim;
use crate::watch_files::WatchFiles;
use crate::{Result, env};
use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, TimeZone, Timelike};
use itertools::Itertools;
use miette::IntoDiagnostic;
use notify::RecursiveMode;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use xx::regex;

/// Pager configuration for displaying logs
struct PagerConfig {
    command: String,
    args: Vec<String>,
}

impl PagerConfig {
    /// Select and configure the appropriate pager.
    /// Uses $PAGER environment variable if set, otherwise defaults to less.
    fn new(start_at_end: bool) -> Self {
        let command = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
        let args = Self::build_args(&command, start_at_end);
        Self { command, args }
    }

    fn build_args(pager: &str, start_at_end: bool) -> Vec<String> {
        let mut args = vec![];
        if pager == "less" {
            args.push("-R".to_string());
            if start_at_end {
                args.push("+G".to_string());
            }
        }
        args
    }

    /// Spawn the pager with piped stdin
    fn spawn_piped(&self) -> io::Result<Child> {
        Command::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::piped())
            .spawn()
    }
}

/// Format a single log line for output.
/// When `single_daemon` is true, omits the daemon ID from the output.
fn format_log_line(date: &str, id: &str, msg: &str, single_daemon: bool) -> String {
    if single_daemon {
        format!("{} {}", edim(date), msg)
    } else {
        format!("{} {} {}", edim(date), id, msg)
    }
}

/// A parsed log entry with timestamp, daemon name, and message
#[derive(Debug)]
struct LogEntry {
    timestamp: String,
    daemon: String,
    message: String,
    source_idx: usize, // Index of the source iterator
}

impl PartialEq for LogEntry {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp
    }
}

impl Eq for LogEntry {}

impl PartialOrd for LogEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LogEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.timestamp.cmp(&other.timestamp)
    }
}

/// Streaming merger for multiple sorted log files using a min-heap.
/// This allows merging sorted iterators without loading all data into memory.
struct StreamingMerger<I>
where
    I: Iterator<Item = (String, String)>,
{
    sources: Vec<(String, I)>,           // (daemon_name, line_iterator)
    heap: BinaryHeap<Reverse<LogEntry>>, // Min-heap (using Reverse for ascending order)
}

impl<I> StreamingMerger<I>
where
    I: Iterator<Item = (String, String)>,
{
    fn new() -> Self {
        Self {
            sources: Vec::new(),
            heap: BinaryHeap::new(),
        }
    }

    fn add_source(&mut self, daemon_name: String, iter: I) {
        self.sources.push((daemon_name, iter));
    }

    fn initialize(&mut self) {
        // Pull the first entry from each source into the heap
        for (idx, (daemon, iter)) in self.sources.iter_mut().enumerate() {
            if let Some((timestamp, message)) = iter.next() {
                self.heap.push(Reverse(LogEntry {
                    timestamp,
                    daemon: daemon.clone(),
                    message,
                    source_idx: idx,
                }));
            }
        }
    }
}

impl<I> Iterator for StreamingMerger<I>
where
    I: Iterator<Item = (String, String)>,
{
    type Item = (String, String, String); // (timestamp, daemon, message)

    fn next(&mut self) -> Option<Self::Item> {
        // Pop the smallest entry from the heap
        let Reverse(entry) = self.heap.pop()?;

        // Pull the next entry from the same source and push to heap
        let (daemon, iter) = &mut self.sources[entry.source_idx];
        if let Some((timestamp, message)) = iter.next() {
            self.heap.push(Reverse(LogEntry {
                timestamp,
                daemon: daemon.clone(),
                message,
                source_idx: entry.source_idx,
            }));
        }

        Some((entry.timestamp, entry.daemon, entry.message))
    }
}

/// A proper streaming log parser that handles multi-line entries
struct StreamingLogParser {
    reader: BufReader<File>,
    current_entry: Option<(String, String)>,
    finished: bool,
}

impl StreamingLogParser {
    fn new(file: File) -> Self {
        Self {
            reader: BufReader::new(file),
            current_entry: None,
            finished: false,
        }
    }
}

impl Iterator for StreamingLogParser {
    type Item = (String, String);

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        let re = regex!(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) ([\w./-]+) (.*)$");

        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => {
                    // EOF - return the last entry if any
                    self.finished = true;
                    return self.current_entry.take();
                }
                Ok(_) => {
                    // Remove trailing newline
                    if line.ends_with('\n') {
                        line.pop();
                        if line.ends_with('\r') {
                            line.pop();
                        }
                    }

                    if let Some(caps) = re.captures(&line) {
                        let date = match caps.get(1) {
                            Some(d) => d.as_str().to_string(),
                            None => continue,
                        };
                        let msg = match caps.get(3) {
                            Some(m) => m.as_str().to_string(),
                            None => continue,
                        };

                        // Return the previous entry and start a new one
                        let prev = self.current_entry.take();
                        self.current_entry = Some((date, msg));

                        if prev.is_some() {
                            return prev;
                        }
                        // First entry - continue to read more
                    } else {
                        // Continuation line - append to current entry
                        if let Some((_, ref mut msg)) = self.current_entry {
                            msg.push('\n');
                            msg.push_str(&line);
                        }
                    }
                }
                Err(_) => {
                    self.finished = true;
                    return self.current_entry.take();
                }
            }
        }
    }
}

/// Displays logs for daemon(s)
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "l",
    verbatim_doc_comment,
    long_about = "\
Displays logs for daemon(s)

Shows logs from managed daemons. Logs are stored in the pitchfork logs directory
and include timestamps for filtering.

Examples:
  pitchfork logs api              Show all logs for 'api' (paged if needed)
  pitchfork logs api worker       Show logs for multiple daemons
  pitchfork logs                  Show logs for all daemons
  pitchfork logs api -n 50        Show last 50 lines
  pitchfork logs api --follow     Follow logs in real-time
  pitchfork logs api --since '2024-01-15 10:00:00'
                                  Show logs since a specific time (forward)
  pitchfork logs api --since '10:30:00'
                                  Show logs since 10:30:00 today
  pitchfork logs api --since '10:30' --until '12:00'
                                  Show logs since 10:30:00 until 12:00:00 today
  pitchfork logs api --since 5min Show logs from last 5 minutes
  pitchfork logs api --raw        Output raw log lines without formatting
  pitchfork logs api --raw -n 100 Output last 100 raw log lines
  pitchfork logs api --clear      Delete logs for 'api'
  pitchfork logs --clear          Delete logs for all daemons"
)]
pub struct Logs {
    /// Show only logs for the specified daemon(s)
    id: Vec<String>,

    /// Delete logs
    #[clap(short, long)]
    clear: bool,

    /// Show last N lines of logs
    ///
    /// Only applies when --since/--until is not used.
    /// Without this option, all logs are shown.
    #[clap(short)]
    n: Option<usize>,

    /// Show logs in real-time
    #[clap(short = 't', short_alias = 'f', long, visible_alias = "follow")]
    tail: bool,

    /// Show logs from this time
    ///
    /// Supports multiple formats:
    /// - Full datetime: "YYYY-MM-DD HH:MM:SS" or "YYYY-MM-DD HH:MM"
    /// - Time only: "HH:MM:SS" or "HH:MM" (uses today's date)
    /// - Relative time: "5min", "2h", "1d" (e.g., last 5 minutes)
    #[clap(short = 's', long)]
    since: Option<String>,

    /// Show logs until this time
    ///
    /// Supports multiple formats:
    /// - Full datetime: "YYYY-MM-DD HH:MM:SS" or "YYYY-MM-DD HH:MM"
    /// - Time only: "HH:MM:SS" or "HH:MM" (uses today's date)
    #[clap(short = 'u', long)]
    until: Option<String>,

    /// Disable pager even in interactive terminal
    #[clap(long)]
    no_pager: bool,

    /// Output raw log lines without color or formatting
    #[clap(long)]
    raw: bool,
}

impl Logs {
    pub async fn run(&self) -> Result<()> {
        // Resolve user-provided IDs to qualified IDs
        let resolved_ids: Vec<DaemonId> = if self.id.is_empty() {
            // When no IDs provided, use all daemon IDs
            get_all_daemon_ids()?
        } else {
            PitchforkToml::resolve_ids(&self.id)?
        };

        if self.clear {
            for id in &resolved_ids {
                let path = id.log_path();
                if path.exists() {
                    xx::file::create(&path)?;
                }
            }
            return Ok(());
        }

        let from = if let Some(since) = self.since.as_ref() {
            Some(parse_time_input(since, true)?)
        } else {
            None
        };
        let to = if let Some(until) = self.until.as_ref() {
            Some(parse_time_input(until, false)?)
        } else {
            None
        };

        self.print_existing_logs(&resolved_ids, from, to)?;
        if self.tail {
            tail_logs(&resolved_ids).await?;
        }

        Ok(())
    }

    fn print_existing_logs(
        &self,
        resolved_ids: &[DaemonId],
        from: Option<DateTime<Local>>,
        to: Option<DateTime<Local>>,
    ) -> Result<()> {
        let log_files = get_log_file_infos(resolved_ids)?;
        trace!("log files for: {}", log_files.keys().join(", "));
        let single_daemon = resolved_ids.len() == 1;
        let has_time_filter = from.is_some() || to.is_some();

        if has_time_filter {
            let mut log_lines = self.collect_log_lines_forward(&log_files, from, to)?;

            if let Some(n) = self.n {
                let len = log_lines.len();
                if len > n {
                    log_lines = log_lines.into_iter().skip(len - n).collect_vec();
                }
            }

            self.output_logs(log_lines, single_daemon, has_time_filter, self.raw)?;
        } else if let Some(n) = self.n {
            let log_lines = self.collect_log_lines_reverse(&log_files, Some(n))?;
            self.output_logs(log_lines, single_daemon, has_time_filter, self.raw)?;
        } else {
            self.stream_logs_to_pager(&log_files, single_daemon, self.raw)?;
        }

        Ok(())
    }

    fn collect_log_lines_forward(
        &self,
        log_files: &BTreeMap<DaemonId, LogFile>,
        from: Option<DateTime<Local>>,
        to: Option<DateTime<Local>>,
    ) -> Result<Vec<(String, String, String)>> {
        let log_lines: Vec<(String, String, String)> = log_files
            .iter()
            .flat_map(
                |(name, lf)| match read_lines_in_time_range(&lf.path, from, to) {
                    Ok(lines) => merge_log_lines(&name.qualified(), lines, false),
                    Err(e) => {
                        error!("{}: {}", lf.path.display(), e);
                        vec![]
                    }
                },
            )
            .sorted_by_cached_key(|l| l.0.to_string())
            .collect_vec();

        Ok(log_lines)
    }

    fn collect_log_lines_reverse(
        &self,
        log_files: &BTreeMap<DaemonId, LogFile>,
        limit: Option<usize>,
    ) -> Result<Vec<(String, String, String)>> {
        let log_lines: Vec<(String, String, String)> = log_files
            .iter()
            .flat_map(|(daemon_id, lf)| {
                let rev = match xx::file::open(&lf.path) {
                    Ok(f) => rev_lines::RevLines::new(f),
                    Err(e) => {
                        error!("{}: {}", lf.path.display(), e);
                        return vec![];
                    }
                };
                let lines = rev.into_iter().filter_map(Result::ok);
                let lines = match limit {
                    Some(n) => lines.take(n).collect_vec(),
                    None => lines.collect_vec(),
                };
                merge_log_lines(&daemon_id.qualified(), lines, true)
            })
            .sorted_by_cached_key(|l| l.0.to_string())
            .collect_vec();

        let log_lines = match limit {
            Some(n) => {
                let len = log_lines.len();
                if len > n {
                    log_lines.into_iter().skip(len - n).collect_vec()
                } else {
                    log_lines
                }
            }
            None => log_lines,
        };

        Ok(log_lines)
    }

    fn output_logs(
        &self,
        log_lines: Vec<(String, String, String)>,
        single_daemon: bool,
        has_time_filter: bool,
        raw: bool,
    ) -> Result<()> {
        if log_lines.is_empty() {
            return Ok(());
        }

        // Raw mode: output without formatting and without pager
        if raw {
            for (date, _, msg) in log_lines {
                println!("{} {}", date, msg);
            }
            return Ok(());
        }

        let use_pager = !self.no_pager && should_use_pager(log_lines.len());

        if use_pager {
            self.output_with_pager(log_lines, single_daemon, has_time_filter)?;
        } else {
            for (date, id, msg) in log_lines {
                println!("{}", format_log_line(&date, &id, &msg, single_daemon));
            }
        }

        Ok(())
    }

    fn output_with_pager(
        &self,
        log_lines: Vec<(String, String, String)>,
        single_daemon: bool,
        has_time_filter: bool,
    ) -> Result<()> {
        // When time filter is used, start at top; otherwise start at end
        let pager_config = PagerConfig::new(!has_time_filter);

        match pager_config.spawn_piped() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    for (date, id, msg) in log_lines {
                        let line =
                            format!("{}\n", format_log_line(&date, &id, &msg, single_daemon));
                        if stdin.write_all(line.as_bytes()).is_err() {
                            break;
                        }
                    }
                    let _ = child.wait();
                } else {
                    debug!("Failed to get pager stdin, falling back to direct output");
                    for (date, id, msg) in log_lines {
                        println!("{}", format_log_line(&date, &id, &msg, single_daemon));
                    }
                }
            }
            Err(e) => {
                debug!(
                    "Failed to spawn pager: {}, falling back to direct output",
                    e
                );
                for (date, id, msg) in log_lines {
                    println!("{}", format_log_line(&date, &id, &msg, single_daemon));
                }
            }
        }

        Ok(())
    }

    fn stream_logs_to_pager(
        &self,
        log_files: &BTreeMap<DaemonId, LogFile>,
        single_daemon: bool,
        raw: bool,
    ) -> Result<()> {
        if !io::stdout().is_terminal() || self.no_pager || raw {
            return self.stream_logs_direct(log_files, single_daemon, raw);
        }

        let pager_config = PagerConfig::new(true); // start_at_end = true

        match pager_config.spawn_piped() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.take() {
                    // Collect file info for the streaming thread
                    let log_files_clone: Vec<_> = log_files
                        .iter()
                        .map(|(daemon_id, lf)| (daemon_id.qualified(), lf.path.clone()))
                        .collect();
                    let single_daemon_clone = single_daemon;

                    // Stream logs using a background thread to avoid blocking
                    std::thread::spawn(move || {
                        let mut writer = BufWriter::new(stdin);

                        // Single file: stream directly without merge overhead
                        if log_files_clone.len() == 1 {
                            let (name, path) = &log_files_clone[0];
                            let file = match File::open(path) {
                                Ok(f) => f,
                                Err(_) => return,
                            };
                            let parser = StreamingLogParser::new(file);
                            for (timestamp, message) in parser {
                                let output = format!(
                                    "{}\n",
                                    format_log_line(
                                        &timestamp,
                                        name,
                                        &message,
                                        single_daemon_clone
                                    )
                                );
                                if writer.write_all(output.as_bytes()).is_err() {
                                    return;
                                }
                            }
                            let _ = writer.flush();
                            return;
                        }

                        // Multiple files: use streaming merger for sorted/interleaved output
                        let mut merger: StreamingMerger<StreamingLogParser> =
                            StreamingMerger::new();

                        for (name, path) in log_files_clone {
                            let file = match File::open(&path) {
                                Ok(f) => f,
                                Err(_) => continue,
                            };
                            let parser = StreamingLogParser::new(file);
                            merger.add_source(name, parser);
                        }

                        // Initialize the heap with first entry from each source
                        merger.initialize();

                        // Stream merged entries to pager
                        for (timestamp, daemon, message) in merger {
                            let output = format!(
                                "{}\n",
                                format_log_line(&timestamp, &daemon, &message, single_daemon_clone)
                            );
                            if writer.write_all(output.as_bytes()).is_err() {
                                return;
                            }
                        }

                        let _ = writer.flush();
                    });

                    let _ = child.wait();
                } else {
                    debug!("Failed to get pager stdin, falling back to direct output");
                    return self.stream_logs_direct(log_files, single_daemon, raw);
                }
            }
            Err(e) => {
                debug!(
                    "Failed to spawn pager: {}, falling back to direct output",
                    e
                );
                return self.stream_logs_direct(log_files, single_daemon, raw);
            }
        }

        Ok(())
    }

    fn stream_logs_direct(
        &self,
        log_files: &BTreeMap<DaemonId, LogFile>,
        single_daemon: bool,
        raw: bool,
    ) -> Result<()> {
        // Fast path for single daemon: directly output file content without parsing
        // This avoids expensive regex parsing for each line in large log files
        if log_files.len() == 1 {
            let (daemon_id, lf) = log_files.iter().next().unwrap();
            let file = match File::open(&lf.path) {
                Ok(f) => f,
                Err(e) => {
                    error!("{}: {}", lf.path.display(), e);
                    return Ok(());
                }
            };
            let reader = BufReader::new(file);
            if raw {
                // Raw mode: output lines as-is
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            if io::stdout().write_all(l.as_bytes()).is_err()
                                || io::stdout().write_all(b"\n").is_err()
                            {
                                return Ok(());
                            }
                        }
                        Err(_) => continue,
                    }
                }
            } else {
                // Formatted mode: parse and format each line
                let parser = StreamingLogParser::new(File::open(&lf.path).into_diagnostic()?);
                for (timestamp, message) in parser {
                    let output = format!(
                        "{}\n",
                        format_log_line(
                            &timestamp,
                            &daemon_id.qualified(),
                            &message,
                            single_daemon
                        )
                    );
                    if io::stdout().write_all(output.as_bytes()).is_err() {
                        return Ok(());
                    }
                }
            }
            return Ok(());
        }

        // Multiple daemons: use streaming merger for sorted output
        let mut merger: StreamingMerger<StreamingLogParser> = StreamingMerger::new();

        for (daemon_id, lf) in log_files {
            let file = match File::open(&lf.path) {
                Ok(f) => f,
                Err(e) => {
                    error!("{}: {}", lf.path.display(), e);
                    continue;
                }
            };
            let parser = StreamingLogParser::new(file);
            merger.add_source(daemon_id.qualified(), parser);
        }

        // Initialize the heap with first entry from each source
        merger.initialize();

        // Stream merged entries to stdout
        for (timestamp, daemon, message) in merger {
            let output = if raw {
                format!("{} {}\n", timestamp, message)
            } else {
                format!(
                    "{}\n",
                    format_log_line(&timestamp, &daemon, &message, single_daemon)
                )
            };
            if io::stdout().write_all(output.as_bytes()).is_err() {
                return Ok(());
            }
        }

        Ok(())
    }
}

fn should_use_pager(line_count: usize) -> bool {
    if !io::stdout().is_terminal() {
        return false;
    }

    let terminal_height = get_terminal_height().unwrap_or(24);
    line_count > terminal_height
}

fn get_terminal_height() -> Option<usize> {
    if let Ok(rows) = std::env::var("LINES")
        && let Ok(h) = rows.parse::<usize>()
    {
        return Some(h);
    }

    crossterm::terminal::size().ok().map(|(_, h)| h as usize)
}

fn read_lines_in_time_range(
    path: &Path,
    from: Option<DateTime<Local>>,
    to: Option<DateTime<Local>>,
) -> Result<Vec<String>> {
    let mut file = File::open(path).into_diagnostic()?;
    let file_size = file.metadata().into_diagnostic()?.len();

    if file_size == 0 {
        return Ok(vec![]);
    }

    let start_pos = if let Some(from_time) = from {
        binary_search_log_position(&mut file, file_size, from_time, true)?
    } else {
        0
    };

    let end_pos = if let Some(to_time) = to {
        binary_search_log_position(&mut file, file_size, to_time, false)?
    } else {
        file_size
    };

    if start_pos >= end_pos {
        return Ok(vec![]);
    }

    file.seek(SeekFrom::Start(start_pos)).into_diagnostic()?;
    let mut reader = BufReader::new(&file);
    let mut lines = Vec::new();
    let mut current_pos = start_pos;

    loop {
        if current_pos >= end_pos {
            break;
        }

        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(bytes_read) => {
                current_pos += bytes_read as u64;
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                lines.push(line);
            }
            Err(_) => break,
        }
    }

    Ok(lines)
}

fn binary_search_log_position(
    file: &mut File,
    file_size: u64,
    target_time: DateTime<Local>,
    find_start: bool,
) -> Result<u64> {
    let mut low: u64 = 0;
    let mut high: u64 = file_size;

    while low < high {
        let mid = low + (high - low) / 2;

        let line_start = find_line_start(file, mid)?;

        file.seek(SeekFrom::Start(line_start)).into_diagnostic()?;
        let mut reader = BufReader::new(&*file);
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line).into_diagnostic()?;
        if bytes_read == 0 {
            high = mid;
            continue;
        }

        let line_time = extract_timestamp(&line);

        match line_time {
            Some(lt) => {
                if find_start {
                    if lt < target_time {
                        low = line_start + bytes_read as u64;
                    } else {
                        high = line_start;
                    }
                } else if lt <= target_time {
                    low = line_start + bytes_read as u64;
                } else {
                    high = line_start;
                }
            }
            None => {
                low = line_start + bytes_read as u64;
            }
        }
    }

    find_line_start(file, low)
}

fn find_line_start(file: &mut File, pos: u64) -> Result<u64> {
    if pos == 0 {
        return Ok(0);
    }

    // Start searching from the byte just before `pos`.
    let mut search_pos = pos.saturating_sub(1);
    const CHUNK_SIZE: usize = 8192;

    loop {
        // Determine the start of the chunk we want to read.
        let chunk_start = search_pos.saturating_sub(CHUNK_SIZE as u64 - 1);
        let len_u64 = search_pos - chunk_start + 1;
        let len = len_u64 as usize;

        // Seek once to the beginning of this chunk.
        file.seek(SeekFrom::Start(chunk_start)).into_diagnostic()?;
        let mut buf = vec![0u8; len];
        if file.read_exact(&mut buf).is_err() {
            // Match the original behavior: on read error, fall back to start of file.
            return Ok(0);
        }

        // Scan this chunk backwards for a newline.
        for (i, &b) in buf.iter().enumerate().rev() {
            if b == b'\n' {
                return Ok(chunk_start + i as u64 + 1);
            }
        }

        // No newline in this chunk; if we've reached the start of the file,
        // there is no earlier newline.
        if chunk_start == 0 {
            return Ok(0);
        }

        // Move to the previous chunk (just before this one).
        search_pos = chunk_start - 1;
    }
}

fn extract_timestamp(line: &str) -> Option<DateTime<Local>> {
    let re = regex!(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})");
    re.captures(line)
        .and_then(|caps| caps.get(1))
        .and_then(|m| parse_datetime(m.as_str()).ok())
}

fn merge_log_lines(id: &str, lines: Vec<String>, reverse: bool) -> Vec<(String, String, String)> {
    let lines = if reverse {
        lines.into_iter().rev().collect()
    } else {
        lines
    };

    let re = regex!(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) ([\w./-]+) (.*)$");
    lines
        .into_iter()
        .fold(vec![], |mut acc, line| match re.captures(&line) {
            Some(caps) => {
                let (date, msg) = match (caps.get(1), caps.get(3)) {
                    (Some(d), Some(m)) => (d.as_str().to_string(), m.as_str().to_string()),
                    _ => return acc,
                };
                acc.push((date, id.to_string(), msg));
                acc
            }
            None => {
                if let Some(l) = acc.last_mut() {
                    l.2.push('\n');
                    l.2.push_str(&line);
                }
                acc
            }
        })
}

fn get_all_daemon_ids() -> Result<Vec<DaemonId>> {
    Ok(xx::file::ls(&*env::PITCHFORK_LOGS_DIR)?
        .into_iter()
        .filter(|d| !d.starts_with("."))
        .filter(|d| d.is_dir())
        .filter_map(|d| d.file_name().map(|f| f.to_string_lossy().to_string()))
        // Convert filesystem-safe path back to daemon ID
        .filter_map(|n| DaemonId::from_safe_path(&n).ok())
        .collect())
}

fn get_log_file_infos(names: &[DaemonId]) -> Result<BTreeMap<DaemonId, LogFile>> {
    let names_set: HashSet<&DaemonId> = names.iter().collect();
    xx::file::ls(&*env::PITCHFORK_LOGS_DIR)?
        .into_iter()
        .filter(|d| !d.starts_with("."))
        .filter(|d| d.is_dir())
        .filter_map(|d| d.file_name().map(|f| f.to_string_lossy().to_string()))
        // Convert filesystem-safe path back to daemon ID for filtering
        .filter_map(|path_name| {
            DaemonId::from_safe_path(&path_name)
                .ok()
                .map(|daemon_id| (daemon_id, path_name))
        })
        .filter(|(daemon_id, _)| names_set.is_empty() || names_set.contains(daemon_id))
        .map(|(daemon_id, path_name)| {
            let path = env::PITCHFORK_LOGS_DIR
                .join(&path_name)
                .join(format!("{path_name}.log"))
                .canonicalize()
                .into_diagnostic()?;
            let mut file = xx::file::open(&path)?;
            // Seek to end and get position atomically to avoid race condition
            // where content is written between metadata check and file open
            file.seek(SeekFrom::End(0)).into_diagnostic()?;
            let cur = file.stream_position().into_diagnostic()?;
            Ok((
                daemon_id.clone(),
                LogFile {
                    _name: daemon_id,
                    file,
                    cur,
                    path,
                },
            ))
        })
        .filter_ok(|(_, f)| f.path.exists())
        .collect::<Result<BTreeMap<_, _>>>()
}

pub async fn tail_logs(names: &[DaemonId]) -> Result<()> {
    let mut log_files = get_log_file_infos(names)?;
    let mut wf = WatchFiles::new(Duration::from_millis(10))?;

    for lf in log_files.values() {
        wf.watch(&lf.path, RecursiveMode::NonRecursive)?;
    }

    let files_to_name: HashMap<PathBuf, DaemonId> = log_files
        .iter()
        .map(|(n, f)| (f.path.clone(), n.clone()))
        .collect();

    while let Some(paths) = wf.rx.recv().await {
        let mut out = vec![];
        for path in paths {
            let Some(name) = files_to_name.get(&path) else {
                warn!("Unknown log file changed: {}", path.display());
                continue;
            };
            let Some(info) = log_files.get_mut(name) else {
                warn!("No log info for: {name}");
                continue;
            };
            info.file
                .seek(SeekFrom::Start(info.cur))
                .into_diagnostic()?;
            let reader = BufReader::new(&info.file);
            let lines = reader.lines().map_while(Result::ok).collect_vec();
            info.cur = info.file.stream_position().into_diagnostic()?;
            out.extend(merge_log_lines(&name.qualified(), lines, false));
        }
        let out = out
            .into_iter()
            .sorted_by_cached_key(|l| l.0.to_string())
            .collect_vec();
        for (date, name, msg) in out {
            println!("{} {} {}", edim(&date), name, msg);
        }
    }
    Ok(())
}

struct LogFile {
    _name: DaemonId,
    path: PathBuf,
    file: fs::File,
    cur: u64,
}

fn parse_datetime(s: &str) -> Result<DateTime<Local>> {
    let naive_dt = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").into_diagnostic()?;
    Local
        .from_local_datetime(&naive_dt)
        .single()
        .ok_or_else(|| miette::miette!("Invalid or ambiguous datetime: '{}'. ", s))
}

/// Parse time input string into DateTime.
///
/// `is_since` indicates whether this is for --since (true) or --until (false).
/// The "yesterday fallback" only applies to --since: if the time is in the future,
/// assume the user meant yesterday. For --until, future times are kept as-is.
fn parse_time_input(s: &str, is_since: bool) -> Result<DateTime<Local>> {
    let s = s.trim();

    // Try full datetime first (YYYY-MM-DD HH:MM:SS)
    if let Ok(dt) = parse_datetime(s) {
        return Ok(dt);
    }

    // Try datetime without seconds (YYYY-MM-DD HH:MM)
    if let Ok(naive_dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Local
            .from_local_datetime(&naive_dt)
            .single()
            .ok_or_else(|| miette::miette!("Invalid or ambiguous datetime: '{}'", s));
    }

    // Try time-only format (HH:MM:SS or HH:MM)
    // Note: This branch won't be reached for inputs like "10:30" that could match
    // parse_datetime, because parse_datetime expects a full date prefix and will fail.
    if let Ok(time) = parse_time_only(s) {
        let now = Local::now();
        let today = now.date_naive();
        let mut naive_dt = NaiveDateTime::new(today, time);
        let mut dt = Local
            .from_local_datetime(&naive_dt)
            .single()
            .ok_or_else(|| miette::miette!("Invalid or ambiguous datetime: '{}'", s))?;

        // If the interpreted time for today is in the future, assume the user meant yesterday
        // BUT only for --since. For --until, a future time today is valid.
        if is_since
            && dt > now
            && let Some(yesterday) = today.pred_opt()
        {
            naive_dt = NaiveDateTime::new(yesterday, time);
            dt = Local
                .from_local_datetime(&naive_dt)
                .single()
                .ok_or_else(|| miette::miette!("Invalid or ambiguous datetime: '{}'", s))?;
        }
        return Ok(dt);
    }

    if let Ok(duration) = humantime::parse_duration(s) {
        let now = Local::now();
        let target = now - chrono::Duration::from_std(duration).into_diagnostic()?;
        return Ok(target);
    }

    Err(miette::miette!(
        "Invalid time format: '{}'. Expected formats:\n\
         - Full datetime: \"YYYY-MM-DD HH:MM:SS\" or \"YYYY-MM-DD HH:MM\"\n\
         - Time only: \"HH:MM:SS\" or \"HH:MM\" (uses today's date)\n\
         - Relative time: \"5min\", \"2h\", \"1d\" (e.g., last 5 minutes)",
        s
    ))
}

fn parse_time_only(s: &str) -> Result<NaiveTime> {
    if let Ok(time) = NaiveTime::parse_from_str(s, "%H:%M:%S") {
        return Ok(time);
    }

    if let Ok(time) = NaiveTime::parse_from_str(s, "%H:%M") {
        return Ok(time);
    }

    Err(miette::miette!("Invalid time format: '{}'", s))
}

pub fn print_logs_for_time_range(
    daemon_id: &DaemonId,
    from: DateTime<Local>,
    to: Option<DateTime<Local>>,
) -> Result<()> {
    let daemon_ids = vec![daemon_id.clone()];
    let log_files = get_log_file_infos(&daemon_ids)?;

    let from = from
        .with_nanosecond(0)
        .expect("0 is always valid for nanoseconds");
    let to = to.map(|t| {
        t.with_nanosecond(0)
            .expect("0 is always valid for nanoseconds")
    });

    let log_lines = log_files
        .iter()
        .flat_map(
            |(daemon_id, lf)| match read_lines_in_time_range(&lf.path, Some(from), to) {
                Ok(lines) => merge_log_lines(&daemon_id.qualified(), lines, false),
                Err(e) => {
                    error!("{}: {}", lf.path.display(), e);
                    vec![]
                }
            },
        )
        .sorted_by_cached_key(|l| l.0.to_string())
        .collect_vec();

    if log_lines.is_empty() {
        eprintln!("No logs found for daemon '{daemon_id}' in the specified time range");
    } else {
        eprintln!("\n{} {} {}", edim("==="), edim("Error logs"), edim("==="));
        for (date, _id, msg) in log_lines {
            eprintln!("{} {}", edim(&date), msg);
        }
        eprintln!("{} {} {}\n", edim("==="), edim("End of logs"), edim("==="));
    }

    Ok(())
}

pub fn print_startup_logs(daemon_id: &DaemonId, from: DateTime<Local>) -> Result<()> {
    let daemon_ids = vec![daemon_id.clone()];
    let log_files = get_log_file_infos(&daemon_ids)?;

    let from = from
        .with_nanosecond(0)
        .expect("0 is always valid for nanoseconds");

    let log_lines = log_files
        .iter()
        .flat_map(
            |(daemon_id, lf)| match read_lines_in_time_range(&lf.path, Some(from), None) {
                Ok(lines) => merge_log_lines(&daemon_id.qualified(), lines, false),
                Err(e) => {
                    error!("{}: {}", lf.path.display(), e);
                    vec![]
                }
            },
        )
        .sorted_by_cached_key(|l| l.0.to_string())
        .collect_vec();

    if !log_lines.is_empty() {
        eprintln!("\n{} {} {}", edim("==="), edim("Startup logs"), edim("==="));
        for (date, _id, msg) in log_lines {
            eprintln!("{} {}", edim(&date), msg);
        }
        eprintln!("{} {} {}\n", edim("==="), edim("End of logs"), edim("==="));
    }

    Ok(())
}
