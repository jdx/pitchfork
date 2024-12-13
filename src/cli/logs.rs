use crate::watch_files::WatchFiles;
use crate::{env, Result};
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
#[clap(visible_alias = "l", verbatim_doc_comment)]
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
}

impl Logs {
    pub async fn run(&self) -> Result<()> {
        if self.clear {
            for id in &self.id {
                let log_dir = env::PITCHFORK_LOGS_DIR.join(id);
                let path = log_dir.join(format!("{}.log", id));
                if path.exists() {
                    xx::file::create(&path)?;
                }
            }
            return Ok(());
        }
        self.print_existing_logs()?;
        if self.tail {
            tail_logs(&self.id).await?;
        }

        Ok(())
    }

    fn print_existing_logs(&self) -> Result<()> {
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
            .sorted_by_cached_key(|l| l.0.to_string());

        let log_lines = if self.n == 0 {
            log_lines.collect_vec()
        } else {
            log_lines.rev().take(self.n).rev().collect_vec()
        };

        for (date, id, msg) in log_lines {
            if self.id.len() == 1 {
                println!("{} {}", date, msg);
            } else {
                println!("{} {} {}", date, id, msg);
            }
        }
        Ok(())
    }
}

fn merge_log_lines(id: &str, lines: Vec<String>) -> Vec<(String, String, String)> {
    lines.into_iter().fold(vec![], |mut acc, line| {
        match regex!(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) (\w)+ (.*)$").captures(&line) {
            Some(caps) => {
                let date = caps.get(1).unwrap().as_str().to_string();
                let msg = caps.get(3).unwrap().as_str().to_string();
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
            let name = files_to_name.get(&path).unwrap().to_string();
            let info = log_files.get_mut(&name).unwrap();
            info.file
                .seek(SeekFrom::Start(info.cur))
                .into_diagnostic()?;
            let reader = BufReader::new(&info.file);
            let lines = reader.lines().map_while(Result::ok).collect_vec();
            info.cur += lines.iter().fold(0, |acc, l| acc + l.len() as u64);
            out.extend(merge_log_lines(&name, lines));
        }
        let out = out
            .into_iter()
            .sorted_by_cached_key(|l| l.0.to_string())
            .collect_vec();
        for (date, name, msg) in out {
            println!("{} {} {}", date, name, msg);
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
