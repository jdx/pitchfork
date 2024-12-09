use crate::{env, Result};
use itertools::Itertools;
use std::collections::{BTreeMap, HashSet};
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
                (
                    n.clone(),
                    env::PITCHFORK_LOGS_DIR.join(&n).join(format!("{n}.log")),
                )
            })
            .filter(|(_, f)| f.exists())
            .collect::<BTreeMap<_, _>>();

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
            lines.into_iter().fold(vec![], |mut acc, line| {
                match regex!(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) (.*)$").captures(&line) {
                    Some(caps) => {
                        let date = caps.get(1).unwrap().as_str().to_string();
                        let msg = caps.get(2).unwrap().as_str().to_string();
                        acc.push((date, name, msg));
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
        Ok(())
    }
}
