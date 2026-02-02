//! Structured daemon ID type that separates namespace and name.
//!
//! This module provides a type-safe representation of daemon IDs that
//! eliminates the need for repeated parsing and formatting operations.

use crate::Result;
use crate::error::DaemonIdError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{self, Display};
use std::hash::Hash;

/// A structured daemon identifier consisting of a namespace and a name.
///
/// All daemons have a namespace - global daemons use "global" as their namespace.
/// This type eliminates the need to repeatedly parse and format daemon IDs.
///
/// # Formats
///
/// - **Qualified format**: `namespace/name` (e.g., `project-a/api`, `global/web`)
/// - **Safe path format**: `namespace--name` (for filesystem paths)
///
/// # Examples
///
/// ```
/// use pitchfork_cli::daemon_id::DaemonId;
///
/// let id = DaemonId::new("project-a", "api");
/// assert_eq!(id.namespace(), "project-a");
/// assert_eq!(id.name(), "api");
/// assert_eq!(id.qualified(), "project-a/api");
/// assert_eq!(id.safe_path(), "project-a--api");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DaemonId {
    namespace: String,
    name: String,
}

impl DaemonId {
    /// Creates a new DaemonId from namespace and name.
    ///
    /// # Panics
    ///
    /// Panics if either the namespace or name is invalid (contains invalid characters,
    /// is empty, contains `--`, etc.). Use `try_new()` for a non-panicking version.
    ///
    /// # Examples
    ///
    /// ```
    /// use pitchfork_cli::daemon_id::DaemonId;
    ///
    /// let id = DaemonId::new("global", "api");
    /// ```
    #[cfg(test)]
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        let namespace = namespace.into();
        let name = name.into();

        // Validate inputs - panic on invalid values
        if let Err(e) = validate_component(&namespace, "namespace") {
            panic!("Invalid namespace '{}': {}", namespace, e);
        }
        if let Err(e) = validate_component(&name, "name") {
            panic!("Invalid name '{}': {}", name, e);
        }

