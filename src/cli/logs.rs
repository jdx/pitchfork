use crate::{env, Result};
use itertools::Itertools;
use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::time::Duration;
use xx::regex;

/// Displays logs for daemon(s)
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Logs {
    /// Show only logs for the specified daemon(s)
    name: Vec<String>,

    /// Show N lines of logs
    ///
    /// Set to 0 to show all logs
    #[clap(short, default_value = "10")]
    n: usize,

    /// Show logs in real-time
    #[clap(short, long)]
    tail: bool,
}

impl Logs {
    pub async fn run(&self) -> Result<()> {
        let names = self.name.iter().collect::<HashSet<_>>();
        let log_files = xx::file::ls(&*env::PITCHFORK_LOGS_DIR)?
            .into_iter()
            .filter(|d| !d.starts_with("."))
            .filter(|d| d.is_dir())
            .filter_map(|d| d.file_name().map(|f| f.to_string_lossy().to_string()))
            .filter(|n| names.is_empty() || names.contains(n))
            .map(|n| {
                Ok((
                    n.clone(),
                    env::PITCHFORK_LOGS_DIR
                        .join(&n)
                        .join(format!("{n}.log"))
                        .canonicalize()?,
                ))
            })
            .filter_ok(|(_, f)| f.exists())
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut log_file_sizes = log_files
            .iter()
            .map(|(name, path)| {
                let size = fs::metadata(path).unwrap().len();
                (path.clone(), size)
            })
            .collect::<HashMap<_, _>>();

        let log_lines = log_files.iter().flat_map(|(name, path)| {
            let rev = match xx::file::open(path) {
                Ok(f) => rev_lines::RevLines::new(f),
                Err(e) => {
                    error!("{}: {}", path.display(), e);
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
        });
        let log_lines = if self.n == 0 {
            log_lines.collect_vec()
        } else {
            log_lines.take(self.n).collect_vec()
        };
        let log_lines = log_lines
            .into_iter()
            .sorted_by_cached_key(|l| l.0.to_string())
            .collect_vec();

        for (date, name, msg) in log_lines {
            println!("{} {} {}", date, name, msg);
        }

        if self.tail {
            let h = tokio::runtime::Handle::current();
            let (tx, mut rx) = tokio::sync::mpsc::channel(1);
            let mut debouncer = new_debouncer(
                Duration::from_millis(10),
                move |res: DebounceEventResult| {
                    let tx = tx.clone();
                    h.spawn(async move {
                        if let Ok(ev) = res {
                            for path in ev.into_iter().map(|e| e.path) {
                                tx.send(path).await.unwrap();
                            }
                        }
                    });
                },
            )?;

            for (_name, path) in &log_files {
                debouncer
                    .watcher()
                    .watch(path, RecursiveMode::NonRecursive)?;
            }

            while let Some(path) = rx.recv().await {
                let mut f = fs::File::open(&path)?;
                let name = log_files.iter().find(|(_, p)| **p == path).unwrap().0;
                let mut existing_size = *log_file_sizes.get(&path).unwrap();
                f.seek(SeekFrom::Start(existing_size))?;
                let lines = BufReader::new(f)
                    .lines()
                    .filter_map(Result::ok)
                    .collect_vec();
                existing_size += lines.iter().fold(0, |acc, l| acc + l.len() as u64);
                let lines = merge_log_lines(name, lines);
                for (date, name, msg) in lines {
                    println!("{} {} {}", date, name, msg);
                }
                log_file_sizes.insert(path, existing_size);
            }
        }

        Ok(())
    }
}

fn merge_log_lines(name: &str, lines: Vec<String>) -> Vec<(String, String, String)> {
    lines.into_iter().fold(vec![], |mut acc, line| {
        match regex!(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) (.*)$").captures(&line) {
            Some(caps) => {
                let date = caps.get(1).unwrap().as_str().to_string();
                let msg = caps.get(2).unwrap().as_str().to_string();
                acc.push((date, name.to_string(), msg));
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
