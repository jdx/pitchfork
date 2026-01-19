//! Custom diagnostic error types for rich error reporting via miette.
//!
//! This module provides structured error types that leverage miette's diagnostic
//! features including error codes, help text, and suggestions.

// False positive: fields are used in #[error] format strings via thiserror derive
#![allow(unused_assignments)]

use miette::Diagnostic;
use thiserror::Error;

/// Errors related to daemon ID validation.
#[derive(Debug, Error, Diagnostic)]
pub enum DaemonIdError {
    #[error("daemon ID cannot be empty")]
    #[diagnostic(
        code(pitchfork::daemon::empty_id),
        help("provide a non-empty identifier for the daemon")
    )]
    Empty,

    #[error("daemon ID '{id}' contains path separator '{sep}'")]
    #[diagnostic(
        code(pitchfork::daemon::path_separator),
        help("daemon IDs cannot contain '/' or '\\' to prevent path traversal")
    )]
    PathSeparator { id: String, sep: char },

    #[error("daemon ID '{id}' contains parent directory reference '..'")]
    #[diagnostic(
        code(pitchfork::daemon::parent_dir_ref),
        help("daemon IDs cannot contain '..' to prevent path traversal")
    )]
    ParentDirRef { id: String },

    #[error("daemon ID '{id}' contains spaces")]
    #[diagnostic(
        code(pitchfork::daemon::contains_space),
        help("use hyphens or underscores instead of spaces (e.g., 'my-daemon' or 'my_daemon')")
    )]
    ContainsSpace { id: String },

    #[error("daemon ID cannot be '.'")]
    #[diagnostic(
        code(pitchfork::daemon::current_dir),
        help("'.' refers to the current directory; use a descriptive name instead")
    )]
    CurrentDir,

    #[error("daemon ID '{id}' contains non-printable or non-ASCII character")]
    #[diagnostic(
        code(pitchfork::daemon::invalid_chars),
        help(
            "daemon IDs must contain only printable ASCII characters (letters, numbers, hyphens, underscores, dots)"
        )
    )]
    InvalidChars { id: String },
}

/// Errors related to dependency resolution.
#[derive(Debug, Error, Diagnostic)]
pub enum DependencyError {
    #[error("daemon '{name}' not found in configuration")]
    #[diagnostic(code(pitchfork::deps::not_found))]
    DaemonNotFound {
        name: String,
        #[help]
        suggestion: Option<String>,
    },

    #[error("daemon '{daemon}' depends on '{dependency}' which is not defined")]
    #[diagnostic(
        code(pitchfork::deps::missing_dependency),
        help("add the missing daemon to your pitchfork.toml or remove it from the depends list")
    )]
    MissingDependency { daemon: String, dependency: String },

    #[error("circular dependency detected involving: {}", involved.join(", "))]
    #[diagnostic(
        code(pitchfork::deps::circular),
        help("break the cycle by removing one of the dependencies")
    )]
    CircularDependency {
        /// The daemons involved in the cycle
        involved: Vec<String>,
    },
}

/// Errors related to file operations (config and state files).
#[derive(Debug, Error, Diagnostic)]
pub enum FileError {
    #[error("failed to parse file: {}", path.display())]
    #[diagnostic(code(pitchfork::file::parse_error))]
    ParseError {
        path: std::path::PathBuf,
        #[help]
        details: Option<String>,
    },

    #[error("failed to read file: {}", path.display())]
    #[diagnostic(code(pitchfork::file::read_error))]
    ReadError {
        path: std::path::PathBuf,
        #[help]
        details: Option<String>,
    },

    #[error("failed to write file: {}", path.display())]
    #[diagnostic(code(pitchfork::file::write_error))]
    WriteError {
        path: std::path::PathBuf,
        #[help]
        details: Option<String>,
    },

    #[error("no file path specified")]
    #[diagnostic(
        code(pitchfork::file::no_path),
        help("ensure a pitchfork.toml file exists in your project or specify a path")
    )]
    NoPath,
}

/// Find the most similar daemon name for suggestions.
pub fn find_similar_daemon<'a>(
    name: &str,
    available: impl Iterator<Item = &'a str>,
) -> Option<String> {
    use fuzzy_matcher::FuzzyMatcher;
    use fuzzy_matcher::skim::SkimMatcherV2;

    let matcher = SkimMatcherV2::default();
    available
        .filter_map(|candidate| {
            matcher
                .fuzzy_match(candidate, name)
                .map(|score| (candidate, score))
        })
        .max_by_key(|(_, score)| *score)
        .filter(|(_, score)| *score > 0)
        .map(|(candidate, _)| format!("did you mean '{}'?", candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_id_error_display() {
        let err = DaemonIdError::Empty;
        assert_eq!(err.to_string(), "daemon ID cannot be empty");

        let err = DaemonIdError::PathSeparator {
            id: "foo/bar".to_string(),
            sep: '/',
        };
        assert_eq!(
            err.to_string(),
            "daemon ID 'foo/bar' contains path separator '/'"
        );

        let err = DaemonIdError::ContainsSpace {
            id: "my app".to_string(),
        };
        assert_eq!(err.to_string(), "daemon ID 'my app' contains spaces");
    }

    #[test]
    fn test_dependency_error_display() {
        let err = DependencyError::DaemonNotFound {
            name: "postgres".to_string(),
            suggestion: None,
        };
        assert_eq!(
            err.to_string(),
            "daemon 'postgres' not found in configuration"
        );

        let err = DependencyError::MissingDependency {
            daemon: "api".to_string(),
            dependency: "db".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "daemon 'api' depends on 'db' which is not defined"
        );

        let err = DependencyError::CircularDependency {
            involved: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        };
        assert!(err.to_string().contains("circular dependency"));
        assert!(err.to_string().contains("a, b, c"));
    }

    #[test]
    fn test_find_similar_daemon() {
        let daemons = ["postgres", "redis", "api", "worker"];

        // Close match
        let suggestion = find_similar_daemon("postgre", daemons.iter().copied());
        assert_eq!(suggestion, Some("did you mean 'postgres'?".to_string()));

        // No reasonable match
        let suggestion = find_similar_daemon("xyz123", daemons.iter().copied());
        assert!(suggestion.is_none());
    }

    #[test]
    fn test_file_error_display() {
        let err = FileError::ParseError {
            path: std::path::PathBuf::from("/path/to/config.toml"),
            details: Some("invalid key".to_string()),
        };
        assert!(err.to_string().contains("failed to parse file"));
        assert!(err.to_string().contains("config.toml"));

        let err = FileError::NoPath;
        assert!(err.to_string().contains("no file path"));
    }
}