        Self { namespace, name }
    }

    /// Creates a new DaemonId without validation.
    ///
    /// # Safety
    ///
    /// This function does not validate the inputs. Use it only when you are certain
    /// the namespace and name are valid (e.g., when reading from a trusted source
    /// like a parsed safe_path with "--" in the namespace component).
    ///
    /// For user-provided input, use `new()` or `try_new()` instead.
    pub(crate) fn new_unchecked(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    /// Creates a new DaemonId with validation.
    ///
    /// Returns an error if either the namespace or name is invalid.
    pub fn try_new(namespace: impl Into<String>, name: impl Into<String>) -> Result<Self> {
        let namespace = namespace.into();
        let name = name.into();

        validate_component(&namespace, "namespace")?;
        validate_component(&name, "name")?;

        Ok(Self { namespace, name })
    }

    /// Parses a qualified daemon ID string into a DaemonId.
    ///
    /// The input must be in the format `namespace/name`.
    ///
    /// # Examples
    ///
    /// ```
    /// use pitchfork_cli::daemon_id::DaemonId;
    ///
    /// let id = DaemonId::parse("project-a/api").unwrap();
    /// assert_eq!(id.namespace(), "project-a");
    /// assert_eq!(id.name(), "api");
    /// ```
    pub fn parse(s: &str) -> Result<Self> {
        validate_qualified_id(s)?;

        if let Some((ns, name)) = s.split_once('/') {
            Ok(Self {
                namespace: ns.to_string(),
                name: name.to_string(),
            })
        } else {
            Err(DaemonIdError::MissingNamespace { id: s.to_string() }.into())
        }
    }

    /// Creates a DaemonId from a filesystem-safe path component.
    ///
    /// Converts `namespace--name` format back to a DaemonId.
    /// Both components are validated with the same rules as `try_new()`,
    /// ensuring that the result can always be serialized and deserialized
    /// through the qualified (`namespace/name`) format without error.
    ///
    /// # Examples
    ///
    /// ```
    /// use pitchfork_cli::daemon_id::DaemonId;
    ///
    /// let id = DaemonId::from_safe_path("project-a--api").unwrap();
    /// assert_eq!(id.qualified(), "project-a/api");
    /// assert_eq!(DaemonId::parse(&id.qualified()).unwrap(), id);
    ///
    /// // Empty namespace or name fails validation
    /// assert!(DaemonId::from_safe_path("--api").is_err());
    /// assert!(DaemonId::from_safe_path("namespace--").is_err());
    /// // Namespace containing "--" is rejected to preserve roundtrip
    /// assert!(DaemonId::from_safe_path("my--project--api").is_err());
    /// ```
    pub fn from_safe_path(s: &str) -> Result<Self> {
        if let Some((ns, name)) = s.split_once("--") {
            // Validate both components with the same rules as try_new().
            // This guarantees that qualified() output can always be re-parsed,
            // preserving the Serialize <-> Deserialize roundtrip contract.
            validate_component(ns, "namespace")?;
            validate_component(name, "name")?;
            Ok(Self {
                namespace: ns.to_string(),
                name: name.to_string(),
            })
        } else {
            Err(DaemonIdError::InvalidSafePath {
                path: s.to_string(),
            }
            .into())
        }
    }

    /// Returns the namespace portion of the daemon ID.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Returns a DaemonId for the pitchfork supervisor itself.
    ///
    /// This is a convenience method to avoid repeated `DaemonId::new("global", "pitchfork")` calls.
    pub fn pitchfork() -> Self {
        // Use new_unchecked for this constant value to avoid redundant validation
        Self::new_unchecked("global", "pitchfork")
    }

    /// Returns the name (short ID) portion of the daemon ID.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the qualified format: `namespace/name`.
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    /// Returns the filesystem-safe format: `namespace--name`.
    pub fn safe_path(&self) -> String {
        format!("{}--{}", self.namespace, self.name)
    }

    /// Returns the main log file path for this daemon.
    pub fn log_path(&self) -> std::path::PathBuf {
        let safe = self.safe_path();
        crate::env::PITCHFORK_LOGS_DIR
            .join(&safe)
            .join(format!("{safe}.log"))
    }

    /// Returns a styled display name for terminal output (stdout).
    ///
    /// The namespace part is displayed in dim color, followed by `/` and the name.
    /// If `all_ids` is provided and the name is unique, only the name is shown.
    pub fn styled_display_name<'a, I>(&self, all_ids: Option<I>) -> String
    where
        I: Iterator<Item = &'a DaemonId>,
    {
        let show_full = match all_ids {
            Some(ids) => ids.filter(|other| other.name == self.name).count() > 1,
            None => true,
        };

        if show_full {
            self.styled_qualified()
        } else {
            self.name.clone()
        }
    }

    /// Returns the qualified format with dim namespace for terminal output (stdout).
    ///
    /// Format: `<dim>namespace</dim>/name`
    pub fn styled_qualified(&self) -> String {
        use crate::ui::style::ndim;
        format!("{}/{}", ndim(&self.namespace), self.name)
    }
}

impl Display for DaemonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.namespace, self.name)
    }
}

// NOTE: AsRef<str> and Borrow<str> implementations were intentionally removed.
// The Borrow trait has a contract that if T: Borrow<U>, then T's Hash/Eq/Ord
// must be consistent with U's. DaemonId derives Hash and Eq on both namespace
// and name, so implementing Borrow<str> would violate this contract and cause
// HashMap/HashSet lookups via &str to silently break due to hash mismatches.

/// Serialize as qualified string "namespace/name"
impl Serialize for DaemonId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.qualified())
    }
}

/// Deserialize from qualified string "namespace/name"
impl<'de> Deserialize<'de> for DaemonId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        DaemonId::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// JSON Schema implementation for DaemonId
/// Represents daemon ID as a string in "namespace/name" format
impl schemars::JsonSchema for DaemonId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DaemonId".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::DaemonId").into()
    }

    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "Daemon ID in the format 'namespace/name'",
            "pattern": r"^[\w.-]+/[\w.-]+$"
        })
    }
}

