use crate::Result;
use glob::glob;
use itertools::Itertools;
use miette::IntoDiagnostic;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, FileIdMap, new_debouncer_opt};
use std::collections::HashSet;
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
                            // Ignore send errors - receiver may be dropped during shutdown
                            let _ = tx.send(paths).await;
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

/// Expand glob patterns to actual file paths.
/// Patterns are resolved relative to base_dir.
/// Returns unique directories that need to be watched.
pub fn expand_watch_patterns(patterns: &[String], base_dir: &Path) -> Result<HashSet<PathBuf>> {
    let mut dirs_to_watch = HashSet::new();

    for pattern in patterns {
        // Make the pattern absolute by joining with base_dir
        let full_pattern = if Path::new(pattern).is_absolute() {
            pattern.clone()
        } else {
            base_dir.join(pattern).to_string_lossy().to_string()
        };

        // Expand the glob pattern
        match glob(&full_pattern) {
            Ok(paths) => {
                for entry in paths.flatten() {
                    // Watch the parent directory of each matched file
                    // This allows us to detect new files that match the pattern
                    if let Some(parent) = entry.parent() {
                        dirs_to_watch.insert(parent.to_path_buf());
                    }
                }
            }
            Err(e) => {
                log::warn!("Invalid glob pattern '{}': {}", pattern, e);
            }
        }

        // Also watch the base directories for patterns with wildcards
        // This ensures we catch new files in watched directories
        if pattern.contains('*') {
            // Find the first directory without wildcards
            let parts: Vec<&str> = pattern.split('/').collect();
            let mut base = base_dir.to_path_buf();
            for part in parts {
                if part.contains('*') {
                    break;
                }
                base = base.join(part);
            }
            if base.is_dir() {
                dirs_to_watch.insert(base);
            }
        }
    }

    Ok(dirs_to_watch)
}

/// Check if a changed path matches any of the watch patterns.
pub fn path_matches_patterns(changed_path: &Path, patterns: &[String], base_dir: &Path) -> bool {
    for pattern in patterns {
        let full_pattern = if Path::new(pattern).is_absolute() {
            pattern.clone()
        } else {
            base_dir.join(pattern).to_string_lossy().to_string()
        };

        if let Ok(glob_pattern) = glob::Pattern::new(&full_pattern)
            && glob_pattern.matches_path(changed_path)
        {
            return true;
        }
    }
    false
}
