//! Custom diagnostic error types for rich error reporting via miette.
//!
//! This module provides structured error types that leverage miette's diagnostic
//! features including error codes, help text, source code highlighting, and suggestions.

// False positive: fields are used in #[error] format strings and miette derive macros
#![allow(unused_assignments)]

use miette::{Diagnostic, NamedSource, SourceSpan};
use std::io;
use std::path::PathBuf;
use thiserror::Error;

/// Errors related to daemon ID validation.
#[derive(Debug, Error, Diagnostic)]
pub enum DaemonIdError {
    #[error("daemon ID cannot be empty")]
    #[diagnostic(
        code(pitchfork::daemon::empty_id),
        url("https://pitchfork.jdx.dev/configuration"),
        help("provide a non-empty identifier for the daemon")
    )]
    Empty,

    #[error("daemon ID '{id}' contains path separator '{sep}'")]
    #[diagnostic(
        code(pitchfork::daemon::path_separator),
        url("https://pitchfork.jdx.dev/configuration"),
        help("daemon IDs cannot contain '/' or '\\' to prevent path traversal")
    )]
    PathSeparator { id: String, sep: char },

    #[error("daemon ID '{id}' contains parent directory reference '..'")]
    #[diagnostic(
        code(pitchfork::daemon::parent_dir_ref),
        url("https://pitchfork.jdx.dev/configuration"),
        help("daemon IDs cannot contain '..' to prevent path traversal")
    )]
    ParentDirRef { id: String },

    #[error("daemon ID '{id}' contains spaces")]
    #[diagnostic(
        code(pitchfork::daemon::contains_space),
        url("https://pitchfork.jdx.dev/configuration"),
        help("use hyphens or underscores instead of spaces (e.g., 'my-daemon' or 'my_daemon')")
    )]
    ContainsSpace { id: String },

    #[error("daemon ID cannot be '.'")]
    #[diagnostic(
        code(pitchfork::daemon::current_dir),
        url("https://pitchfork.jdx.dev/configuration"),
        help("'.' refers to the current directory; use a descriptive name instead")
    )]
    CurrentDir,

    #[error("daemon ID '{id}' contains non-printable or non-ASCII character")]
    #[diagnostic(
        code(pitchfork::daemon::invalid_chars),
        url("https://pitchfork.jdx.dev/configuration"),
        help(
            "daemon IDs must contain only printable ASCII characters (letters, numbers, hyphens, underscores, dots)"
        )
    )]
    InvalidChars { id: String },
}

/// Errors related to daemon operations.
#[derive(Debug, Error, Diagnostic)]
pub enum DaemonError {
    #[error("failed to stop daemon '{id}': {error}")]
    #[diagnostic(
        code(pitchfork::daemon::stop_failed),
        help("the process may be stuck or require manual intervention. Try: kill -9 <pid>")
    )]
    StopFailed { id: String, error: String },
}

/// Errors related to dependency resolution.
#[derive(Debug, Error, Diagnostic)]
pub enum DependencyError {
    #[error("daemon '{name}' not found in configuration")]
    #[diagnostic(
        code(pitchfork::deps::not_found),
        url("https://pitchfork.jdx.dev/configuration#depends")
    )]
    DaemonNotFound {
        name: String,
        #[help]
        suggestion: Option<String>,
    },

    #[error("daemon '{daemon}' depends on '{dependency}' which is not defined")]
    #[diagnostic(
        code(pitchfork::deps::missing_dependency),
        url("https://pitchfork.jdx.dev/configuration#depends"),
        help("add the missing daemon to your pitchfork.toml or remove it from the depends list")
    )]
    MissingDependency { daemon: String, dependency: String },

    #[error("circular dependency detected involving: {}", involved.join(", "))]
    #[diagnostic(
        code(pitchfork::deps::circular),
        url("https://pitchfork.jdx.dev/configuration#depends"),
        help("break the cycle by removing one of the dependencies")
    )]
    CircularDependency {
        /// The daemons involved in the cycle
        involved: Vec<String>,
    },
}

/// Error for TOML configuration parse failures with source code highlighting.
#[derive(Debug, Error, Diagnostic)]
#[error("failed to parse configuration")]
#[diagnostic(code(pitchfork::config::parse_error))]
pub struct ConfigParseError {
    /// The source file contents for display
    #[source_code]
    pub src: NamedSource<String>,

    /// The location of the error in the source
    #[label("{message}")]
    pub span: SourceSpan,

    /// The error message from the TOML parser
    pub message: String,

    /// Additional help text
    #[help]
    pub help: Option<String>,
}

impl ConfigParseError {
    /// Create a new ConfigParseError from a toml parse error
    pub fn from_toml_error(path: &std::path::Path, contents: String, err: toml::de::Error) -> Self {
        let message = err.message().to_string();

        // Try to get span information from the TOML error
        let span = err
            .span()
            .map(|r| SourceSpan::from(r.start..r.end))
            .unwrap_or_else(|| SourceSpan::from(0..0));

        Self {
            src: NamedSource::new(path.display().to_string(), contents),
            span,
            message,
            help: Some("check TOML syntax at https://toml.io".to_string()),
        }
    }
}

