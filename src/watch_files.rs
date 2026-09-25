use crate::Result;
use crate::pitchfork_toml::WatchMode;
use globset::{GlobBuilder, GlobMatcher};
use itertools::Itertools;
use miette::IntoDiagnostic;
use notify::{Config, EventKind, PollWatcher, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, NoCache, new_debouncer_opt};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub struct WatchFiles {
    pub rx: tokio::sync::mpsc::Receiver<Vec<PathBuf>>,
    backend: WatchFilesBackend,
}

// `NoCache` rather than `FileIdMap`: only changed paths are needed, not rename
// tracking, and `FileIdMap` re-walks every recursive root on each rescan. On
// Linux that walk opens every directory, and the resulting inotify `IN_OPEN`
// events can overflow the event queue, which triggers another rescan and spins
// the watcher thread indefinitely on large trees.
enum WatchFilesBackend {
    Native(Debouncer<RecommendedWatcher, NoCache>),
    Poll(Debouncer<PollWatcher, NoCache>),
}

impl WatchFiles {
    pub fn new(duration: Duration, mode: WatchMode, poll_interval: Duration) -> Result<Self> {
        let h = tokio::runtime::Handle::current();
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let make_callback = |tx: tokio::sync::mpsc::Sender<Vec<PathBuf>>,
                             h: tokio::runtime::Handle| {
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
            }
        };

        let backend = match mode {
            WatchMode::Native => WatchFilesBackend::Native(
                new_debouncer_opt(
                    duration,
                    None,
                    make_callback(tx.clone(), h.clone()),
                    NoCache::new(),
                    Config::default(),
                )
                .into_diagnostic()?,
            ),
            WatchMode::Poll => WatchFilesBackend::Poll(
                new_debouncer_opt(
                    duration,
                    None,
                    make_callback(tx.clone(), h.clone()),
                    NoCache::new(),
                    Config::default().with_poll_interval(poll_interval),
                )
                .into_diagnostic()?,
            ),
            WatchMode::Auto => {
                return Err(miette::miette!(
                    "WatchMode::Auto must not be passed directly to WatchFiles::new; \
                     the caller must resolve auto to native or poll"
                ));
            }
        };

        Ok(Self { backend, rx })
    }

    pub fn watch(&mut self, path: &Path, recursive_mode: RecursiveMode) -> Result<()> {
        match &mut self.backend {
            WatchFilesBackend::Native(debouncer) => {
                debouncer.watch(path, recursive_mode).into_diagnostic()
            }
            WatchFilesBackend::Poll(debouncer) => {
                debouncer.watch(path, recursive_mode).into_diagnostic()
            }
        }
    }

    pub fn unwatch(&mut self, path: &Path) -> Result<()> {
        match &mut self.backend {
            WatchFilesBackend::Native(debouncer) => debouncer.unwatch(path).into_diagnostic(),
            WatchFilesBackend::Poll(debouncer) => debouncer.unwatch(path).into_diagnostic(),
        }
    }
}

/// Normalize a path by attempting to canonicalize it. If that fails, it attempts
/// to resolve it as an absolute path. This helps ensure that different relative
/// paths to the same directory are deduplicated.
///
/// On Windows, `std::fs::canonicalize()` returns paths with the `\\?\` (verbatim)
/// prefix. The `notify` crate's PollWatcher may not correctly report changes for
/// verbatim-prefixed paths, and the changed paths it reports would carry the
/// prefix, causing mismatches with non-canonicalized glob patterns. We strip
/// the prefix after canonicalization to keep paths consistent across the watcher
/// and the pattern matcher.
fn normalize_watch_path(path: &Path) -> PathBuf {
    match path.canonicalize() {
        Ok(p) => {
            #[cfg(windows)]
            {
                strip_verbatim_prefix(&p)
            }
            #[cfg(not(windows))]
            {
                p
            }
        }
        Err(_) => {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                crate::env::CWD.join(path)
            }
        }
    }
}

/// Strip the `\\?\` verbatim prefix from a Windows path.
/// `\\?\C:\dir` → `C:\dir`, `\\?\UNC\server\share` → `\\server\share`
#[cfg(windows)]
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if let Some(unc) = rest.strip_prefix(r"UNC\") {
            PathBuf::from(format!(r"\\{}", unc))
        } else {
            PathBuf::from(rest)
        }
    } else {
        path.to_path_buf()
    }
}

