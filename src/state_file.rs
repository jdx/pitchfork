use crate::daemon::Daemon;
use crate::daemon_id::DaemonId;
use crate::error::FileError;
use crate::{Result, env};
use once_cell::sync::Lazy;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateFile {
    #[serde(default)]
    pub daemons: BTreeMap<DaemonId, Daemon>,
    #[serde(default)]
    pub disabled: BTreeSet<DaemonId>,
    #[serde(default)]
    pub shell_dirs: BTreeMap<String, PathBuf>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
}

impl StateFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            daemons: Default::default(),
            disabled: Default::default(),
            shell_dirs: Default::default(),
            path,
        }
    }

    pub fn get() -> &'static Self {
        static STATE_FILE: Lazy<StateFile> = Lazy::new(|| {
            let path = &*env::PITCHFORK_STATE_FILE;
            StateFile::read(path).unwrap_or_else(|e| {
                error!(
                    "failed to read state file {}: {}. Falling back to in-memory empty state",
                    path.display(),
                    e
                );
                StateFile::new(path.to_path_buf())
            })
        });
        &STATE_FILE
    }

    pub fn read<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }
        let canonical_path = normalized_lock_path(path);
        let _lock = xx::fslock::get(&canonical_path, false)?;
        let raw = xx::file::read_to_string(path).unwrap_or_else(|e| {
            warn!("Error reading state file {path:?}: {e}");
            String::new()
        });

        // Try to parse directly (new format with qualified IDs)
        match toml::from_str::<Self>(&raw) {
            Ok(mut state_file) => {
                state_file.path = path.to_path_buf();
                for (id, daemon) in state_file.daemons.iter_mut() {
                    daemon.id = id.clone();
                }
                Ok(state_file)
            }
            Err(parse_err) => {
                if Self::looks_like_old_format(&raw) {
                    // Silent migration: attempt to rewrite bare keys as legacy/<name>
                    debug!(
                        "State file at {} appears to be in old format, attempting silent migration",
                        path.display()
                    );
                    match Self::migrate_old_format(&raw) {
                        Ok(migrated) => {
                            let mut state_file = migrated;
                            state_file.path = path.to_path_buf();
                            // Persist migrated state while we still hold the lock
                            if let Err(e) = state_file.write_unlocked() {
                                warn!("State file migration write failed: {e}");
                            }
                            debug!("State file migrated successfully");
                            return Ok(state_file);
                        }
                        Err(e) => {
                            error!(
                                "State file migration failed: {e}. \
                                 Raw content preserved at {}. Starting with empty state.",
                                path.display()
                            );
                            return Err(miette::miette!(
                                "Failed to migrate state file {}: {e}",
                                path.display()
                            ));
                        }
                    }
                }
                // New-format parse failure: do NOT silently discard state.
                Err(miette::miette!(
                    "Failed to parse state file {}: {parse_err}",
                    path.display()
                ))
            }
        }
    }

    /// Returns true if the TOML looks like the old state file format, i.e. the
    /// `daemons` table has at least one key that is missing the `namespace/`
    /// prefix.  Detection is done by parsing as a generic `toml::Value` so it
    /// works regardless of how the table headers are written.
    fn looks_like_old_format(raw: &str) -> bool {
        use toml::Value;
        let Ok(Value::Table(doc)) = toml::from_str::<Value>(raw) else {
            return false;
        };
        let Some(Value::Table(daemons)) = doc.get("daemons") else {
            return false;
        };
        // Old format: at least one daemon key has no '/'
        !daemons.is_empty() && daemons.keys().any(|k| !k.contains('/'))
    }

    /// Parse old-format state TOML (bare daemon names) and return a new-format
    /// `StateFile` with daemon IDs qualified under the `"legacy"` namespace.
    fn migrate_old_format(raw: &str) -> Result<Self> {
        use toml::Value;

        const LEGACY_NAMESPACE: &str = "legacy";

        // Parse as generic TOML value
        let mut doc: toml::map::Map<String, Value> = toml::from_str(raw)
            .map_err(|e| miette::miette!("failed to parse old state file: {e}"))?;

        // Re-key [daemons] entries: "name" -> "legacy/name"
        if let Some(Value::Table(daemons)) = doc.get_mut("daemons") {
            let old_keys: Vec<String> = daemons.keys().cloned().collect();
            for key in old_keys {
                if !key.contains('/')
                    && let Some(val) = daemons.remove(&key)
                {
                    let mut new_key = format!("{LEGACY_NAMESPACE}/{key}");
                    // Preserve data on collision by assigning a unique migrated key.
                    if daemons.contains_key(&new_key) {
                        let base = format!("{key}-legacy");
                        let mut candidate = format!("{LEGACY_NAMESPACE}/{base}");
                        let mut n: u32 = 2;
                        while daemons.contains_key(&candidate) {
                            candidate = format!("{LEGACY_NAMESPACE}/{base}-{n}");
                            n += 1;
                        }
                        warn!(
                            "Legacy daemon key '{}' collides with '{}'; migrating as '{}'",
                            key,
                            format_args!("{LEGACY_NAMESPACE}/{key}"),
                            candidate
                        );
                        new_key = candidate;
                    }
                    // Update the inner `id` field too
                    let val = if let Value::Table(mut tbl) = val {
                        tbl.insert("id".to_string(), Value::String(new_key.clone()));
                        Value::Table(tbl)
                    } else {
                        val
                    };
                    daemons.insert(new_key, val);
                }
            }
        }

        // Re-key [disabled] set entries the same way
        if let Some(Value::Array(disabled)) = doc.get_mut("disabled") {
            for entry in disabled.iter_mut() {
                if let Value::String(s) = entry
                    && !s.contains('/')
                {
                    *s = format!("{LEGACY_NAMESPACE}/{s}");
                }
            }
        }

        let new_raw =
            toml::to_string(&Value::Table(doc)).map_err(|e| FileError::SerializeError {
                path: PathBuf::new(),
                source: e,
            })?;

        let mut state_file: Self = toml::from_str(&new_raw)
            .map_err(|e| miette::miette!("failed to parse migrated state file: {e}"))?;
        // Sync inner daemon id fields
        for (id, daemon) in state_file.daemons.iter_mut() {
            daemon.id = id.clone();
        }
        Ok(state_file)
    }

    pub fn write(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FileError::WriteError {
                path: parent.to_path_buf(),
                details: Some(format!("failed to create state file directory: {e}")),
            })?;
        }
        let canonical_path = normalized_lock_path(&self.path);
        let _lock = xx::fslock::get(&canonical_path, false)?;
        self.write_unlocked()
    }

    /// Write the state file without acquiring the lock.
    /// Used internally when the lock is already held (e.g., during migration in read()).
    fn write_unlocked(&self) -> Result<()> {
        let raw = toml::to_string(self).map_err(|e| FileError::SerializeError {
            path: self.path.clone(),
            source: e,
        })?;

        // Use atomic write: write to temp file first, then rename
        // This prevents readers from seeing partially written content
        let temp_path = self.path.with_extension("toml.tmp");
        xx::file::write(&temp_path, &raw).map_err(|e| FileError::WriteError {
            path: temp_path.clone(),
            details: Some(e.to_string()),
        })?;
        std::fs::rename(&temp_path, &self.path).map_err(|e| FileError::WriteError {
            path: self.path.clone(),
            details: Some(format!("failed to rename temp file: {e}")),
        })?;
        Ok(())
    }
}