/// Errors related to file operations (config and state files).
#[derive(Debug, Error, Diagnostic)]
pub enum FileError {
    #[error("failed to read file: {}", path.display())]
    #[diagnostic(code(pitchfork::file::read_error))]
    ReadError {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to write file: {}", path.display())]
    #[diagnostic(code(pitchfork::file::write_error))]
    WriteError {
        path: PathBuf,
        #[help]
        details: Option<String>,
    },

    #[error("failed to serialize data for file: {}", path.display())]
    #[diagnostic(
        code(pitchfork::file::serialize_error),
        help("this is likely an internal error; please report it")
    )]
    SerializeError {
        path: PathBuf,
        #[source]
        source: toml::ser::Error,
    },

    #[error("no file path specified")]
    #[diagnostic(
        code(pitchfork::file::no_path),
        help("ensure a pitchfork.toml file exists in your project or specify a path")
    )]
    NoPath,
}

/// Errors related to IPC communication with the supervisor.
#[derive(Debug, Error, Diagnostic)]
pub enum IpcError {
    #[error("failed to connect to supervisor after {attempts} attempts")]
    #[diagnostic(
        code(pitchfork::ipc::connection_failed),
        url("https://pitchfork.jdx.dev/supervisor")
    )]
    ConnectionFailed {
        attempts: u32,
        #[source]
        source: Option<io::Error>,
        #[help]
        help: String,
    },

    #[error("IPC request timed out after {seconds}s")]
    #[diagnostic(
        code(pitchfork::ipc::timeout),
        url("https://pitchfork.jdx.dev/supervisor"),
        help(
            "the supervisor may be unresponsive or overloaded.\nCheck supervisor status: pitchfork supervisor status\nView logs: pitchfork logs"
        )
    )]
    Timeout { seconds: u64 },

    #[error("IPC connection closed unexpectedly")]
    #[diagnostic(
        code(pitchfork::ipc::connection_closed),
        url("https://pitchfork.jdx.dev/supervisor"),
        help(
            "the supervisor may have crashed or been stopped.\nRestart with: pitchfork supervisor start"
        )
    )]
    ConnectionClosed,

    #[error("failed to read IPC response")]
    #[diagnostic(code(pitchfork::ipc::read_failed))]
    ReadFailed {
        #[source]
        source: io::Error,
    },

    #[error("failed to send IPC request")]
    #[diagnostic(code(pitchfork::ipc::send_failed))]
    SendFailed {
        #[source]
        source: io::Error,
    },

    #[error("unexpected response from supervisor: expected {expected}, got {actual}")]
    #[diagnostic(
        code(pitchfork::ipc::unexpected_response),
        help("this may indicate a version mismatch between the CLI and supervisor")
    )]
    UnexpectedResponse { expected: String, actual: String },

    #[error("IPC message is invalid: {reason}")]
    #[diagnostic(code(pitchfork::ipc::invalid_message))]
    InvalidMessage { reason: String },
}

/// A collection of multiple errors that occurred during validation or processing.
///
/// This is useful when you want to collect and report all validation errors at once
/// instead of failing on the first error.
#[derive(Debug, Error, Diagnostic)]
#[error("multiple errors occurred ({} total)", errors.len())]
#[diagnostic(code(pitchfork::multiple_errors))]
#[allow(dead_code)]
pub struct MultipleErrors {
    #[related]
    pub errors: Vec<Box<dyn Diagnostic + Send + Sync + 'static>>,
}

#[allow(dead_code)]
impl MultipleErrors {
    /// Create a new MultipleErrors from a vector of diagnostics
    pub fn new(errors: Vec<Box<dyn Diagnostic + Send + Sync + 'static>>) -> Self {
        Self { errors }
    }

    /// Returns true if there are no errors
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns the number of errors
    pub fn len(&self) -> usize {
        self.errors.len()
    }
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
        .map(|(candidate, _)| format!("did you mean '{candidate}'?"))
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
        let err = FileError::ReadError {
            path: PathBuf::from("/path/to/config.toml"),
            source: io::Error::new(io::ErrorKind::NotFound, "file not found"),
        };
        assert!(err.to_string().contains("failed to read file"));
        assert!(err.to_string().contains("config.toml"));

        let err = FileError::NoPath;
        assert!(err.to_string().contains("no file path"));
    }

    #[test]
    fn test_ipc_error_display() {
        let err = IpcError::ConnectionFailed {
            attempts: 5,
            source: None,
            help: "ensure the supervisor is running".to_string(),
        };
        assert!(err.to_string().contains("failed to connect"));
        assert!(err.to_string().contains("5 attempts"));

        let err = IpcError::Timeout { seconds: 30 };
        assert!(err.to_string().contains("timed out"));
        assert!(err.to_string().contains("30s"));

        let err = IpcError::UnexpectedResponse {
            expected: "Ok".to_string(),
            actual: "Error".to_string(),
        };
        assert!(err.to_string().contains("unexpected response"));
        assert!(err.to_string().contains("Ok"));
        assert!(err.to_string().contains("Error"));
    }

    #[test]
    fn test_config_parse_error() {
        let contents = "[daemons.test]\nrun = ".to_string();
        let err = toml::from_str::<toml::Value>(&contents).unwrap_err();
        let parse_err =
            ConfigParseError::from_toml_error(std::path::Path::new("test.toml"), contents, err);

        assert!(parse_err.to_string().contains("failed to parse"));
    }

    #[test]
    fn test_multiple_errors() {
        let errors: Vec<Box<dyn Diagnostic + Send + Sync>> = vec![
            Box::new(DaemonIdError::Empty),
            Box::new(DaemonIdError::CurrentDir),
        ];
        let multi = MultipleErrors::new(errors);

        assert_eq!(multi.len(), 2);
        assert!(!multi.is_empty());
        assert!(multi.to_string().contains("2 total"));
    }
}
