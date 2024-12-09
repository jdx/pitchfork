use crate::{env, Result};
use itertools::Itertools;
use std::collections::{BTreeMap, HashSet};

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

        let log_lines = log_files
            .iter()
            .flat_map(|(name, path)| {
                let rev = match xx::file::open(path) {
                    Ok(f) => rev_lines::RevLines::new(f),
                    Err(e) => {
                        error!("{}: {}", path.display(), e);
                        return vec![];
                    }
                };
                let lines = rev.into_iter()
                    .filter_map(Result::ok)
                    .map(|l| (name, l));
                if self.n == 0 {
                    lines.collect()
                } else {
                    lines.take(self.n).collect()
                }
            })
            .rev()
            .collect_vec();

        dbg!(&log_lines);
        Ok(())
    }
}