fn normalized_lock_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    if let Some(parent) = path.parent()
        && let Ok(canonical_parent) = parent.canonicalize()
        && let Some(file_name) = path.file_name()
    {
        return canonical_parent.join(file_name);
    }

    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_status::DaemonStatus;

    #[test]
    fn test_state_file_toml_roundtrip_stopped() {
        let mut state = StateFile::new(PathBuf::from("/tmp/test.toml"));
        let daemon_id = DaemonId::new("project", "test");
        state.daemons.insert(
            daemon_id.clone(),
            Daemon {
                id: daemon_id,
                title: None,
                pid: None,
                shell_pid: None,
                status: DaemonStatus::Stopped,
                dir: None,
                cmd: None,
                autostop: false,
                cron_schedule: None,
                cron_retrigger: None,
                last_cron_triggered: None,
                last_exit_success: Some(true),
                retry: 0,
                retry_count: 0,
                ready_delay: None,
                ready_output: None,
                ready_http: None,
                ready_port: None,
                ready_cmd: None,
                depends: vec![],
                env: None,
                watch: vec![],
                watch_base_dir: None,
            },
        );

        let toml_str = toml::to_string(&state).unwrap();
        println!("Serialized TOML:\n{}", toml_str);

        let parsed: StateFile = toml::from_str(&toml_str).expect("Failed to parse TOML");
        println!("Parsed: {:?}", parsed);

        assert!(
            parsed
                .daemons
                .contains_key(&DaemonId::new("project", "test"))
        );
    }

    #[test]
    fn test_looks_like_old_format_bare_names() {
        let old = r#"
[daemons.api]
id = "api"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
"#;
        assert!(StateFile::looks_like_old_format(old));
    }

    #[test]
    fn test_looks_like_old_format_new_format() {
        let new = r#"
    disabled = []

    [daemons."legacy/api"]
    id = "legacy/api"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
"#;
        assert!(!StateFile::looks_like_old_format(new));
    }

    #[test]
    fn test_looks_like_old_format_empty() {
        assert!(!StateFile::looks_like_old_format(""));
        assert!(!StateFile::looks_like_old_format("[shell_dirs]"));
    }

    #[test]
    fn test_migrate_old_format_basic() {
        let old = r#"
[daemons.api]
id = "api"
autostop = false
retry = 0
retry_count = 0
status = "stopped"

[daemons.worker]
id = "worker"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
last_exit_success = true
"#;
        let migrated = StateFile::migrate_old_format(old).expect("migration should succeed");
        assert!(
            migrated
                .daemons
                .contains_key(&DaemonId::new("legacy", "api")),
            "api should be migrated to legacy/api"
        );
        assert!(
            migrated
                .daemons
                .contains_key(&DaemonId::new("legacy", "worker")),
            "worker should be migrated to legacy/worker"
        );
        assert_eq!(migrated.daemons.len(), 2);
    }

    #[test]
    fn test_migrate_old_format_preserves_disabled() {
        let old = r#"
disabled = ["api", "worker"]

[daemons.api]
id = "api"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
"#;
        let migrated = StateFile::migrate_old_format(old).expect("migration should succeed");
        assert!(
            migrated.disabled.contains(&DaemonId::new("legacy", "api")),
            "disabled 'api' should become 'legacy/api'"
        );
        assert!(
            migrated
                .disabled
                .contains(&DaemonId::new("legacy", "worker")),
            "disabled 'worker' should become 'legacy/worker'"
        );
    }

    #[test]
    fn test_migrate_old_format_already_qualified_unchanged() {
        // If somehow a key already has a namespace, it should not be double-prefixed
        let mixed = r#"
[daemons.bare]
id = "bare"
autostop = false
retry = 0
retry_count = 0
status = "stopped"
"#;
        let migrated = StateFile::migrate_old_format(mixed).expect("migration should succeed");
        // "bare" -> "legacy/bare", not "legacy/legacy/bare"
        assert!(
            migrated
                .daemons
                .contains_key(&DaemonId::new("legacy", "bare")),
            "bare key should become legacy/bare"
        );
        // Should not have double-prefixed entry
        assert_eq!(migrated.daemons.len(), 1);
    }

    #[test]
    fn test_migrate_old_format_does_not_overwrite_existing_qualified_entry() {
        let mixed = r#"
[daemons.api]
id = "api"
cmd = ["echo", "old"]
autostop = false
retry = 0
retry_count = 0
status = "stopped"

[daemons."legacy/api"]
id = "legacy/api"
cmd = ["echo", "new"]
autostop = false
retry = 0
retry_count = 0
status = "stopped"
"#;

        let migrated = StateFile::migrate_old_format(mixed).expect("migration should succeed");
        let key = DaemonId::new("legacy", "api");
        let daemon = migrated.daemons.get(&key).expect("legacy/api should exist");

        let cmd = daemon.cmd.as_ref().expect("cmd should exist");
        assert_eq!(cmd, &vec!["echo".to_string(), "new".to_string()]);

        // Colliding bare key should be preserved under a unique migrated key.
        let preserved = DaemonId::new("legacy", "api-legacy");
        let preserved_daemon = migrated
            .daemons
            .get(&preserved)
            .expect("colliding bare key should be preserved as legacy/api-legacy");
        let preserved_cmd = preserved_daemon
            .cmd
            .as_ref()
            .expect("preserved cmd should exist");
        assert_eq!(preserved_cmd, &vec!["echo".to_string(), "old".to_string()]);
        assert_eq!(migrated.daemons.len(), 2);
    }
}
