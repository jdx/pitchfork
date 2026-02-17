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
    /// # Examples
    ///
    /// ```
    /// use pitchfork_cli::daemon_id::DaemonId;
    ///
    /// let id = DaemonId::new("global", "api");
    /// ```
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    /// Creates a new DaemonId with validation.
    ///
    /// Returns an error if either the namespace or name is invalid.
    #[allow(dead_code)]
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

    /// Parses a daemon ID string, using a default namespace if not qualified.
    ///
    /// # Examples
    ///
    /// ```
    /// use pitchfork_cli::daemon_id::DaemonId;
    ///
    /// // Qualified ID
    /// let id = DaemonId::parse_or_default("project-a/api", "global").unwrap();
    /// assert_eq!(id.namespace(), "project-a");
    ///
    /// // Unqualified ID uses default namespace
    /// let id = DaemonId::parse_or_default("api", "global").unwrap();
    /// assert_eq!(id.namespace(), "global");
    /// ```
    pub fn parse_or_default(s: &str, default_namespace: &str) -> Result<Self> {
        if s.contains('/') {
            Self::parse(s)
        } else {
            validate_component(s, "name")?;
            Ok(Self::new(default_namespace, s))
        }
    }

    /// Creates a DaemonId from a filesystem-safe path component.
    ///
    /// Converts `namespace--name` format back to a DaemonId.
    /// Uses `rsplit_once` to split at the LAST `--` occurrence, which handles
    /// the case where directory names might contain `--` (though this is discouraged).
    ///
    /// # Examples
    ///
    /// ```
    /// use pitchfork_cli::daemon_id::DaemonId;
    ///
    /// let id = DaemonId::from_safe_path("project-a--api").unwrap();
    /// assert_eq!(id.qualified(), "project-a/api");
    ///
    /// // Handles directory names with -- (edge case)
    /// let id = DaemonId::from_safe_path("my--project--api").unwrap();
    /// assert_eq!(id.namespace(), "my--project");
    /// assert_eq!(id.name(), "api");
    /// ```
    pub fn from_safe_path(s: &str) -> Result<Self> {
        // Use rsplit_once to split at the LAST "--" occurrence.
        // This handles edge cases where the namespace (derived from directory names)
        // might contain "--", even though this is discouraged.
        // The daemon name itself cannot contain "--" due to validation.
        if let Some((ns, name)) = s.rsplit_once("--") {
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
        Self {
            namespace: "global".to_string(),
            name: "pitchfork".to_string(),
        }
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

    /// Returns the display name, optionally hiding the namespace if unambiguous.
    ///
    /// # Arguments
    /// * `all_ids` - Iterator over all daemon IDs to check for conflicts
    ///
    /// # Examples
    ///
    /// If there's another daemon with the same name, returns the full qualified ID.
    /// Otherwise, returns just the name.
    #[allow(dead_code)]
    pub fn display_name<'a, I>(&self, all_ids: I) -> String
    where
        I: Iterator<Item = &'a DaemonId>,
    {
        let count = all_ids.filter(|other| other.name == self.name).count();
        if count > 1 {
            self.qualified()
        } else {
            self.name.clone()
        }
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

    /// Returns the qualified format with dim namespace for stderr.
    ///
    /// Format: `<dim>namespace</dim>/name`
    #[allow(dead_code)]
    pub fn styled_qualified_stderr(&self) -> String {
        use crate::ui::style::edim;
        format!("{}/{}", edim(&self.namespace), self.name)
    }

    /// Returns HTML for displaying the daemon ID with dimmed namespace.
    ///
    /// The namespace is wrapped in a span with class "daemon-ns" for CSS styling.
    /// Both namespace and name are HTML-escaped to prevent XSS attacks.
    #[allow(dead_code)]
    pub fn html_display(&self) -> String {
        fn escape_html(input: &str) -> String {
            let mut escaped = String::with_capacity(input.len());
            for ch in input.chars() {
                match ch {
                    '&' => escaped.push_str("&amp;"),
                    '<' => escaped.push_str("&lt;"),
                    '>' => escaped.push_str("&gt;"),
                    '"' => escaped.push_str("&quot;"),
                    '\'' => escaped.push_str("&#x27;"),
                    _ => escaped.push(ch),
                }
            }
            escaped
        }
        format!(
            r#"<span class="daemon-ns">{}</span>/{}"#,
            escape_html(&self.namespace),
            escape_html(&self.name),
        )
    }
}

impl Default for DaemonId {
    /// Returns a placeholder DaemonId.
    ///
    /// This is primarily used for struct defaults where the ID will be overwritten.
    /// Uses "global/_placeholder" to clearly indicate it's a placeholder value.
    fn default() -> Self {
        Self {
            namespace: "global".to_string(),
            name: "_placeholder".to_string(),
        }
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
    if !s.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
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
    fn test_daemon_id_parse_or_default() {
        // Qualified ID
        let id = DaemonId::parse_or_default("project-a/api", "global").unwrap();
        assert_eq!(id.namespace(), "project-a");
        assert_eq!(id.name(), "api");

        // Unqualified ID uses default namespace
        let id = DaemonId::parse_or_default("api", "global").unwrap();
        assert_eq!(id.namespace(), "global");
        assert_eq!(id.name(), "api");
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
    fn test_daemon_id_display_name() {
        let id1 = DaemonId::new("project-a", "api");
        let id2 = DaemonId::new("project-b", "api");
        let id3 = DaemonId::new("global", "worker");

        let all_ids = vec![&id1, &id2, &id3];

        // Conflict on "api" - show full qualified ID
        assert_eq!(id1.display_name(all_ids.iter().copied()), "project-a/api");
        assert_eq!(id2.display_name(all_ids.iter().copied()), "project-b/api");

        // No conflict on "worker" - show short name
        assert_eq!(id3.display_name(all_ids.iter().copied()), "worker");
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

    // Edge case tests for from_safe_path using rsplit_once
    #[test]
    fn test_from_safe_path_with_double_dash_in_namespace() {
        // This tests the fix for ambiguous from_safe_path parsing.
        // Directory names might contain "--" (e.g., "my--project"), and we need
        // to correctly handle this edge case by splitting at the LAST "--".
        let id = DaemonId::from_safe_path("my--project--api").unwrap();
        assert_eq!(id.namespace(), "my--project");
        assert_eq!(id.name(), "api");

        // Verify roundtrip works (namespace with -- is preserved)
        // Note: This doesn't fully roundtrip because the namespace contains "--"
        // which is sanitized in namespace_from_path(), but from_safe_path should
        // handle the input correctly.
        let safe = id.safe_path();
        assert_eq!(safe, "my--project--api");
        let recovered = DaemonId::from_safe_path(&safe).unwrap();
        assert_eq!(recovered, id);
    }

    #[test]
    fn test_from_safe_path_multiple_double_dashes() {
        // Multiple "--" sequences - should split at the LAST one
        let id = DaemonId::from_safe_path("a--b--c--daemon").unwrap();
        assert_eq!(id.namespace(), "a--b--c");
        assert_eq!(id.name(), "daemon");
    }

    #[test]
    fn test_from_safe_path_only_one_double_dash() {
        // Standard case - single "--" separator
        let id = DaemonId::from_safe_path("global--api").unwrap();
        assert_eq!(id.namespace(), "global");
        assert_eq!(id.name(), "api");
    }

    #[test]
    fn test_from_safe_path_no_separator() {
        // No "--" at all - should fail
        assert!(DaemonId::from_safe_path("globalapi").is_err());
        assert!(DaemonId::from_safe_path("api").is_err());
    }

    #[test]
    fn test_from_safe_path_empty_parts() {
        // Empty namespace (starts with --)
        let result = DaemonId::from_safe_path("--api");
        // rsplit_once("--") returns ("", "api"), so namespace is empty string
        // This should succeed with empty namespace (no validation in from_safe_path)
        assert!(result.is_ok());
        assert_eq!(result.unwrap().namespace(), "");

        // Empty name (ends with --)
        let result = DaemonId::from_safe_path("namespace--");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "");
    }

    // Default trait tests
    #[test]
    fn test_daemon_id_default() {
        let id = DaemonId::default();
        assert_eq!(id.namespace(), "global");
        assert_eq!(id.name(), "_placeholder");
        assert_eq!(id.qualified(), "global/_placeholder");
    }

    // Cross-namespace dependency parsing tests
    #[test]
    fn test_parse_cross_namespace_dependency() {
        // Can parse fully qualified dependency reference
        let id = DaemonId::parse("other-project/postgres").unwrap();
        assert_eq!(id.namespace(), "other-project");
        assert_eq!(id.name(), "postgres");
    }

    #[test]
    fn test_parse_or_default_preserves_explicit_namespace() {
        // When a full namespace is provided, use it instead of default
        let id = DaemonId::parse_or_default("external/db", "my-project").unwrap();
        assert_eq!(id.namespace(), "external");
        assert_eq!(id.name(), "db");
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