/// Expand watch patterns to the directories that must be watched to see every
/// change they can match, and how each directory must be watched.
/// Patterns are resolved relative to base_dir.
///
/// Recursive watches are only used where a `**` component requires one, since
/// they register a watch on every directory below the root. Otherwise each
/// directory level the pattern spans is watched non-recursively, which also
/// notices newly created directories that match; the supervisor re-expands the
/// patterns after every batch of events so new directories get watched.
pub fn expand_watch_patterns(
    patterns: &[String],
    base_dir: &Path,
) -> HashMap<PathBuf, RecursiveMode> {
    let mut targets = HashMap::new();
    for pattern in patterns {
        for (dir, mode) in watch_targets_for_pattern(pattern, base_dir) {
            insert_watch_target(&mut targets, normalize_watch_path(&dir), mode);
        }
    }
    targets
}

/// Add a watch target, upgrading an existing entry to recursive if needed.
pub fn insert_watch_target(
    targets: &mut HashMap<PathBuf, RecursiveMode>,
    dir: PathBuf,
    mode: RecursiveMode,
) {
    let entry = targets.entry(dir).or_insert(mode);
    if mode == RecursiveMode::Recursive {
        *entry = RecursiveMode::Recursive;
    }
}

fn watch_targets_for_pattern(pattern: &str, base_dir: &Path) -> Vec<(PathBuf, RecursiveMode)> {
    // Strip leading "./" from patterns to handle relative path prefixes
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    let full_path = base_dir.join(pattern);
    let mut components = full_path.components().collect_vec();
    let Some(file_part) = components.pop() else {
        return vec![];
    };

    // Directories before the first glob component are fixed.
    let mut dir_parts = components.into_iter().peekable();
    let mut literal_dir = PathBuf::new();
    while let Some(part) = dir_parts.next_if(|c| !is_glob_component(c)) {
        literal_dir.push(part);
    }
    if !literal_dir.is_dir() {
        // Watch the nearest existing ancestor so creating the missing
        // directory wakes the watcher, which then re-expands the pattern.
        return literal_dir
            .ancestors()
            .find(|p| p.is_dir())
            .map(|p| vec![(p.to_path_buf(), RecursiveMode::NonRecursive)])
            .unwrap_or_default();
    }

    let mut targets = vec![];
    let mut current = vec![literal_dir];
    for part in dir_parts {
        let part = part.as_os_str().to_string_lossy();
        if part.contains("**") {
            targets.extend(current.into_iter().map(|d| (d, RecursiveMode::Recursive)));
            return targets;
        }
        let Some(matcher) = component_matcher(&part, pattern) else {
            return targets;
        };
        // Watch this level too, so a newly created matching directory is seen.
        targets.extend(
            current
                .iter()
                .map(|d| (d.clone(), RecursiveMode::NonRecursive)),
        );
        current = current
            .iter()
            .flat_map(|d| matching_subdirs(d, &matcher))
            .collect();
    }

    let mode = if file_part.as_os_str().to_string_lossy().contains("**") {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    targets.extend(current.into_iter().map(|d| (d, mode)));
    targets
}

fn is_glob_component(component: &Component) -> bool {
    component
        .as_os_str()
        .to_string_lossy()
        .contains(['*', '?', '[', '{'])
}

/// Build a matcher for a single path component, with the same glob semantics
/// as `path_matches_patterns`.
fn component_matcher(component: &str, pattern: &str) -> Option<GlobMatcher> {
    match GlobBuilder::new(component)
        .case_insensitive(cfg!(target_os = "windows"))
        .literal_separator(true)
        .build()
    {
        Ok(glob) => Some(glob.compile_matcher()),
        Err(e) => {
            log::warn!("Invalid glob pattern '{pattern}': {e}");
            None
        }
    }
}

fn matching_subdirs(dir: &Path, matcher: &GlobMatcher) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| matcher.is_match(e.file_name()))
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// Normalize a path string to use forward slashes for glob pattern matching.
/// This ensures consistent behavior across Windows and Unix platforms.
///
/// On Windows, `std::fs::canonicalize()` returns paths with the `\\?\` prefix
/// (verbatim path). If we don't strip it, canonicalized watcher paths won't
/// match non-canonicalized glob patterns built from `env::CWD`, causing all
/// file-change matching to silently fail on Windows.
///
/// Verbatim UNC paths (`\\?\UNC\server\share`) are converted to the regular
/// UNC form (`//server/share`) so they match glob patterns consistently.
fn normalize_path_for_glob(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!("//{}", rest.replace('\\', "/"))
    } else {
        path.strip_prefix(r"\\?\")
            .unwrap_or(path)
            .replace('\\', "/")
    }
}

