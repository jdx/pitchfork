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

    pub fn unwatch(&mut self, path: &Path) -> Result<()> {
        self.debouncer.unwatch(path).into_diagnostic()
    }
}

/// Normalize a path by attempting to canonicalize it. If that fails, it attempts
/// to resolve it as an absolute path. This helps ensure that different relative
/// paths to the same directory are deduplicated.
fn normalize_watch_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            crate::env::CWD.join(path)
        }
    })
}

/// Expand glob patterns to actual file paths.
/// Patterns are resolved relative to base_dir.
/// Returns unique directories that need to be watched.
pub fn expand_watch_patterns(patterns: &[String], base_dir: &Path) -> Result<HashSet<PathBuf>> {
    let mut dirs_to_watch = HashSet::new();

    for pattern in patterns {
        // Strip leading "./" from patterns to handle relative path prefixes
        let normalized_pattern = pattern.strip_prefix("./").unwrap_or(pattern);

        // Make the pattern absolute by joining with base_dir
        let full_pattern = if Path::new(normalized_pattern).is_absolute() {
            normalize_path_for_glob(normalized_pattern)
        } else {
            normalize_path_for_glob(&base_dir.join(normalized_pattern).to_string_lossy())
        };

        // Expand the glob pattern
        match glob(&full_pattern) {
            Ok(paths) => {
                for entry in paths.flatten() {
                    // Watch the parent directory of each matched file
                    // This allows us to detect new files that match the pattern
                    if let Some(parent) = entry.parent() {
                        dirs_to_watch.insert(normalize_watch_path(parent));
                    }
                }
            }
            Err(e) => {
                log::warn!("Invalid glob pattern '{pattern}': {e}");
            }
        }

        // For patterns with wildcards, watch the base directory (before the wildcard)
        // For non-wildcard patterns, watch the parent directory of the specific file
        // This ensures we catch new files even if they don't exist at startup
        if normalized_pattern.contains('*') {
            // Find the first directory without wildcards
            // Normalize to use forward slashes for cross-platform compatibility
            let normalized_pattern_str = normalize_path_for_glob(normalized_pattern);
            let parts: Vec<&str> = normalized_pattern_str.split('/').collect();
            let mut base = base_dir.to_path_buf();
            for part in parts {
                if part.contains('*') {
                    break;
                }
                base = base.join(part);
            }
            // Watch the base directory if it exists, otherwise fall back to base_dir
            // This ensures we can detect when the directory is created
            let dir_to_watch = if base.is_dir() {
                base
            } else {
                base_dir.to_path_buf()
            };
            dirs_to_watch.insert(normalize_watch_path(&dir_to_watch));
        } else {
            // Non-wildcard pattern (specific file like "package.json")
            // Always watch the parent directory, even if file doesn't exist yet
            let full_path = if Path::new(normalized_pattern).is_absolute() {
                PathBuf::from(normalized_pattern)
            } else {
                base_dir.join(normalized_pattern)
            };
            if let Some(parent) = full_path.parent() {
                // Watch the parent if it exists (or base_dir as fallback)
                let dir_to_watch = if parent.is_dir() {
                    parent.to_path_buf()
                } else {
                    base_dir.to_path_buf()
                };
                dirs_to_watch.insert(normalize_watch_path(&dir_to_watch));
            }
        }
    }

    Ok(dirs_to_watch)
}

/// Normalize a path string to use forward slashes for glob pattern matching.
/// This ensures consistent behavior across Windows and Unix platforms.
fn normalize_path_for_glob(path: &str) -> String {
    path.replace('\\', "/")
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
        let path = PathBuf::from("/nonexistent/path/to/dir");

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

    #[test]
    fn test_expand_watch_patterns_specific_file() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();

        // Create a test file
        let test_file = base_dir.join("package.json");
        fs::write(&test_file, "{}").unwrap();

        // Expand pattern for a specific file
        let patterns = vec!["package.json".to_string()];
        let dirs = expand_watch_patterns(&patterns, base_dir).unwrap();

        // Should watch the parent directory
        assert_eq!(dirs.len(), 1);
        let dir = dirs.iter().next().unwrap();
        assert!(dir.is_absolute());
    }

    #[test]
    fn test_expand_watch_patterns_glob() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();
        let subdir = base_dir.join("src");
        fs::create_dir(&subdir).unwrap();

        // Create test files in src directory
        fs::write(subdir.join("file1.rs"), "").unwrap();
        fs::write(subdir.join("file2.rs"), "").unwrap();

        // Expand glob pattern
        let patterns = vec!["src/**/*.rs".to_string()];
        let dirs = expand_watch_patterns(&patterns, base_dir).unwrap();

        // Should watch the src directory
        assert!(!dirs.is_empty());
        for dir in &dirs {
            assert!(dir.is_absolute());
        }
    }

    #[test]
    fn test_expand_watch_patterns_nonexistent_file() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();

        // Pattern for a file that doesn't exist yet
        let patterns = vec!["config.toml".to_string()];
        let dirs = expand_watch_patterns(&patterns, base_dir).unwrap();

        // Should still watch the parent directory (base_dir in this case)
        assert_eq!(dirs.len(), 1);
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
        let patterns = vec!["./config.json".to_string()];
        let dirs = expand_watch_patterns(&patterns, base_dir).unwrap();

        // Should watch the parent directory
        assert_eq!(dirs.len(), 1);
        let dir = dirs.iter().next().unwrap();
        assert!(dir.is_absolute());
    }
}
