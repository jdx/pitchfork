use crate::ui::style::edim;
use crate::watch_files::WatchFiles;
use crate::{env, Result};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Timelike};
use itertools::Itertools;
use miette::IntoDiagnostic;
use notify::RecursiveMode;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
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
  pitchfork logs api              Show last 100 lines for 'api'
  pitchfork logs api worker       Show logs for multiple daemons
  pitchfork logs                  Show logs for all daemons
  pitchfork logs api -n 50        Show last 50 lines
  pitchfork logs api -n 0         Show all logs (no limit)
  pitchfork logs api --tail       Follow logs in real-time
  pitchfork logs api --from '2024-01-15 10:00:00'
                                  Show logs since a specific time
  pitchfork logs api --to '2024-01-15 12:00:00'
                                  Show logs until a specific time
  pitchfork logs api --clear      Delete logs for 'api'
  pitchfork logs --clear          Delete logs for all daemons"
)]
pub struct Logs {
    /// Show only logs for the specified daemon(s)
    id: Vec<String>,

    /// Delete logs
    #[clap(short, long)]
    clear: bool,

    /// Show N lines of logs
    ///
    /// Set to 0 to show all logs
    #[clap(short, default_value = "100")]
    n: usize,

    /// Show logs in real-time
    #[clap(short, long)]
    tail: bool,

    /// Show logs from this time (format: "YYYY-MM-DD HH:MM:SS")
    #[clap(long)]
    from: Option<String>,

    /// Show logs until this time (format: "YYYY-MM-DD HH:MM:SS")
    #[clap(long)]
    to: Option<String>,
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
                let path = log_dir.join(format!("{}.log", id));
                if path.exists() {
                    xx::file::create(&path)?;
                }
            }
            return Ok(());
        }

        let from = self.from.as_ref().and_then(|s| parse_datetime(s).ok());
        let to = self.to.as_ref().and_then(|s| parse_datetime(s).ok());

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
        let log_lines = log_files
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
                let lines = if self.n == 0 {
                    lines.collect_vec()
                } else {
                    lines.take(self.n).collect_vec()
                };
                merge_log_lines(name, lines)
            })
            .filter(|(date, _, _)| {
                if let Ok(dt) = parse_datetime(date) {
                    if let Some(from) = from {
                        if dt < from {
                            return false;
                        }
                    }
                    if let Some(to) = to {
                        if dt > to {
                            return false;
                        }
                    }
                    true
                } else {
                    true // Include lines without valid timestamps
                }
            })
            .sorted_by_cached_key(|l| l.0.to_string());

        let log_lines = if self.n == 0 {
            log_lines.collect_vec()
        } else {
            log_lines.rev().take(self.n).rev().collect_vec()
        };

        for (date, id, msg) in log_lines {
            if self.id.len() == 1 {
                println!("{} {}", edim(&date), msg);
            } else {
                println!("{} {} {}", edim(&date), id, msg);
            }
        }
        Ok(())
    }
}

fn merge_log_lines(id: &str, lines: Vec<String>) -> Vec<(String, String, String)> {
    lines.into_iter().fold(vec![], |mut acc, line| {
        match regex!(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) (\w)+ (.*)$").captures(&line) {
            Some(caps) => {
                let (date, msg) = match (caps.get(1), caps.get(3)) {
                    (Some(d), Some(m)) => (d.as_str().to_string(), m.as_str().to_string()),
                    _ => return acc, // Skip malformed lines
                };
                acc.push((date, id.to_string(), msg));
                acc
            }
            None => {
                if let Some(l) = acc.last_mut() {
                    l.2.push_str(&line)
                }
                acc
            }
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
            Ok((
                n.clone(),
                LogFile {
                    _name: n,
                    file: xx::file::open(&path)?,
                    // TODO: might be better to build the length when reading the file so we don't have gaps
                    cur: xx::file::metadata(&path).into_diagnostic()?.len(),
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
            info.cur += lines.iter().fold(0, |acc, l| acc + l.len() as u64);
            out.extend(merge_log_lines(name, lines));
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

/// Print logs for a specific daemon within a time range
/// This is a public API used by other commands (like run/start) to show logs on failure
pub fn print_logs_for_time_range(
    daemon_id: &str,
    from: DateTime<Local>,
    to: Option<DateTime<Local>>,
) -> Result<()> {
    let daemon_ids = vec![daemon_id.to_string()];
    let log_files = get_log_file_infos(&daemon_ids)?;

    // Truncate 'from' to second precision to match log timestamp precision
    // This ensures we include logs that occurred in the same second as the start time
    // Note: with_nanosecond(0) cannot fail since 0 is always valid
    let from = from
        .with_nanosecond(0)
        .expect("0 is always valid for nanoseconds");
    let to = to.map(|t| {
        t.with_nanosecond(0)
            .expect("0 is always valid for nanoseconds")
    });

    let log_lines = log_files
        .iter()
        .flat_map(|(name, lf)| {
            let rev = match xx::file::open(&lf.path) {
                Ok(f) => rev_lines::RevLines::new(f),
                Err(e) => {
                    error!("{}: {}", lf.path.display(), e);
                    return vec![];
                }
            };
            let lines = rev.into_iter().filter_map(Result::ok).collect_vec();
            merge_log_lines(name, lines)
        })
        .filter(|(date, _, _)| {
            if let Ok(dt) = parse_datetime(date) {
                // include logs at the exact start time
                if dt < from {
                    return false;
                }
                if let Some(to) = to {
                    if dt > to {
                        return false;
                    }
                }
                true
            } else {
                true
            }
        })
        .sorted_by_cached_key(|l| l.0.to_string())
        .collect_vec();

    if log_lines.is_empty() {
        eprintln!(
            "No logs found for daemon '{}' in the specified time range",
            daemon_id
        );
    } else {
        eprintln!("\n{} {} {}", edim("==="), edim("Error logs"), edim("==="));
        for (date, _id, msg) in log_lines {
            eprintln!("{} {}", edim(&date), msg);
        }
        eprintln!("{} {} {}\n", edim("==="), edim("End of logs"), edim("==="));
    }

    Ok(())
}