/// Check if a changed path matches any of the watch patterns.
/// Uses globset which properly supports ** for recursive directory matching.
pub fn path_matches_patterns(changed_path: &Path, patterns: &[String], base_dir: &Path) -> bool {
    // Normalize the changed path to use forward slashes for consistent matching
    let changed_path_str = normalize_path_for_glob(&changed_path.to_string_lossy());

    for pattern in patterns {
        // Strip leading "./" from patterns to handle relative path prefixes
        let normalized_pattern = pattern.strip_prefix("./").unwrap_or(pattern);

        // Build the full pattern and normalize to use forward slashes
        let full_pattern = if Path::new(normalized_pattern).is_absolute() {
            normalize_path_for_glob(normalized_pattern)
        } else {
            normalize_path_for_glob(&base_dir.join(normalized_pattern).to_string_lossy())
        };

        // Use globset which properly supports ** for recursive matching
        let glob = globset::GlobBuilder::new(&full_pattern)
            .case_insensitive(cfg!(target_os = "windows"))
            .literal_separator(true) // * doesn't match /, use ** for recursive
            .build();

        if let Ok(glob) = glob {
            let matcher = glob.compile_matcher();
            if matcher.is_match(&changed_path_str) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_normalize_watch_path_existing_directory() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path().join("test_dir");
        fs::create_dir(&dir_path).unwrap();

        // Canonicalize should work for existing directories
        let normalized = normalize_watch_path(&dir_path);
        assert!(normalized.is_absolute());
        assert!(normalized.exists());
    }

    #[test]
    fn test_normalize_watch_path_nonexistent_path() {
        // Use a platform-appropriate absolute path that doesn't exist.
        // On Windows, "/nonexistent/..." is not absolute (no drive letter),
        // so normalize_watch_path would prepend CWD instead of returning as-is.
        #[cfg(unix)]
        let path = PathBuf::from("/nonexistent/path/to/dir");
        #[cfg(windows)]
        let path = PathBuf::from(r"C:\nonexistent\path\to\dir");

        // Should return the original path when canonicalization fails
        let normalized = normalize_watch_path(&path);
        assert_eq!(normalized, path);
    }

    #[test]
    fn test_normalize_watch_path_deduplication() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path().join("test_dir");
        fs::create_dir(&dir_path).unwrap();

        // Create a subdirectory to test path traversal
        let subdir = dir_path.join("subdir");
        fs::create_dir(&subdir).unwrap();

        // Create two different relative paths pointing to the same directory
        // One is direct, the other uses parent/child traversal
        let path1 = subdir.clone();
        let path2 = subdir.join("..").join("subdir");

        let normalized1 = normalize_watch_path(&path1);
        let normalized2 = normalize_watch_path(&path2);

        // Both should canonicalize to the same path
        assert_eq!(normalized1, normalized2);
    }

    fn canon(path: &Path) -> PathBuf {
        normalize_watch_path(path)
    }

    fn expand(patterns: &[&str], base_dir: &Path) -> HashMap<PathBuf, RecursiveMode> {
        let patterns = patterns.iter().map(|p| p.to_string()).collect_vec();
        expand_watch_patterns(&patterns, base_dir)
    }

    #[test]
    fn test_expand_watch_patterns_specific_file() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        fs::write(base_dir.join("package.json"), "{}").unwrap();
        // A large sibling tree must not make the watch recursive
        fs::create_dir_all(base_dir.join("node_modules/a/b")).unwrap();

        let dirs = expand(&["package.json"], base_dir);

        assert_eq!(
            dirs,
            HashMap::from([(canon(base_dir), RecursiveMode::NonRecursive)])
        );
    }

    #[test]
    fn test_expand_watch_patterns_recursive_glob() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        let subdir = base_dir.join("src");
        fs::create_dir_all(subdir.join("nested")).unwrap();
        fs::write(subdir.join("file1.rs"), "").unwrap();
        fs::write(subdir.join("nested/file2.rs"), "").unwrap();

        let dirs = expand(&["src/**/*.rs"], base_dir);

        assert_eq!(
            dirs,
            HashMap::from([(canon(&subdir), RecursiveMode::Recursive)])
        );
    }

    #[test]
    fn test_expand_watch_patterns_single_level_glob() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        fs::create_dir_all(base_dir.join("config/nested")).unwrap();

        let dirs = expand(&["config/*.toml"], base_dir);

        assert_eq!(
            dirs,
            HashMap::from([(canon(&base_dir.join("config")), RecursiveMode::NonRecursive)])
        );
    }

    #[test]
    fn test_expand_watch_patterns_glob_directory_component() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        fs::create_dir_all(base_dir.join("crates/a/deep")).unwrap();
        fs::create_dir_all(base_dir.join("crates/b")).unwrap();
        fs::write(base_dir.join("crates/file.toml"), "").unwrap();

        let dirs = expand(&["crates/*/Cargo.toml"], base_dir);

        // The glob level is watched to notice new crates, plus each match
        assert_eq!(
            dirs,
            HashMap::from([
                (canon(&base_dir.join("crates")), RecursiveMode::NonRecursive),
                (
                    canon(&base_dir.join("crates/a")),
                    RecursiveMode::NonRecursive
                ),
                (
                    canon(&base_dir.join("crates/b")),
                    RecursiveMode::NonRecursive
                ),
            ])
        );
    }

    #[test]
    fn test_expand_watch_patterns_recursive_wins() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        fs::create_dir(base_dir.join("src")).unwrap();

        let dirs = expand(&["src/main.rs", "src/**/*.rs"], base_dir);

        assert_eq!(
            dirs,
            HashMap::from([(canon(&base_dir.join("src")), RecursiveMode::Recursive)])
        );
    }

    #[test]
    fn test_expand_watch_patterns_nonexistent_file() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();

        // Pattern for a file that doesn't exist yet
        let dirs = expand(&["config.toml"], base_dir);

        assert_eq!(
            dirs,
            HashMap::from([(canon(base_dir), RecursiveMode::NonRecursive)])
        );
    }

    #[test]
    fn test_expand_watch_patterns_nonexistent_directory() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        fs::create_dir(base_dir.join("config")).unwrap();

        // The nearest existing ancestor is watched until the directory appears
        let dirs = expand(&["config/app/*.toml", "lib/**/*.ts"], base_dir);

        assert_eq!(
            dirs,
            HashMap::from([
                (canon(base_dir), RecursiveMode::NonRecursive),
                (canon(&base_dir.join("config")), RecursiveMode::NonRecursive),
            ])
        );
    }

    #[test]
    fn test_path_matches_patterns_simple() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();

        // Create test files
        let test_txt = base_dir.join("test.txt");
        let test_rs = base_dir.join("test.rs");
        fs::write(&test_txt, "").unwrap();
        fs::write(&test_rs, "").unwrap();

        // Simple pattern match
        assert!(path_matches_patterns(
            &test_txt,
            &["*.txt".to_string()],
            base_dir
        ));

        // Non-matching pattern
        assert!(!path_matches_patterns(
            &test_rs,
            &["*.txt".to_string()],
            base_dir
        ));
    }

    #[test]
    fn test_path_matches_patterns_recursive_glob() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        let src_dir = base_dir.join("src");
        let deep_dir = src_dir.join("deep");
        fs::create_dir_all(&deep_dir).unwrap();

        // Create test files
        let deep_file = deep_dir.join("file.rs");
        let src_file = src_dir.join("file.rs");
        fs::write(&deep_file, "").unwrap();
        fs::write(&src_file, "").unwrap();

        // ** pattern should match any depth
        assert!(path_matches_patterns(
            &deep_file,
            &["src/**/*.rs".to_string()],
            base_dir
        ));

        // Should also match top-level
        assert!(path_matches_patterns(
            &src_file,
            &["src/**/*.rs".to_string()],
            base_dir
        ));
    }

    #[test]
    fn test_path_matches_patterns_multiple_patterns() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();

        // Create test files
        let cargo_toml = base_dir.join("Cargo.toml");
        let main_rs = base_dir.join("main.rs");
        let readme_md = base_dir.join("README.md");
        fs::write(&cargo_toml, "").unwrap();
        fs::write(&main_rs, "").unwrap();
        fs::write(&readme_md, "").unwrap();

        // Multiple patterns - should match if any pattern matches
        let patterns = vec!["*.rs".to_string(), "*.toml".to_string()];
        assert!(path_matches_patterns(&cargo_toml, &patterns, base_dir));
        assert!(path_matches_patterns(&main_rs, &patterns, base_dir));
        assert!(!path_matches_patterns(&readme_md, &patterns, base_dir));
    }

    #[test]
    fn test_path_matches_patterns_relative_prefix() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();

        // Create a test file
        let test_file = base_dir.join("config.json");
        fs::write(&test_file, "{}").unwrap();

        // Pattern with "./" prefix should match the file
        assert!(path_matches_patterns(
            &test_file,
            &["./config.json".to_string()],
            base_dir
        ));

        // Same pattern without prefix should also match
        assert!(path_matches_patterns(
            &test_file,
            &["config.json".to_string()],
            base_dir
        ));
    }

    #[test]
    fn test_expand_watch_patterns_relative_prefix() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();

        // Create a test file
        let test_file = base_dir.join("config.json");
        fs::write(&test_file, "{}").unwrap();

        // Pattern with "./" prefix should expand correctly
        let dirs = expand(&["./config.json"], base_dir);

        assert_eq!(
            dirs,
            HashMap::from([(canon(base_dir), RecursiveMode::NonRecursive)])
        );
    }
}