/// Validates a single component (namespace or name) of a daemon ID.
fn validate_component(s: &str, component_name: &str) -> Result<()> {
    if s.is_empty() {
        return Err(DaemonIdError::EmptyComponent {
            component: component_name.to_string(),
        }
        .into());
    }
    if s.contains('/') {
        return Err(DaemonIdError::PathSeparator {
            id: s.to_string(),
            sep: '/',
        }
        .into());
    }
    if s.contains('\\') {
        return Err(DaemonIdError::PathSeparator {
            id: s.to_string(),
            sep: '\\',
        }
        .into());
    }
    if s.contains("..") {
        return Err(DaemonIdError::ParentDirRef { id: s.to_string() }.into());
    }
    if s.contains("--") {
        return Err(DaemonIdError::ReservedSequence { id: s.to_string() }.into());
    }
    if s.contains(' ') {
        return Err(DaemonIdError::ContainsSpace { id: s.to_string() }.into());
    }
    if s == "." {
        return Err(DaemonIdError::CurrentDir.into());
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(DaemonIdError::InvalidChars { id: s.to_string() }.into());
    }
    Ok(())
}

/// Validates a qualified daemon ID string.
fn validate_qualified_id(s: &str) -> Result<()> {
    if s.is_empty() {
        return Err(DaemonIdError::Empty.into());
    }
    if s.contains('\\') {
        return Err(DaemonIdError::PathSeparator {
            id: s.to_string(),
            sep: '\\',
        }
        .into());
    }
    if s.contains(' ') {
        return Err(DaemonIdError::ContainsSpace { id: s.to_string() }.into());
    }
    if !s.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
        return Err(DaemonIdError::InvalidChars { id: s.to_string() }.into());
    }

    // Check slash count
    let slash_count = s.chars().filter(|&c| c == '/').count();
    if slash_count == 0 {
        return Err(DaemonIdError::MissingNamespace { id: s.to_string() }.into());
    }
    if slash_count > 1 {
        return Err(DaemonIdError::PathSeparator {
            id: s.to_string(),
            sep: '/',
        }
        .into());
    }

    // Check both parts are non-empty
    let (ns, name) = s.split_once('/').unwrap();
    if ns.is_empty() || name.is_empty() {
        return Err(DaemonIdError::PathSeparator {
            id: s.to_string(),
            sep: '/',
        }
        .into());
    }

    // Validate each component individually
    // This ensures parse("./api") fails just like try_new(".", "api")
    validate_component(ns, "namespace")?;
    validate_component(name, "name")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_id_new() {
        let id = DaemonId::new("global", "api");
        assert_eq!(id.namespace(), "global");
        assert_eq!(id.name(), "api");
        assert_eq!(id.qualified(), "global/api");
        assert_eq!(id.safe_path(), "global--api");
    }

    #[test]
    fn test_daemon_id_parse() {
        let id = DaemonId::parse("project-a/api").unwrap();
        assert_eq!(id.namespace(), "project-a");
        assert_eq!(id.name(), "api");

        // Missing namespace should fail
        assert!(DaemonId::parse("api").is_err());

        // Empty parts should fail
        assert!(DaemonId::parse("/api").is_err());
        assert!(DaemonId::parse("project/").is_err());

        // Multiple slashes should fail
        assert!(DaemonId::parse("a/b/c").is_err());
    }

    #[test]
    fn test_daemon_id_from_safe_path() {
        let id = DaemonId::from_safe_path("project-a--api").unwrap();
        assert_eq!(id.namespace(), "project-a");
        assert_eq!(id.name(), "api");

        // No separator should fail
        assert!(DaemonId::from_safe_path("projectapi").is_err());
    }

    #[test]
    fn test_daemon_id_roundtrip() {
        let original = DaemonId::new("my-project", "my-daemon");
        let safe = original.safe_path();
        let recovered = DaemonId::from_safe_path(&safe).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_daemon_id_display() {
        let id = DaemonId::new("global", "api");
        assert_eq!(format!("{}", id), "global/api");
    }

    #[test]
    fn test_daemon_id_serialize() {
        let id = DaemonId::new("global", "api");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"global/api\"");

        let deserialized: DaemonId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_daemon_id_validation() {
        // Valid IDs
        assert!(DaemonId::try_new("global", "api").is_ok());
        assert!(DaemonId::try_new("my-project", "my-daemon").is_ok());
        assert!(DaemonId::try_new("project_a", "daemon_1").is_ok());

        // Invalid - contains reserved sequences
        assert!(DaemonId::try_new("my--project", "api").is_err());
        assert!(DaemonId::try_new("project", "my--daemon").is_err());

        // Invalid - contains path separators
        assert!(DaemonId::try_new("my/project", "api").is_err());
        assert!(DaemonId::try_new("project", "my/daemon").is_err());

        // Invalid - empty
        assert!(DaemonId::try_new("", "api").is_err());
        assert!(DaemonId::try_new("project", "").is_err());
    }

    #[test]
    fn test_daemon_id_styled_display_name() {
        let id1 = DaemonId::new("project-a", "api");
        let id2 = DaemonId::new("project-b", "api");
        let id3 = DaemonId::new("global", "worker");

        let all_ids = [&id1, &id2, &id3];

        // "api" is ambiguous → full qualified ID must appear in the output
        let out1 = id1.styled_display_name(Some(all_ids.iter().copied()));
        let out2 = id2.styled_display_name(Some(all_ids.iter().copied()));
        assert!(
            out1.contains("project-a") && out1.contains("api"),
            "ambiguous id1 should show namespace: {out1}"
        );
        assert!(
            out2.contains("project-b") && out2.contains("api"),
            "ambiguous id2 should show namespace: {out2}"
        );

        // "worker" is unique → only the short name
        let out3 = id3.styled_display_name(Some(all_ids.iter().copied()));
        assert_eq!(out3, "worker", "unique id3 should show only short name");
    }

    #[test]
    fn test_daemon_id_ordering() {
        let id1 = DaemonId::new("a", "x");
        let id2 = DaemonId::new("a", "y");
        let id3 = DaemonId::new("b", "x");

        assert!(id1 < id2);
        assert!(id2 < id3);
        assert!(id1 < id3);
    }

    // Edge case tests for from_safe_path
    #[test]
    fn test_from_safe_path_double_dash_in_namespace_rejected() {
        // Namespaces containing "--" are rejected to preserve the Serialize <->
        // Deserialize roundtrip: qualified() output must always be re-parseable.
        // namespace_from_path() already sanitizes "--" -> "-" before reaching here.
        assert!(DaemonId::from_safe_path("my--project--api").is_err());
        assert!(DaemonId::from_safe_path("a--b--c--daemon").is_err());
    }

    #[test]
    fn test_from_safe_path_roundtrip_via_qualified() {
        // Standard case - single "--" separator, full roundtrip via qualified()
        let id = DaemonId::from_safe_path("global--api").unwrap();
        assert_eq!(id.namespace(), "global");
        assert_eq!(id.name(), "api");
        // Must roundtrip through qualified format (Serialize <-> Deserialize)
        let recovered = DaemonId::parse(&id.qualified()).unwrap();
        assert_eq!(recovered, id);
    }

    #[test]
    fn test_from_safe_path_no_separator() {
        // No "--" at all - should fail
        assert!(DaemonId::from_safe_path("globalapi").is_err());
        assert!(DaemonId::from_safe_path("api").is_err());
    }

    #[test]
    fn test_from_safe_path_empty_parts() {
        // Empty namespace (starts with --) - should fail validation
        let result = DaemonId::from_safe_path("--api");
        assert!(result.is_err());

        // Empty name (ends with --) - should fail validation
        let result = DaemonId::from_safe_path("namespace--");
        assert!(result.is_err());
    }

    // Cross-namespace dependency parsing tests
    #[test]
    fn test_parse_cross_namespace_dependency() {
        // Can parse fully qualified dependency reference
        let id = DaemonId::parse("other-project/postgres").unwrap();
        assert_eq!(id.namespace(), "other-project");
        assert_eq!(id.name(), "postgres");
    }

    // Test for directory names containing -- (namespace sanitization)
    #[test]
    fn test_directory_with_double_dash_in_name() {
        // Directory names like "my--project" are invalid for try_new because -- is reserved
        let result = DaemonId::try_new("my--project", "api");
        assert!(result.is_err());

        // from_safe_path also rejects "--" in namespace to preserve Serialize <->
        // Deserialize roundtrip. namespace_from_path() sanitizes "--" to "-" before
        // writing to the filesystem, so this case never arises in practice.
        let result = DaemonId::from_safe_path("my--project--api");
        assert!(
            result.is_err(),
            "from_safe_path must reject '--' in namespace to guarantee roundtrip via qualified()"
        );
    }

    #[test]
    fn test_parse_dot_namespace_rejected() {
        // parse("./api") should fail because "." is invalid as namespace
        // This ensures consistency with try_new(".", "api") which also fails
        let result = DaemonId::parse("./api");
        assert!(result.is_err());

        // Also test ".." as namespace
        let result = DaemonId::parse("../api");
        assert!(result.is_err());
    }

    // Serialization roundtrip tests
    #[test]
    fn test_daemon_id_toml_roundtrip() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        struct TestConfig {
            daemon_id: DaemonId,
        }

        let config = TestConfig {
            daemon_id: DaemonId::new("my-project", "api"),
        };

        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("daemon_id = \"my-project/api\""));

        let recovered: TestConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config, recovered);
    }

    #[test]
    fn test_daemon_id_json_roundtrip_in_map() {
        use std::collections::HashMap;

        let mut map: HashMap<String, DaemonId> = HashMap::new();
        map.insert("primary".to_string(), DaemonId::new("global", "api"));
        map.insert("secondary".to_string(), DaemonId::new("project", "worker"));

        let json = serde_json::to_string(&map).unwrap();
        let recovered: HashMap<String, DaemonId> = serde_json::from_str(&json).unwrap();
        assert_eq!(map, recovered);
    }

    // Pitchfork special ID test
    #[test]
    fn test_pitchfork_id() {
        let id = DaemonId::pitchfork();
        assert_eq!(id.namespace(), "global");
        assert_eq!(id.name(), "pitchfork");
        assert_eq!(id.qualified(), "global/pitchfork");
    }

    // Unicode and special character tests
    #[test]
    fn test_daemon_id_rejects_unicode() {
        assert!(DaemonId::try_new("プロジェクト", "api").is_err());
        assert!(DaemonId::try_new("project", "工作者").is_err());
    }

    #[test]
    fn test_daemon_id_rejects_control_chars() {
        assert!(DaemonId::try_new("project\x00", "api").is_err());
        assert!(DaemonId::try_new("project", "api\x1b").is_err());
    }

    #[test]
    fn test_daemon_id_rejects_spaces() {
        assert!(DaemonId::try_new("my project", "api").is_err());
        assert!(DaemonId::try_new("project", "my api").is_err());
        assert!(DaemonId::parse("my project/api").is_err());
    }

    #[test]
    fn test_daemon_id_rejects_chars_outside_schema_pattern() {
        // Schema only allows [A-Za-z0-9_.-] for each component.
        assert!(DaemonId::try_new("project+alpha", "api").is_err());
        assert!(DaemonId::try_new("project", "api@v1").is_err());
    }

    #[test]
    fn test_daemon_id_rejects_parent_dir_traversal() {
        assert!(DaemonId::try_new("project", "..").is_err());
        assert!(DaemonId::try_new("..", "api").is_err());
        assert!(DaemonId::parse("../api").is_err());
        assert!(DaemonId::parse("project/..").is_err());
    }

    #[test]
    fn test_daemon_id_rejects_current_dir() {
        assert!(DaemonId::try_new(".", "api").is_err());
        assert!(DaemonId::try_new("project", ".").is_err());
    }

    // Hash and equality tests for HashMap usage
    #[test]
    fn test_daemon_id_hash_consistency() {
        use std::collections::HashSet;

        let id1 = DaemonId::new("project", "api");
        let id2 = DaemonId::new("project", "api");
        let id3 = DaemonId::parse("project/api").unwrap();

        let mut set = HashSet::new();
        set.insert(id1.clone());

        // Same ID constructed differently should be found
        assert!(set.contains(&id2));
        assert!(set.contains(&id3));

        // Verify they're all equal
        assert_eq!(id1, id2);
        assert_eq!(id2, id3);
    }
}
