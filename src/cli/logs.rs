use crate::ui::style::edim;
use crate::watch_files::WatchFiles;
use crate::{Result, env};
use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, TimeZone, Timelike};
use itertools::Itertools;
use miette::IntoDiagnostic;
use notify::RecursiveMode;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use xx::regex;

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
        if self.clear {
            let ids = if self.id.is_empty() {
                // Clear all logs when no daemon specified
                get_all_daemon_ids()?
            } else {
                self.id.clone()
            };
            for id in &ids {
                let log_dir = env::PITCHFORK_LOGS_DIR.join(id);
                let path = log_dir.join(format!("{id}.log"));
                if path.exists() {
                    xx::file::create(&path)?;
                }
            }
            return Ok(());
        }

        let from = self.since.as_ref().and_then(|s| parse_time_input(s).ok());
        let to = self.until.as_ref().and_then(|s| parse_time_input(s).ok());

        self.print_existing_logs(from, to)?;
        if self.tail {
            tail_logs(&self.id).await?;
        }

        Ok(())
    }

    fn print_existing_logs(
        &self,
        from: Option<DateTime<Local>>,
        to: Option<DateTime<Local>>,
    ) -> Result<()> {
        let log_files = get_log_file_infos(&self.id)?;
        trace!("log files for: {}", log_files.keys().join(", "));
        let single_daemon = self.id.len() == 1;
        let has_time_filter = from.is_some() || to.is_some();

        if self.raw {
            return self.print_raw_lines(&log_files, from, to, has_time_filter);
        }

        if has_time_filter {
            let mut log_lines = self.collect_log_lines_forward(&log_files, from, to)?;

            if let Some(n) = self.n {
                let len = log_lines.len();
                if len > n {
                    log_lines = log_lines.into_iter().skip(len - n).collect_vec();
                }
            }

            self.output_logs(log_lines, single_daemon, has_time_filter)?;
        } else if let Some(n) = self.n {
            let log_lines = self.collect_log_lines_reverse(&log_files, Some(n))?;
            self.output_logs(log_lines, single_daemon, has_time_filter)?;
        } else {
            self.stream_logs_to_pager(&log_files, single_daemon)?;
        }

        Ok(())
    }

    fn collect_log_lines_forward(
        &self,
        log_files: &BTreeMap<String, LogFile>,
        from: Option<DateTime<Local>>,
        to: Option<DateTime<Local>>,
    ) -> Result<Vec<(String, String, String)>> {
        let log_lines: Vec<(String, String, String)> = log_files
            .iter()
            .flat_map(
                |(name, lf)| match read_lines_in_time_range(&lf.path, from, to) {
                    Ok(lines) => merge_log_lines(name, lines, false),
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
        log_files: &BTreeMap<String, LogFile>,
        limit: Option<usize>,
    ) -> Result<Vec<(String, String, String)>> {
        let log_lines: Vec<(String, String, String)> = log_files
            .iter()
            .flat_map(|(name, lf)| {
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
                merge_log_lines(name, lines, true)
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
    ) -> Result<()> {
        if log_lines.is_empty() {
            return Ok(());
        }

        let use_pager = !self.no_pager && should_use_pager(log_lines.len());

        if use_pager {
            self.output_with_pager(log_lines, single_daemon, has_time_filter)?;
        } else {
            for (date, id, msg) in log_lines {
                if single_daemon {
                    println!("{} {}", edim(&date), msg);
                } else {
                    println!("{} {} {}", edim(&date), id, msg);
                }
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
        let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
        let mut pager_args = vec![];

        if pager == "less" {
            pager_args.push("-R");
            if !has_time_filter {
                pager_args.push("+G");
            }
        }

        let pager_result = Command::new(&pager)
            .args(&pager_args)
            .stdin(Stdio::piped())
            .spawn();

        match pager_result {
            Ok(mut child) => {
                let stdin = child.stdin.as_mut().expect("Failed to open pager stdin");

                for (date, id, msg) in log_lines {
                    let line = if single_daemon {
                        format!("{} {}\n", edim(&date), msg)
                    } else {
                        format!("{} {} {}\n", edim(&date), id, msg)
                    };
                    if stdin.write_all(line.as_bytes()).is_err() {
                        break;
                    }
                }

                let _ = child.wait();
            }
            Err(_) => {
                for (date, id, msg) in log_lines {
                    if single_daemon {
                        println!("{} {}", edim(&date), msg);
                    } else {
                        println!("{} {} {}", edim(&date), id, msg);
                    }
                }
            }
        }

        Ok(())
    }

    fn stream_logs_to_pager(
        &self,
        log_files: &BTreeMap<String, LogFile>,
        single_daemon: bool,
    ) -> Result<()> {
        if !io::stdout().is_terminal() || self.no_pager {
            return self.stream_logs_direct(log_files, single_daemon);
        }

        if log_files.len() == 1 {
            let (_, lf) = log_files.iter().next().unwrap();
            return self.open_file_in_pager(&lf.path);
        }

        let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
        let mut pager_args = vec![];

        if pager == "less" {
            pager_args.push("-R");
            pager_args.push("+G");
        }

        let pager_result = Command::new(&pager)
            .args(&pager_args)
            .stdin(Stdio::piped())
            .spawn();

        match pager_result {
            Ok(mut child) => {
                let stdin = child.stdin.take().expect("Failed to open pager stdin");

                // Use a separate thread to stream data, so pager can start immediately
                let log_files_clone: Vec<_> = log_files
                    .iter()
                    .map(|(name, lf)| (name.clone(), lf.path.clone()))
                    .collect();
                let single_daemon_clone = single_daemon;

                std::thread::spawn(move || {
                    let mut writer = BufWriter::new(stdin);

                    for (name, path) in log_files_clone {
                        let file = match File::open(&path) {
                            Ok(f) => f,
                            Err(_) => continue,
                        };

                        let reader = BufReader::new(file);
                        let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
                        let parsed = parse_log_lines(&lines);

                        for (date, msg) in parsed {
                            let output = if single_daemon_clone {
                                format!("{} {}\n", edim(&date), msg)
                            } else {
                                format!("{} {} {}\n", edim(&date), name, msg)
                            };
                            if writer.write_all(output.as_bytes()).is_err() {
                                return;
                            }
                        }
                    }

                    let _ = writer.flush();
                });

                let _ = child.wait();
            }
            Err(_) => {
                return self.stream_logs_direct(log_files, single_daemon);
            }
        }

        Ok(())
    }

    fn open_file_in_pager(&self, path: &Path) -> Result<()> {
        let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
        let mut pager_args = vec![];

        if pager == "less" {
            pager_args.push("-R");
            pager_args.push("+G");
        }

        pager_args.push(path.to_str().unwrap());

        let status = Command::new(&pager)
            .args(&pager_args)
            .status()
            .into_diagnostic()?;

        if !status.success() {
            return Err(miette::miette!("Pager exited with error"));
        }

        Ok(())
    }
    fn stream_logs_direct(
        &self,
        log_files: &BTreeMap<String, LogFile>,
        single_daemon: bool,
    ) -> Result<()> {
        use std::io::Write;

        for (name, lf) in log_files {
            let file = match File::open(&lf.path) {
                Ok(f) => f,
                Err(e) => {
                    error!("{}: {}", lf.path.display(), e);
                    continue;
                }
            };

            let reader = BufReader::new(file);
            let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
            let parsed = parse_log_lines(&lines);

            for (date, msg) in parsed {
                let output = if single_daemon {
                    format!("{} {}\n", edim(&date), msg)
                } else {
                    format!("{} {} {}\n", edim(&date), name, msg)
                };
                if io::stdout().write_all(output.as_bytes()).is_err() {
                    return Ok(());
                }
            }
        }

        Ok(())
    }

    fn print_raw_lines(
        &self,
        log_files: &BTreeMap<String, LogFile>,
        from: Option<DateTime<Local>>,
        to: Option<DateTime<Local>>,
        has_time_filter: bool,
    ) -> Result<()> {
        if has_time_filter {
            let mut all_lines: Vec<String> = log_files
                .iter()
                .flat_map(
                    |(_, lf)| match read_lines_in_time_range(&lf.path, from, to) {
                        Ok(lines) => lines,
                        Err(e) => {
                            error!("{}: {}", lf.path.display(), e);
                            vec![]
                        }
                    },
                )
                .collect();

            if let Some(n) = self.n {
                let len = all_lines.len();
                if len > n {
                    all_lines = all_lines.into_iter().skip(len - n).collect();
                }
            }

            for line in all_lines {
                println!("{}", line);
            }
        } else if let Some(n) = self.n {
            let mut all_lines: Vec<String> = log_files
                .iter()
                .flat_map(|(_, lf)| {
                    let rev = match xx::file::open(&lf.path) {
                        Ok(f) => rev_lines::RevLines::new(f),
                        Err(e) => {
                            error!("{}: {}", lf.path.display(), e);
                            return vec![];
                        }
                    };
                    rev.into_iter()
                        .filter_map(Result::ok)
                        .take(n)
                        .collect::<Vec<_>>()
                })
                .collect();

            all_lines.reverse();

            let len = all_lines.len();
            if len > n {
                all_lines = all_lines.into_iter().skip(len - n).collect();
            }

            for line in all_lines {
                println!("{}", line);
            }
        } else {
            for lf in log_files.values() {
                let file = match File::open(&lf.path) {
                    Ok(f) => f,
                    Err(e) => {
                        error!("{}: {}", lf.path.display(), e);
                        continue;
                    }
                };

                let reader = BufReader::new(file);
                for line in reader.lines() {
                    match line {
                        Ok(l) => println!("{}", l),
                        Err(_) => continue,
                    }
                }
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

    if let Ok(output) = Command::new("tput").arg("lines").output()
        && output.status.success()
        && let Ok(s) = String::from_utf8(output.stdout)
        && let Ok(h) = s.trim().parse::<usize>()
    {
        return Some(h);
    }

    None
}

fn read_lines_in_time_range(
    path: &PathBuf,
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
        if reader.read_line(&mut line).into_diagnostic()? == 0 {
            high = mid;
            continue;
        }

        let line_time = extract_timestamp(&line);

        match line_time {
            Some(lt) => {
                if find_start {
                    if lt < target_time {
                        low = line_start + line.len() as u64;
                    } else {
                        high = line_start;
                    }
                } else {
                    if lt <= target_time {
                        low = line_start + line.len() as u64;
                    } else {
                        high = line_start;
                    }
                }
            }
            None => {
                low = line_start + line.len() as u64;
            }
        }
    }

    find_line_start(file, low)
}

fn find_line_start(file: &mut File, pos: u64) -> Result<u64> {
    if pos == 0 {
        return Ok(0);
    }

    let search_start = pos.saturating_sub(1);
    file.seek(SeekFrom::Start(search_start)).into_diagnostic()?;

    let mut search_pos = search_start;
    let mut buf = [0u8; 1];

    loop {
        if file.read_exact(&mut buf).is_err() {
            return Ok(0);
        }

        if buf[0] == b'\n' {
            return Ok(search_pos + 1);
        }

        if search_pos == 0 {
            return Ok(0);
        }

        search_pos -= 1;
        file.seek(SeekFrom::Start(search_pos)).into_diagnostic()?;
    }
}

fn extract_timestamp(line: &str) -> Option<DateTime<Local>> {
    let re = regex!(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})");
    re.captures(line)
        .and_then(|caps| caps.get(1))
        .and_then(|m| parse_datetime(m.as_str()).ok())
}

fn parse_log_lines(lines: &[String]) -> Vec<(String, String)> {
    let re = regex!(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) (\w)+ (.*)$");
    lines
        .iter()
        .fold(vec![], |mut acc, line| match re.captures(line) {
            Some(caps) => {
                let (date, msg) = match (caps.get(1), caps.get(3)) {
                    (Some(d), Some(m)) => (d.as_str().to_string(), m.as_str().to_string()),
                    _ => return acc,
                };
                acc.push((date, msg));
                acc
            }
            None => {
                if let Some(l) = acc.last_mut() {
                    l.1.push('\n');
                    l.1.push_str(line);
                }
                acc
            }
        })
}

fn merge_log_lines(id: &str, lines: Vec<String>, reverse: bool) -> Vec<(String, String, String)> {
    let lines = if reverse {
        lines.into_iter().rev().collect()
    } else {
        lines
    };

    let re = regex!(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) (\w)+ (.*)$");
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

fn get_all_daemon_ids() -> Result<Vec<String>> {
    Ok(xx::file::ls(&*env::PITCHFORK_LOGS_DIR)?
        .into_iter()
        .filter(|d| !d.starts_with("."))
        .filter(|d| d.is_dir())
        .filter_map(|d| d.file_name().map(|f| f.to_string_lossy().to_string()))
        .collect())
}

fn get_log_file_infos(names: &[String]) -> Result<BTreeMap<String, LogFile>> {
    let names = names.iter().collect::<HashSet<_>>();
    xx::file::ls(&*env::PITCHFORK_LOGS_DIR)?
        .into_iter()
        .filter(|d| !d.starts_with("."))
        .filter(|d| d.is_dir())
        .filter_map(|d| d.file_name().map(|f| f.to_string_lossy().to_string()))
        .filter(|n| names.is_empty() || names.contains(n))
        .map(|n| {
            let path = env::PITCHFORK_LOGS_DIR
                .join(&n)
                .join(format!("{n}.log"))
                .canonicalize()
                .into_diagnostic()?;
            let mut file = xx::file::open(&path)?;
            // Seek to end and get position atomically to avoid race condition
            // where content is written between metadata check and file open
            file.seek(SeekFrom::End(0)).into_diagnostic()?;
            let cur = file.stream_position().into_diagnostic()?;
            Ok((
                n.clone(),
                LogFile {
                    _name: n,
                    file,
                    cur,
                    path,
                },
            ))
        })
        .filter_ok(|(_, f)| f.path.exists())
        .collect::<Result<BTreeMap<_, _>>>()
}

pub async fn tail_logs(names: &[String]) -> Result<()> {
    let mut log_files = get_log_file_infos(names)?;
    let mut wf = WatchFiles::new(Duration::from_millis(10))?;

    for lf in log_files.values() {
        wf.watch(&lf.path, RecursiveMode::NonRecursive)?;
    }

    let files_to_name = log_files
        .iter()
        .map(|(n, f)| (f.path.clone(), n.clone()))
        .collect::<HashMap<_, _>>();

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
            out.extend(merge_log_lines(name, lines, false));
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
    _name: String,
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

fn parse_time_input(s: &str) -> Result<DateTime<Local>> {
    let s = s.trim();

    if let Ok(dt) = parse_datetime(s) {
        return Ok(dt);
    }

    if let Ok(naive_dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Local
            .from_local_datetime(&naive_dt)
            .single()
            .ok_or_else(|| miette::miette!("Invalid or ambiguous datetime: '{}'", s));
    }

    if let Ok(time) = parse_time_only(s) {
        let today = Local::now().date_naive();
        let naive_dt = NaiveDateTime::new(today, time);
        return Local
            .from_local_datetime(&naive_dt)
            .single()
            .ok_or_else(|| miette::miette!("Invalid or ambiguous datetime: '{}'", s));
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
    daemon_id: &str,
    from: DateTime<Local>,
    to: Option<DateTime<Local>>,
) -> Result<()> {
    let daemon_ids = vec![daemon_id.to_string()];
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
            |(name, lf)| match read_lines_in_time_range(&lf.path, Some(from), to) {
                Ok(lines) => merge_log_lines(name, lines, false),
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

pub fn print_startup_logs(daemon_id: &str, from: DateTime<Local>) -> Result<()> {
    let daemon_ids = vec![daemon_id.to_string()];
    let log_files = get_log_file_infos(&daemon_ids)?;

    let from = from
        .with_nanosecond(0)
        .expect("0 is always valid for nanoseconds");

    let log_lines = log_files
        .iter()
        .flat_map(
            |(name, lf)| match read_lines_in_time_range(&lf.path, Some(from), None) {
                Ok(lines) => merge_log_lines(name, lines, false),
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
