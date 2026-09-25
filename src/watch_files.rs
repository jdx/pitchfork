use crate::Result;
use crate::pitchfork_toml::WatchMode;
use globset::{GlobBuilder, GlobMatcher};
use itertools::Itertools;
use miette::IntoDiagnostic;
use notify::event::ModifyKind;
use notify::{Config, EventKind, PollWatcher, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, NoCache, new_debouncer_opt};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// A debounced batch of file changes.
#[derive(Debug)]
pub struct WatchEvents {
    /// Every path that was created, modified, or removed.
    pub paths: Vec<PathBuf>,
    /// Paths that were created or moved in; their contents produce no events
    /// of their own.
    pub created: Vec<PathBuf>,
}

pub struct WatchFiles {
    pub rx: tokio::sync::mpsc::Receiver<WatchEvents>,
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
        let make_callback = |tx: tokio::sync::mpsc::Sender<WatchEvents>,
                             h: tokio::runtime::Handle| {
            move |res: DebounceEventResult| {
                let Ok(ev) = res else { return };
                let mut paths = vec![];
                let mut created = vec![];
                for e in ev.iter().filter(|e| {
                    matches!(
                        e.kind,
                        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                    )
                }) {
                    paths.extend(e.paths.iter().cloned());
                    if matches!(
                        e.kind,
                        EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
                    ) {
                        created.extend(e.paths.iter().cloned());
                    }
                }
                if paths.is_empty() {
                    return;
                }
                let events = WatchEvents {
                    paths: paths.into_iter().unique().collect(),
                    created: created.into_iter().unique().collect(),
                };
                let tx = tx.clone();
                h.spawn(async move {
                    // Ignore send errors - receiver may be dropped during shutdown
                    let _ = tx.send(events).await;
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

/// List the entries a watch on `dir` covers: its direct entries, or with
/// `Recursive` every entry below it. Symlinked directories are not followed.
pub fn watched_entries(dir: &Path, mode: RecursiveMode) -> Vec<PathBuf> {
    let mut paths = vec![];
    collect_entries(dir, mode, &mut paths);
    paths
}

fn collect_entries(dir: &Path, mode: RecursiveMode, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if mode == RecursiveMode::Recursive && entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect_entries(&path, mode, paths);
        }
        paths.push(path);
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
        for alt in relative_alternatives(pattern) {
            for (dir, mode) in watch_targets_for_pattern(&alt, base_dir) {
                insert_watch_target(&mut targets, normalize_watch_path(&dir), mode);
            }
        }
    }
    targets
}

/// Brace alternatives of `pattern`, kept relative to the base directory when
/// `pattern` is: `{a,}/**/*.rs` must not become `/**/*.rs` and watch `/`.
fn relative_alternatives(pattern: &str) -> Vec<String> {
    if Path::new(pattern).is_absolute() {
        return expand_braces(pattern);
    }
    expand_braces(pattern)
        .into_iter()
        .map(|alt| alt.trim_start_matches(['/', '\\']).to_string())
        // e.g. a drive-prefixed alternative on Windows
        .filter(|alt| !Path::new(alt).is_absolute())
        .collect()
}

/// Expand `{a,b}` alternatives into separate patterns, so each is watched
/// narrowly even when an alternative contains `/` (`{src/api,lib}/*.rs`).
/// Patterns with more alternatives than is reasonable are returned as is.
fn expand_braces(pattern: &str) -> Vec<String> {
    const MAX_ALTERNATIVES: usize = 64;
    let mut expanded = vec![];
    let mut pending = vec![pattern.to_string()];
    while let Some(p) = pending.pop() {
        match first_brace_group(&p) {
            Some((start, end, alternatives)) => {
                for alt in alternatives {
                    pending.push(format!("{}{alt}{}", &p[..start], &p[end + 1..]));
                }
            }
            None => expanded.push(p),
        }
        if expanded.len() + pending.len() > MAX_ALTERNATIVES {
            return vec![pattern.to_string()];
        }
    }
    expanded
}

/// Find the first top-level `{...}` group, returning the byte offsets of its
/// braces and its comma-separated alternatives.
fn first_brace_group(pattern: &str) -> Option<(usize, usize, Vec<&str>)> {
    let bytes = pattern.as_bytes();
    let (mut start, mut alt_start, mut depth) = (0, 0, 0);
    let mut in_class = false;
    let mut alternatives = vec![];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // globset treats `\` as an escape except on Windows
            b'\\' if cfg!(not(windows)) => i += 1,
            b']' if in_class => in_class = false,
            _ if in_class => {}
            b'[' => in_class = true,
            b'{' => {
                if depth == 0 {
                    start = i;
                    alt_start = i + 1;
                }
                depth += 1;
            }
            b',' if depth == 1 => {
                alternatives.push(&pattern[alt_start..i]);
                alt_start = i + 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    alternatives.push(&pattern[alt_start..i]);
                    return Some((start, i, alternatives));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
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
        // `**` is only recursive as a whole component; elsewhere it acts as `*`
        if part == "**" {
            targets.extend(current.into_iter().map(|d| (d, RecursiveMode::Recursive)));
            return targets;
        }
        let Some(matcher) = component_matcher(&part) else {
            let full_pattern = normalize_path_for_glob(&full_path.to_string_lossy());
            match GlobBuilder::new(&full_pattern).build() {
                // A brace or class containing `/` spans components and cannot
                // be expanded level by level, so watch all it could match.
                Ok(_) => targets.extend(current.into_iter().map(|d| (d, RecursiveMode::Recursive))),
                // An invalid pattern never matches, so it needs no watch.
                Err(e) => log::warn!("Invalid glob pattern '{pattern}': {e}"),
            }
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

    let mode = if file_part.as_os_str() == "**" {
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
fn component_matcher(component: &str) -> Option<GlobMatcher> {
    GlobBuilder::new(component)
        .case_insensitive(cfg!(target_os = "windows"))
        .literal_separator(true)
        .build()
        .ok()
        .map(|glob| glob.compile_matcher())
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
    fn test_expand_watch_patterns_embedded_double_star() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        fs::create_dir_all(base_dir.join("src/a")).unwrap();
        fs::create_dir_all(base_dir.join("src/b")).unwrap();

        // `**` inside a component matches like `*`, within one level
        let dirs = expand(&["src/foo**bar.rs", "sr**/x.rs"], base_dir);

        assert_eq!(
            dirs,
            HashMap::from([
                (canon(base_dir), RecursiveMode::NonRecursive),
                (canon(&base_dir.join("src")), RecursiveMode::NonRecursive),
            ])
        );
    }

    #[test]
    fn test_expand_watch_patterns_trailing_double_star() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        fs::create_dir(base_dir.join("src")).unwrap();

        let dirs = expand(&["src/**"], base_dir);

        assert_eq!(
            dirs,
            HashMap::from([(canon(&base_dir.join("src")), RecursiveMode::Recursive)])
        );
    }

    #[test]
    fn test_expand_watch_patterns_alternatives_with_separator() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        fs::create_dir_all(base_dir.join("src/api")).unwrap();
        fs::create_dir(base_dir.join("lib")).unwrap();

        // Each alternative is watched on its own, even across components
        let dirs = expand(&["{src/api,lib}/*.rs"], base_dir);
        assert_eq!(
            dirs,
            HashMap::from([
                (
                    canon(&base_dir.join("src/api")),
                    RecursiveMode::NonRecursive
                ),
                (canon(&base_dir.join("lib")), RecursiveMode::NonRecursive),
            ])
        );
        assert!(path_matches_patterns(
            &base_dir.join("src/api/main.rs"),
            &["{src/api,lib}/*.rs".to_string()],
            base_dir
        ));

        // A class spanning components can't be expanded level by level, so
        // everything below it is watched
        let dirs = expand(&["[a/b]/*.rs"], base_dir);
        assert_eq!(
            dirs,
            HashMap::from([(canon(base_dir), RecursiveMode::Recursive)])
        );

        // An invalid pattern never matches, so it is not watched
        assert!(expand(&["[z-a]/*.rs"], base_dir).is_empty());
    }

    #[test]
    fn test_expand_braces() {
        let mut expanded = expand_braces("{src/{a,b},lib}/*.{rs,toml}");
        expanded.sort();
        assert_eq!(
            expanded,
            [
                "lib/*.rs",
                "lib/*.toml",
                "src/a/*.rs",
                "src/a/*.toml",
                "src/b/*.rs",
                "src/b/*.toml",
            ]
        );

        // Braces inside a class or escaped are literal
        assert_eq!(expand_braces("[{]x}/*.rs"), ["[{]x}/*.rs"]);
        #[cfg(unix)]
        assert_eq!(expand_braces(r"\{a,b}.rs"), [r"\{a,b}.rs"]);

        // Too many alternatives are left for the recursive fallback
        let many = "{a,b,c,d,e}/{a,b,c,d,e}/{a,b,c}/*.rs";
        assert_eq!(expand_braces(many), [many]);
    }

    #[test]
    fn test_expand_watch_patterns_alternatives_stay_relative() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        fs::create_dir_all(base_dir.join("src/x")).unwrap();

        // An empty or `/`-leading alternative must not escape to `/`
        let dirs = expand(&["{src,}/**/*.rs", "{/src/x,lib}/*.rs"], base_dir);

        assert_eq!(
            dirs,
            HashMap::from([
                (canon(base_dir), RecursiveMode::Recursive),
                (canon(&base_dir.join("src")), RecursiveMode::Recursive),
                (canon(&base_dir.join("src/x")), RecursiveMode::NonRecursive),
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
    fn test_watched_entries() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path().join("new");
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("a.toml"), "").unwrap();
        fs::write(dir.join("nested/b.toml"), "").unwrap();

        let mut paths = watched_entries(&dir, RecursiveMode::NonRecursive);
        paths.sort();
        assert_eq!(paths, vec![dir.join("a.toml"), dir.join("nested")]);

        let mut paths = watched_entries(&dir, RecursiveMode::Recursive);
        paths.sort();
        assert_eq!(
            paths,
            vec![
                dir.join("a.toml"),
                dir.join("nested"),
                dir.join("nested/b.toml"),
            ]
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
