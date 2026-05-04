use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::PitchforkToml;
use crate::state_file::StateFile;
use crate::ui::style::{edim, estyle, ndim};
use crate::{Result, env};
use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, TimeZone, Timelike};
use console;
use itertools::Itertools;
use miette::IntoDiagnostic;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeSet, BinaryHeap};
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
/// `id_width` is the display width used to pad the daemon name column
/// so messages line up vertically across different daemon names.
/// When `strip_ansi` is true, strips ANSI escape codes from the message.
fn format_log_line(
    date: &str,
    id: &str,
    msg: &str,
    single_daemon: bool,
    id_width: usize,
    strip_ansi: bool,
) -> String {
    let msg = if strip_ansi {
        console::strip_ansi_codes(msg).to_string()
    } else {
        msg.to_string()
    };
    if single_daemon {
        format!("{} {}", ndim(date), msg)
    } else {
        let colors_on = !strip_ansi && console::colors_enabled();
        let colored = dimmed_id(id, colors_on);
        let padded = console::pad_str(&colored, id_width, console::Alignment::Left, None);
        format!("{}  {} {}", padded, ndim(date), msg)
    }
}

/// Return a dimmed, colorized daemon ID string for display.
/// Each daemon gets a deterministic color via FNV-1a hash so that
/// multiple daemons are visually distinguishable while remaining subtle.
fn dimmed_id(id: &str, colors_enabled: bool) -> String {
    if !colors_enabled {
        return id.to_string();
    }
    let colors = [
        (180, 120, 120), // dim red
        (180, 160, 100), // dim yellow
        (120, 180, 120), // dim green
        (120, 180, 180), // dim cyan
        (180, 120, 180), // dim magenta
        (120, 160, 180), // dim blue
    ];
    let mut h: usize = 0x811C_9DC5; // FNV offset basis
    for b in id.bytes() {
        h = h.wrapping_mul(0x0100_0193).wrapping_add(b as usize);
    }
    let (r, g, b) = colors[h % colors.len()];
    format!("\x1b[2;38;2;{};{};{}m{}\x1b[0m", r, g, b, id)
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
        // Migrate legacy log directories (old format: "api" → new format: "legacy--api").
        // This is idempotent and silent so it is safe to run on every invocation.
        migrate_legacy_log_dirs();

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

        let single_daemon = resolved_ids.len() == 1;
        self.print_existing_logs(&resolved_ids, from, to, single_daemon)?;
        if self.tail {
            tail_logs(&resolved_ids, single_daemon, true).await?;
        }

        Ok(())
    }

    fn print_existing_logs(
        &self,
        resolved_ids: &[DaemonId],
        from: Option<DateTime<Local>>,
        to: Option<DateTime<Local>>,
        single_daemon: bool,
    ) -> Result<()> {
        let valid_ids: Vec<DaemonId> = resolved_ids
            .iter()
            .filter(|id| id.log_path().exists())
            .cloned()
            .collect();
        let id_width = valid_ids
            .iter()
            .map(|id| id.qualified().len())
            .max()
            .unwrap_or(0);
        trace!(
            "log files for: {}",
            valid_ids.iter().map(|id| id.qualified()).join(", ")
        );
        let has_time_filter = from.is_some() || to.is_some();

        if has_time_filter {
            let mut log_lines = self.collect_log_lines_forward(&valid_ids, from, to)?;

            if let Some(n) = self.n {
                let len = log_lines.len();
                if len > n {
                    log_lines = log_lines.into_iter().skip(len - n).collect_vec();
                }
            }

            self.output_logs(
                log_lines,
                single_daemon,
                id_width,
                has_time_filter,
                self.raw,
            )?;
        } else if let Some(n) = self.n {
            let log_lines = self.collect_log_lines_reverse(&valid_ids, Some(n))?;
            self.output_logs(
                log_lines,
                single_daemon,
                id_width,
                has_time_filter,
                self.raw,
            )?;
        } else {
            self.stream_logs_to_pager(&valid_ids, single_daemon, id_width, self.raw)?;
        }

        Ok(())
    }

    fn collect_log_lines_forward(
        &self,
        ids: &[DaemonId],
        from: Option<DateTime<Local>>,
        to: Option<DateTime<Local>>,
    ) -> Result<Vec<(String, String, String)>> {
        let log_lines: Vec<(String, String, String)> = ids
            .iter()
            .flat_map(|id| {
                let path = id.log_path();
                match read_lines_in_time_range(&path, from, to) {
                    Ok(lines) => merge_log_lines(&id.qualified(), lines, false),
                    Err(e) => {
                        error!("{}: {}", path.display(), e);
                        vec![]
                    }
                }
            })
            .sorted_by_cached_key(|l| l.0.to_string())
            .collect_vec();

        Ok(log_lines)
    }

    fn collect_log_lines_reverse(
        &self,
        ids: &[DaemonId],
        limit: Option<usize>,
    ) -> Result<Vec<(String, String, String)>> {
        let log_lines: Vec<(String, String, String)> = ids
            .iter()
            .flat_map(|id| {
                let path = id.log_path();
                let rev = match xx::file::open(&path) {
                    Ok(f) => rev_lines::RevLines::new(f),
                    Err(e) => {
                        error!("{}: {}", path.display(), e);
                        return vec![];
                    }
                };
                let lines = rev.into_iter().filter_map(Result::ok);
                let lines = match limit {
                    Some(n) => lines.take(n).collect_vec(),
                    None => lines.collect_vec(),
                };
                merge_log_lines(&id.qualified(), lines, true)
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
        id_width: usize,
        has_time_filter: bool,
        raw: bool,
    ) -> Result<()> {
        if log_lines.is_empty() {
            return Ok(());
        }

        let strip_ansi = raw || !console::colors_enabled();

        // Raw mode: output without formatting and without pager
        if raw {
            for (date, id, msg) in log_lines {
                let line = format_log_line(&date, &id, &msg, single_daemon, id_width, strip_ansi);
                println!("{line}");
            }
            return Ok(());
        }

        let use_pager = !self.no_pager && should_use_pager(log_lines.len());

        if use_pager {
            self.output_with_pager(
                log_lines,
                single_daemon,
                id_width,
                has_time_filter,
                strip_ansi,
            )?;
        } else {
            for (date, id, msg) in log_lines {
                println!(
                    "{}",
                    format_log_line(&date, &id, &msg, single_daemon, id_width, strip_ansi)
                );
            }
        }

        Ok(())
    }

    fn output_with_pager(
        &self,
        log_lines: Vec<(String, String, String)>,
        single_daemon: bool,
        id_width: usize,
        has_time_filter: bool,
        strip_ansi: bool,
    ) -> Result<()> {
        // When time filter is used, start at top; otherwise start at end
        let pager_config = PagerConfig::new(!has_time_filter);

        match pager_config.spawn_piped() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    for (date, id, msg) in log_lines {
                        let line = format!(
                            "{}\n",
                            format_log_line(&date, &id, &msg, single_daemon, id_width, strip_ansi)
                        );
                        if stdin.write_all(line.as_bytes()).is_err() {
                            break;
                        }
                    }
                    let _ = child.wait();
                } else {
                    debug!("Failed to get pager stdin, falling back to direct output");
                    for (date, id, msg) in log_lines {
                        println!(
                            "{}",
                            format_log_line(&date, &id, &msg, single_daemon, id_width, strip_ansi)
                        );
                    }
                }
            }
            Err(e) => {
                debug!("Failed to spawn pager: {e}, falling back to direct output");
                for (date, id, msg) in log_lines {
                    println!(
                        "{}",
                        format_log_line(&date, &id, &msg, single_daemon, id_width, strip_ansi)
                    );
                }
            }
        }

        Ok(())
    }

    fn stream_logs_to_pager(
        &self,
        ids: &[DaemonId],
        single_daemon: bool,
        id_width: usize,
        raw: bool,
    ) -> Result<()> {
        let strip_ansi = raw || !console::colors_enabled();

        if !io::stdout().is_terminal() || self.no_pager || self.tail || raw {
            return self.stream_logs_direct(ids, single_daemon, id_width, raw, strip_ansi);
        }

        let pager_config = PagerConfig::new(true); // start_at_end = true

        match pager_config.spawn_piped() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.take() {
                    // Collect file info for the streaming thread
                    let file_infos: Vec<_> = ids
                        .iter()
                        .map(|id| (id.qualified(), id.log_path()))
                        .collect();
                    let single_daemon_clone = single_daemon;
                    let strip_ansi_clone = strip_ansi;
                    let id_width_clone = id_width;

                    // Stream logs using a background thread to avoid blocking
                    std::thread::spawn(move || {
                        let mut writer = BufWriter::new(stdin);

                        // Single file: stream directly without merge overhead
                        if file_infos.len() == 1 {
                            let (name, path) = &file_infos[0];
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
                                        single_daemon_clone,
                                        id_width_clone,
                                        strip_ansi_clone
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

                        for (name, path) in file_infos {
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
                                format_log_line(
                                    &timestamp,
                                    &daemon,
                                    &message,
                                    single_daemon_clone,
                                    id_width_clone,
                                    strip_ansi_clone
                                )
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
                    return self.stream_logs_direct(ids, single_daemon, id_width, raw, strip_ansi);
                }
            }
            Err(e) => {
                debug!("Failed to spawn pager: {e}, falling back to direct output");
                return self.stream_logs_direct(ids, single_daemon, id_width, raw, strip_ansi);
            }
        }

        Ok(())
    }

    fn stream_logs_direct(
        &self,
        ids: &[DaemonId],
        single_daemon: bool,
        id_width: usize,
        raw: bool,
        strip_ansi: bool,
    ) -> Result<()> {
        // Fast path for single daemon: directly output file content without parsing
        // This avoids expensive regex parsing for each line in large log files
        if ids.len() == 1 {
            let daemon_id = &ids[0];
            let path = daemon_id.log_path();
            let file = match File::open(&path) {
                Ok(f) => f,
                Err(e) => {
                    error!("{}: {}", path.display(), e);
                    return Ok(());
                }
            };
            let reader = BufReader::new(file);
            if raw {
                // Raw mode: output lines as-is (but strip ansi if colors disabled)
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            let l = if strip_ansi {
                                console::strip_ansi_codes(&l).to_string()
                            } else {
                                l
                            };
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
                let parser = StreamingLogParser::new(File::open(&path).into_diagnostic()?);
                for (timestamp, message) in parser {
                    let output = format!(
                        "{}\n",
                        format_log_line(
                            &timestamp,
                            &daemon_id.qualified(),
                            &message,
                            single_daemon,
                            id_width,
                            strip_ansi
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

        for id in ids {
            let path = id.log_path();
            let file = match File::open(&path) {
                Ok(f) => f,
                Err(e) => {
                    error!("{}: {}", path.display(), e);
                    continue;
                }
            };
            let parser = StreamingLogParser::new(file);
            merger.add_source(id.qualified(), parser);
        }

        // Initialize the heap with first entry from each source
        merger.initialize();

        // Stream merged entries to stdout
        for (timestamp, daemon, message) in merger {
            let output = format!(
                "{}\n",
                format_log_line(
                    &timestamp,
                    &daemon,
                    &message,
                    single_daemon,
                    id_width,
                    strip_ansi
                )
            );
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

/// Rename legacy log directories that predate namespace-qualified daemon IDs.
///
/// Old layout: `PITCHFORK_LOGS_DIR/<name>/<name>.log`
/// New layout: `PITCHFORK_LOGS_DIR/legacy--<name>/legacy--<name>.log`
///
/// Only directories that clearly match the old layout are migrated:
/// - directory name does not contain `"--"`
/// - directory contains `<name>.log`
/// - `<name>` is a valid daemon short name under current DaemonId rules
fn migrate_legacy_log_dirs() {
    let known_safe_paths = known_daemon_safe_paths();
    let dirs = match xx::file::ls(&*env::PITCHFORK_LOGS_DIR) {
        Ok(d) => d,
        Err(_) => return,
    };
    for dir in dirs {
        if dir.starts_with(".") || !dir.is_dir() {
            continue;
        }
        let name = match dir.file_name().map(|f| f.to_string_lossy().to_string()) {
            Some(n) => n,
            None => continue,
        };
        // New-format directories usually contain "--". For safety, only treat
        // them as new-format if they match a known daemon ID safe-path.
        if name.contains("--") {
            // If it parses as a valid safe-path, treat it as already migrated
            // and keep idempotent behavior silent.
            if DaemonId::from_safe_path(&name).is_ok() {
                continue;
            }
            // Keep noisy warnings only for invalid/ambiguous names that cannot
            // be interpreted as new-format IDs.
            if known_safe_paths.contains(&name) {
                continue;
            }
            warn!(
                "Skipping invalid legacy log directory '{name}': contains '--' but is not a valid daemon safe-path"
            );
            continue;
        }

        // Migrate only explicit old-layout directories to avoid renaming
        // unrelated folders under logs/.
        let old_log = dir.join(format!("{name}.log"));
        if !old_log.exists() {
            continue;
        }
        if DaemonId::try_new("legacy", &name).is_err() {
            warn!("Skipping invalid legacy log directory '{name}': not a valid daemon ID");
            continue;
        }

        let new_name = format!("legacy--{name}");
        let new_dir = env::PITCHFORK_LOGS_DIR.join(&new_name);
        // Skip if a target directory already exists to avoid clobbering data.
        if new_dir.exists() {
            continue;
        }
        if std::fs::rename(&dir, &new_dir).is_err() {
            continue;
        }
        // Also rename the log file inside the directory.
        let old_log = new_dir.join(format!("{name}.log"));
        let new_log = new_dir.join(format!("{new_name}.log"));
        if old_log.exists() {
            let _ = std::fs::rename(&old_log, &new_log);
        }
        debug!("Migrated legacy log dir '{name}' → '{new_name}'");
    }
}

fn known_daemon_safe_paths() -> BTreeSet<String> {
    let mut out = BTreeSet::new();

    match StateFile::read(&*env::PITCHFORK_STATE_FILE) {
        Ok(state) => {
            for id in state.daemons.keys() {
                out.insert(id.safe_path());
            }
        }
        Err(e) => {
            warn!("Failed to read state while checking known daemon IDs: {e}");
        }
    }

    match PitchforkToml::all_merged() {
        Ok(config) => {
            for id in config.daemons.keys() {
                out.insert(id.safe_path());
            }
        }
        Err(e) => {
            warn!("Failed to read config while checking known daemon IDs: {e}");
        }
    }

    out
}

fn get_all_daemon_ids() -> Result<Vec<DaemonId>> {
    let mut ids = BTreeSet::new();

    match StateFile::read(&*env::PITCHFORK_STATE_FILE) {
        Ok(state) => ids.extend(state.daemons.keys().cloned()),
        Err(e) => warn!("Failed to read state for log daemon discovery: {e}"),
    }

    match PitchforkToml::all_merged() {
        Ok(config) => ids.extend(config.daemons.keys().cloned()),
        Err(e) => warn!("Failed to read config for log daemon discovery: {e}"),
    }

    Ok(ids
        .into_iter()
        .filter(|id| id.log_path().exists())
        .collect())
}

pub async fn tail_logs(
    names: &[DaemonId],
    single_daemon: bool,
    start_from_end: bool,
) -> Result<()> {
    // Poll each log file in a loop instead of using file-system event watchers.
    //
    // Why polling:
    // - Real-time enough: 200ms interval is imperceptible for human consumption,
    //   and comparable to what `tail -f` provides.
    // - No long-running overhead: `logs --tail` runs in the foreground with the
    //   user watching output; the polling stops when the process exits.
    // - Cross-platform reliable: avoids edge cases in notify/FSEvents where events
    //   can be missed when the writer uses buffered I/O.
    //
    // `start_from_end`: when true, skip content already output by a prior
    // print_existing_logs call (used by `logs --tail`). When false, start from
    // the beginning so no content is missed (used by `wait`).
    let id_width = names
        .iter()
        .map(|id| id.qualified().len())
        .max()
        .unwrap_or(0);

    let mut states: Vec<(DaemonId, PathBuf, u64)> = names
        .iter()
        .filter_map(|id| {
            let path = id.log_path();
            if !path.exists() {
                return None;
            }
            let pos = if start_from_end {
                fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            } else {
                0
            };
            Some((id.clone(), path, pos))
        })
        .collect();

    let strip_ansi = !console::colors_enabled();

    let interval = tokio::time::interval(Duration::from_millis(200));
    tokio::pin!(interval);

    loop {
        interval.tick().await;

        // Discover log files that appeared since last iteration.
        // Always start from position 0 — content written between ticks
        // must not be silently dropped.
        for id in names {
            let path = id.log_path();
            if !path.exists() || states.iter().any(|(s, _, _)| s == id) {
                continue;
            }
            states.push((id.clone(), path, 0));
        }

        let mut out = vec![];
        for (id, path, pos) in &mut states {
            let mut file = match fs::File::open(path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let file_size = match file.metadata() {
                Ok(m) => m.len(),
                Err(_) => continue,
            };
            let start = if *pos > file_size { 0 } else { *pos };
            file.seek(SeekFrom::Start(start)).into_diagnostic()?;

            // Track bytes consumed rather than using stream_position(),
            // which includes BufReader's read-ahead buffer and would skip
            // content written concurrently.
            let mut reader = BufReader::new(&file);
            let mut bytes_read: u64 = 0;
            let mut lines = vec![];
            loop {
                let mut line = String::new();
                let n = reader.read_line(&mut line).into_diagnostic()?;
                if n == 0 {
                    break;
                }
                // Only advance position for complete lines (ending with \n).
                // Partial lines at the end of file may still be written to;
                // leave them for the next tick.
                if line.ends_with('\n') {
                    bytes_read += n as u64;
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                    lines.push(line);
                } else {
                    // Partial line — don't advance, will retry next tick.
                    break;
                }
            }
            *pos = start + bytes_read;
            out.extend(merge_log_lines(&id.qualified(), lines, false));
        }

        if !out.is_empty() {
            let out = out
                .into_iter()
                .sorted_by_cached_key(|l| l.0.to_string())
                .collect_vec();
            for (date, name, msg) in out {
                println!(
                    "{}",
                    format_log_line(&date, &name, &msg, single_daemon, id_width, strip_ansi)
                );
            }
        }
    }
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
    let from = from
        .with_nanosecond(0)
        .expect("0 is always valid for nanoseconds");
    let to = to.map(|t| {
        t.with_nanosecond(0)
            .expect("0 is always valid for nanoseconds")
    });

    let path = daemon_id.log_path();
    let log_lines = if path.exists() {
        match read_lines_in_time_range(&path, Some(from), to) {
            Ok(lines) => merge_log_lines(&daemon_id.qualified(), lines, false),
            Err(e) => {
                error!("{}: {}", path.display(), e);
                vec![]
            }
        }
    } else {
        vec![]
    };

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

/// Collects startup log lines for a single daemon (does not print).
///
/// Returns a list of `(time, daemon_id_qualified, message)` tuples for log
/// entries written after `from`.
pub fn collect_startup_logs(
    daemon_id: &DaemonId,
    from: DateTime<Local>,
) -> Result<Vec<(String, String, String)>> {
    let from = from
        .with_nanosecond(0)
        .expect("0 is always valid for nanoseconds");

    let path = daemon_id.log_path();
    let log_lines = if path.exists() {
        match read_lines_in_time_range(&path, Some(from), None) {
            Ok(lines) => merge_log_lines(&daemon_id.qualified(), lines, false),
            Err(e) => {
                error!("{}: {}", path.display(), e);
                vec![]
            }
        }
    } else {
        vec![]
    };

    Ok(log_lines)
}

/// Prints collected startup log lines for all daemons in a unified block.
///
/// When only one daemon ID appears in the log lines, omits the ID column since
/// it would be redundant.  When multiple daemons are present, aligns the ID column.
///
/// Format (single daemon):
/// ```text
///   STARTUP LOGS
///   17:12:14 v24.14.0
/// ```
///
/// Format (multiple daemons):
/// ```text
///   STARTUP LOGS
///   api     17:12:14 v3.1.0 ready
///   web     17:12:14 listening on 0.0.0.0:8080
/// ```
pub fn print_startup_logs_block(log_lines: &[(String, String, String)]) {
    if log_lines.is_empty() {
        return;
    }

    // Sort by timestamp so logs from multiple daemons are interleaved
    // rather than grouped per-daemon.
    let log_lines = log_lines
        .iter()
        .sorted_by_cached_key(|(ts, _, _)| ts.clone())
        .collect_vec();

    // Unique daemon IDs to decide whether to show the ID column.
    // We decide based on what's actually in the logs, not how many daemons
    // were started — if multiple daemons started but only one emitted logs,
    // the user still needs to know which daemon the logs belong to.
    let unique_ids: BTreeSet<&str> = log_lines.iter().map(|(_, id, _)| id.as_str()).collect();
    let show_id = unique_ids.len() > 1;

    // Filter PTY control sequences from log messages, keeping SGR (color) codes.
    // Non-tty: also strip all remaining ANSI color codes.
    let is_tty = std::io::stderr().is_terminal();
    let format_msg = |msg: &str| -> String {
        let stripped = strip_pty_controls(msg);
        if is_tty {
            stripped
        } else {
            console::strip_ansi_codes(&stripped).to_string()
        }
    };

    // Tag with dim background style, always on its own line
    let tag = estyle(" STARTUP LOGS ").black().on_color256(8); // dark gray bg
    eprintln!("\n{tag}");

    if show_id {
        let id_width = log_lines
            .iter()
            .map(|(_, id, _)| console::measure_text_width(id))
            .max()
            .unwrap_or(0);
        for (date, id, msg) in log_lines {
            let time = date.split(' ').nth(1).unwrap_or(date);
            let colored = dimmed_id(id, is_tty && console::colors_enabled_stderr());
            let padded = console::pad_str(&colored, id_width, console::Alignment::Left, None);
            eprintln!("{}  {} {}", padded, edim(time), format_msg(msg));
        }
    } else {
        for (date, _, msg) in log_lines {
            let time = date.split(' ').nth(1).unwrap_or(date);
            eprintln!("{} {}", edim(time), format_msg(msg));
        }
    }
}

/// Strips PTY control sequences from a string while preserving SGR (color/style) codes.
///
/// Removes CSI sequences that control cursor movement, screen clearing, erasing, etc.,
/// but keeps `\x1b[...m` (SGR) sequences so colors are retained.
fn strip_pty_controls(s: &str) -> String {
    struct Stripper {
        result: String,
    }

    impl vte::Perform for Stripper {
        fn print(&mut self, c: char) {
            self.result.push(c);
        }

        fn execute(&mut self, byte: u8) {
            // Keep \n and \t; drop other control characters (BEL, BS, CR, etc.)
            if byte == b'\n' || byte == b'\t' {
                self.result.push(byte as char);
            }
        }

        fn csi_dispatch(
            &mut self,
            params: &vte::Params,
            _intermediates: &[u8],
            _ignore: bool,
            action: char,
        ) {
            // Keep SGR sequences (final byte 'm')
            if action == 'm' {
                self.result.push_str("\x1b[");
                let mut first = true;
                for sub in params.iter() {
                    if !first {
                        self.result.push(';');
                    }
                    first = false;
                    for (i, &p) in sub.iter().enumerate() {
                        if i > 0 {
                            self.result.push(':');
                        }
                        self.result.push_str(&p.to_string());
                    }
                }
                self.result.push('m');
            }
            // All other CSI sequences (cursor move, clear, erase, etc.) are dropped
        }

        fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
            // Drop OSC sequences (e.g. window title)
        }

        fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {
            // Drop ESC sequences (e.g. ESC c = reset terminal)
        }

        fn hook(
            &mut self,
            _params: &vte::Params,
            _intermediates: &[u8],
            _ignore: bool,
            _action: char,
        ) {
            // Drop DCS hooks
        }

        fn put(&mut self, _byte: u8) {
            // Drop DCS data
        }

        fn unhook(&mut self) {
            // Drop DCS unhook
        }
    }

    let mut parser = vte::Parser::new();
    let mut stripper = Stripper {
        result: String::with_capacity(s.len()),
    };
    parser.advance(&mut stripper, s.as_bytes());
    stripper.result
}
