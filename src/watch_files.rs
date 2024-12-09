use crate::Result;
use miette::IntoDiagnostic;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct WatchFiles {
    pub rx: tokio::sync::mpsc::Receiver<PathBuf>,
    debouncer: Debouncer<RecommendedWatcher>,
}

impl WatchFiles {
    pub fn new(duration: Duration) -> Result<Self> {
        let h = tokio::runtime::Handle::current();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let debouncer = new_debouncer(duration, move |res: DebounceEventResult| {
            let tx = tx.clone();
            h.spawn(async move {
                if let Ok(ev) = res {
                    for path in ev.into_iter().map(|e| e.path) {
                        tx.send(path).await.unwrap();
                    }
                }
            });
        })
        .into_diagnostic()?;

        Ok(Self { debouncer, rx })
    }

    pub fn watch(&mut self, path: &Path, recursive_mode: RecursiveMode) -> Result<()> {
        self.debouncer
            .watcher()
            .watch(path, recursive_mode)
            .into_diagnostic()
    }
}
