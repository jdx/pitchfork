use crate::Result;
use async_watcher::notify::RecursiveMode;
use async_watcher::{
    notify::{self, RecommendedWatcher},
    AsyncDebouncer, DebouncedEvent,
};
use std::{path::Path, time::Duration};
use tokio::sync::mpsc::Receiver;

pub async fn async_debounce_watch<P: AsRef<Path>>(
    paths: Vec<(P, &str)>,
) -> Result<(
    Receiver<Result<Vec<DebouncedEvent>, Vec<notify::Error>>>,
    AsyncDebouncer<RecommendedWatcher>,
)> {
    let (tx, rx) = tokio::sync::mpsc::channel(100);

    let mut debouncer =
        AsyncDebouncer::new(Duration::from_secs(1), Some(Duration::from_secs(1)), tx).await?;

    // add the paths to the watcher
    paths.iter().for_each(|(p, rm)| {
        debouncer
            .watcher()
            .watch(
                p.as_ref(),
                if *rm == "nonrecursive" {
                    RecursiveMode::NonRecursive
                } else if *rm == "recursive" {
                    RecursiveMode::Recursive
                } else {
                    unreachable!("invalid RecursiveMode")
                },
            )
            .unwrap();
    });

    Ok((rx, debouncer))
}
