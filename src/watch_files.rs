use crate::Result;
use itertools::Itertools;
use miette::IntoDiagnostic;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, FileIdMap, new_debouncer_opt};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct WatchFiles {
    pub rx: tokio::sync::mpsc::Receiver<Vec<PathBuf>>,
    debouncer: Debouncer<RecommendedWatcher, FileIdMap>,
}

impl WatchFiles {
    pub fn new(duration: Duration) -> Result<Self> {
        let h = tokio::runtime::Handle::current();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let debouncer = new_debouncer_opt(
            duration,
            None,
            move |res: DebounceEventResult| {
                let tx = tx.clone();
                h.spawn(async move {
                    if let Ok(ev) = res {
                        let paths = ev
                            .into_iter()
                            .filter(|e| {
                                matches!(
                                    e.kind,
                                    EventKind::Modify(_)
                                        | EventKind::Create(_)
                                        | EventKind::Remove(_)
                                )
                            })
                            .flat_map(|e| e.paths.clone())
                            .unique()
                            .collect_vec();
                        if !paths.is_empty() {
                            tx.send(paths).await.unwrap();
                        }
                    }
                });
            },
            FileIdMap::new(),
            Config::default(),
        )
        .into_diagnostic()?;

        Ok(Self { debouncer, rx })
    }

    pub fn watch(&mut self, path: &Path, recursive_mode: RecursiveMode) -> Result<()> {
        self.debouncer.watch(path, recursive_mode).into_diagnostic()
    }
}
