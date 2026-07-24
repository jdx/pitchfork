use crate::daemon::Daemon;
use crate::daemon_id::DaemonId;
use crate::daemon_status::DaemonStatus;
use crate::error::FileError;
use crate::{Result, env};
use once_cell::sync::Lazy;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct StateFile {
    #[serde(default)]
    pub daemons: BTreeMap<DaemonId, Daemon>,
    #[serde(default)]
    pub disabled: BTreeSet<DaemonId>,
    #[serde(default)]
    pub shell_dirs: BTreeMap<String, PathBuf>,
    /// Project sessions keyed by host PID (as string, matching `shell_dirs`)
    /// and then by canonical directory. `#[serde(default)]` keeps older
    /// state files (that predate project sessions) parseable.
    #[serde(default)]
    pub project_sessions: BTreeMap<String, BTreeMap<PathBuf, ProjectSession>>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
    #[serde(skip)]
    pub(crate) dirty: AtomicBool,
    /// Snapshot of the last written TOML content. Used by `write()` to skip
    /// redundant disk I/O when the serialized state hasn't changed.
    /// Guarded by the file lock in practice; `Mutex` is used only to satisfy
    /// `Sync` since `write` takes `&self`.
    #[serde(skip)]
    pub(crate) last_content: Mutex<Option<String>>,
}

/// A project session entry. The owning host PID and tracked directory live in
/// the nested map key, so the value only needs the liveness title snapshot
/// used to mitigate PID reuse.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ProjectSession {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub liveness_title: Option<String>,
}

impl StateFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            daemons: Default::default(),
            disabled: Default::default(),
            shell_dirs: Default::default(),
            project_sessions: Default::default(),
            path,
            dirty: AtomicBool::new(false),
            last_content: Mutex::new(None),
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
                state_file.dirty = AtomicBool::new(false);
                for (id, daemon) in state_file.daemons.iter_mut() {
                    daemon.id = id.clone();
                }
                // Seed last_content with the raw TOML so the first write() can
                // skip disk I/O when the state hasn't actually changed.
                state_file.last_content = Mutex::new(Some(raw));
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

    /// Mark the state file as dirty so the background flush task will
    /// persist it on the next tick.
    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Check whether the state file needs to be flushed.
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// Insert or replace a daemon entry and mark the state dirty.
    pub fn insert_daemon(&mut self, id: &DaemonId, daemon: Daemon) {
        self.daemons.insert(id.clone(), daemon);
        self.mark_dirty();
    }

    /// Remove a daemon entry and mark the state dirty if the daemon existed.
    pub fn remove_daemon(&mut self, id: &DaemonId) {
        if self.daemons.remove(id).is_some() {
            self.mark_dirty();
        }
    }

    /// Disable a daemon (add to disabled set) and mark the state dirty.
    /// Returns true if the daemon was not already disabled.
    pub fn disable_daemon(&mut self, id: &DaemonId) -> bool {
        let inserted = self.disabled.insert(id.clone());
        if inserted {
            self.mark_dirty();
        }
        inserted
    }

    /// Enable a daemon (remove from disabled set) and mark the state dirty.
    /// Returns true if the daemon was previously disabled.
    pub fn enable_daemon(&mut self, id: &DaemonId) -> bool {
        let removed = self.disabled.remove(id);
        if removed {
            self.mark_dirty();
        }
        removed
    }

    /// Set the active port for a daemon and mark the state dirty.
    /// Returns true if the daemon was found and updated.
    pub fn set_active_port(&mut self, id: &DaemonId, port: u16) -> bool {
        if let Some(d) = self.daemons.get_mut(id) {
            d.active_port = Some(port);
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    /// Update a daemon's status only while the state still belongs to that PID.
    pub fn set_daemon_status_if_owned(
        &mut self,
        id: &DaemonId,
        pid: u32,
        status: DaemonStatus,
    ) -> bool {
        let Some(daemon) = self.daemons.get_mut(id) else {
            return false;
        };
        if daemon.pid != Some(pid) {
            return false;
        }

        daemon.status = status;
        self.mark_dirty();
        true
    }

    /// Record a daemon process exit only if the state still belongs to that PID.
    ///
    /// A replacement process may be started before the old process monitor
    /// finishes its exit path. Checking and updating under the same state lock
    /// prevents the old monitor from overwriting the replacement's state.
    pub fn record_daemon_exit(
        &mut self,
        id: &DaemonId,
        pid: u32,
        status: DaemonStatus,
        last_exit_success: bool,
    ) -> bool {
        self.record_daemon_terminal_state(id, pid, status, Some(last_exit_success))
    }

    /// Record an explicit stop only if the state still belongs to that PID.
    ///
    /// `last_exit_success` is optional because the already-dead branch cannot
    /// infer a new outcome and must preserve the existing historical result.
    pub fn record_daemon_stop(
        &mut self,
        id: &DaemonId,
        pid: u32,
        last_exit_success: Option<bool>,
    ) -> bool {
        self.record_daemon_terminal_state(id, pid, DaemonStatus::Stopped, last_exit_success)
    }

    fn record_daemon_terminal_state(
        &mut self,
        id: &DaemonId,
        pid: u32,
        status: DaemonStatus,
        last_exit_success: Option<bool>,
    ) -> bool {
        let Some(daemon) = self.daemons.get_mut(id) else {
            return false;
        };
        if daemon.pid != Some(pid) {
            return false;
        }

        daemon.pid = None;
        daemon.status = status;
        if let Some(last_exit_success) = last_exit_success {
            daemon.last_exit_success = Some(last_exit_success);
        }
        daemon.active_port = None;
        self.mark_dirty();
        true
    }

    /// Update the last cron trigger time for a daemon and mark the state dirty.
    /// Returns true if the daemon was found and updated.
    pub fn set_last_cron_triggered(
        &mut self,
        id: &DaemonId,
        time: chrono::DateTime<chrono::Local>,
    ) -> bool {
        if let Some(d) = self.daemons.get_mut(id) {
            d.last_cron_triggered = Some(time);
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    /// Set a shell working directory and mark the state dirty.
    pub fn set_shell_dir(&mut self, shell_pid: u32, dir: PathBuf) {
        self.shell_dirs.insert(shell_pid.to_string(), dir);
        self.mark_dirty();
    }

    /// Remove a shell working directory and mark the state dirty.
    /// Returns true if the entry existed.
    pub fn remove_shell_dir(&mut self, shell_pid: u32) -> bool {
        let removed = self.shell_dirs.remove(&shell_pid.to_string()).is_some();
        if removed {
            self.mark_dirty();
        }
        removed
    }

    /// Insert or replace a project session for the given host PID and directory
    /// and mark the state dirty. Returns the previous session, if any.
    pub fn set_project_session(
        &mut self,
        pid: u32,
        dir: PathBuf,
        session: ProjectSession,
    ) -> Option<ProjectSession> {
        let inner = self.project_sessions.entry(pid.to_string()).or_default();
        let old = inner.insert(dir, session);
        self.mark_dirty();
        old
    }

    /// Remove a project session for the given host PID and directory and mark
    /// the state dirty if it existed. Returns the removed session, if any.
    /// Empty per-PID subtables are pruned so the persisted TOML stays tidy.
    pub fn remove_project_session(&mut self, pid: u32, dir: &Path) -> Option<ProjectSession> {
        let pid_str = pid.to_string();
        if let std::collections::btree_map::Entry::Occupied(mut entry) =
            self.project_sessions.entry(pid_str)
        {
            let removed = entry.get_mut().remove(dir);
            if removed.is_some() {
                if entry.get().is_empty() {
                    entry.remove();
                }
                self.mark_dirty();
            }
            removed
        } else {
            None
        }
    }

    /// Look up a project session for the given host PID and directory.
    pub fn get_project_session(&self, pid: u32, dir: &Path) -> Option<&ProjectSession> {
        self.project_sessions
            .get(&pid.to_string())
            .and_then(|inner| inner.get(dir))
    }

    /// Flat iterator over all project sessions yielding `(pid_str, dir, session)`
    /// for every entry. Used by the supervisor refresh loop to evaluate liveness.
    pub fn iter_project_sessions(&self) -> Vec<(&str, &PathBuf, &ProjectSession)> {
        let mut out: Vec<(&str, &PathBuf, &ProjectSession)> = Vec::new();
        for (pid_str, inner) in &self.project_sessions {
            for (dir, session) in inner {
                out.push((pid_str.as_str(), dir, session));
            }
        }
        out
    }

    /// Retain only daemons matching the predicate and mark the state dirty
    /// if any were removed.
    pub fn retain_daemons<F>(&mut self, mut f: F)
    where
        F: FnMut(&DaemonId, &Daemon) -> bool,
    {
        let before = self.daemons.len();
        self.daemons.retain(|id, daemon| f(id, daemon));
        if self.daemons.len() != before {
            self.mark_dirty();
        }
    }

    /// Synchronous force-write. Clears the dirty flag. If the serialized
    /// content matches the last written content, the disk write is skipped to
    /// avoid unnecessary I/O. Used during shutdown and migration where async
    /// flushing is not available.
    pub fn write(&self) -> Result<()> {
        let canonical_path = normalized_lock_path(&self.path);
        let _lock = xx::fslock::get(&canonical_path, false)?;
        let raw = toml::to_string(self).map_err(|e| FileError::SerializeError {
            path: self.path.clone(),
            source: e,
        })?;
        if self
            .last_content
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|last| last == &raw)
        {
            // No real change — just clear the dirty flag
            self.dirty.store(false, Ordering::Relaxed);
            return Ok(());
        }
        Self::write_raw(&self.path, &raw)?;
        *self.last_content.lock().unwrap() = Some(raw);
        self.dirty.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Write the state file without acquiring the lock.
    /// Used internally when the lock is already held (e.g., during migration in read()).
    fn write_unlocked(&self) -> Result<()> {
        let raw = toml::to_string(self).map_err(|e| FileError::SerializeError {
            path: self.path.clone(),
            source: e,
        })?;
        Self::write_raw(&self.path, &raw)?;
        *self.last_content.lock().unwrap() = Some(raw);
        self.dirty.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Perform the actual file I/O (temp file + atomic rename).
    /// **The caller MUST hold the file lock** (via `xx::fslock::get`) before
    /// calling this function; otherwise concurrent writes may corrupt the file.
    pub(crate) fn write_raw(path: &Path, raw: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FileError::WriteError {
                path: parent.to_path_buf(),
                details: Some(format!("failed to create state file directory: {e}")),
            })?;
        }
        let temp_path = path.with_extension("toml.tmp");
        xx::file::write(&temp_path, raw).map_err(|e| FileError::WriteError {
            path: temp_path.clone(),
            details: Some(e.to_string()),
        })?;
        std::fs::rename(&temp_path, path).map_err(|e| FileError::WriteError {
            path: path.to_path_buf(),
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
                status: DaemonStatus::Stopped,
                last_exit_success: Some(true),
                user: Some("postgres".to_string()),
                ..Daemon::default()
            },
        );

        let toml_str = toml::to_string(&state).unwrap();
        println!("Serialized TOML:\n{toml_str}");

        let parsed: StateFile = toml::from_str(&toml_str).expect("Failed to parse TOML");
        println!("Parsed: {parsed:?}");

        assert!(
            parsed
                .daemons
                .contains_key(&DaemonId::new("project", "test"))
        );
        let daemon = parsed
            .daemons
            .get(&DaemonId::new("project", "test"))
            .unwrap();
        assert_eq!(daemon.user.as_deref(), Some("postgres"));
    }

    #[test]
    fn record_daemon_exit_does_not_overwrite_replacement_process() {
        let mut state = StateFile::new(PathBuf::from("/tmp/test.toml"));
        let daemon_id = DaemonId::new("project", "worker");
        state.daemons.insert(
            daemon_id.clone(),
            Daemon {
                id: daemon_id.clone(),
                pid: Some(200),
                status: DaemonStatus::Running,
                last_exit_success: None,
                active_port: Some(4321),
                ..Daemon::default()
            },
        );

        // The old PID 100 monitor completes after PID 200 has replaced it.
        assert!(!state.is_dirty());
        assert!(!state.record_daemon_exit(&daemon_id, 100, DaemonStatus::Stopped, true));
        assert!(!state.is_dirty());

        let daemon = state.daemons.get(&daemon_id).unwrap();
        assert_eq!(daemon.pid, Some(200));
        assert!(daemon.status.is_running());
        assert_eq!(daemon.last_exit_success, None);
        assert_eq!(daemon.active_port, Some(4321));
    }

    #[test]
    fn record_daemon_exit_updates_matching_process() {
        let mut state = StateFile::new(PathBuf::from("/tmp/test.toml"));
        let daemon_id = DaemonId::new("project", "worker");
        state.daemons.insert(
            daemon_id.clone(),
            Daemon {
                id: daemon_id.clone(),
                pid: Some(100),
                status: DaemonStatus::Stopping,
                active_port: Some(4321),
                ..Daemon::default()
            },
        );

        assert!(!state.is_dirty());
        assert!(state.record_daemon_exit(&daemon_id, 100, DaemonStatus::Stopped, true,));
        assert!(state.is_dirty());

        let daemon = state.daemons.get(&daemon_id).unwrap();
        assert_eq!(daemon.pid, None);
        assert!(daemon.status.is_stopped());
        assert_eq!(daemon.last_exit_success, Some(true));
        assert_eq!(daemon.active_port, None);
    }

    #[test]
    fn record_daemon_exit_ignores_process_already_cleared_by_stop() {
        let mut state = StateFile::new(PathBuf::from("/tmp/test.toml"));
        let daemon_id = DaemonId::new("project", "worker");
        state.daemons.insert(
            daemon_id.clone(),
            Daemon {
                id: daemon_id.clone(),
                pid: None,
                status: DaemonStatus::Stopped,
                last_exit_success: Some(true),
                ..Daemon::default()
            },
        );

        assert!(!state.record_daemon_exit(&daemon_id, 100, DaemonStatus::Errored(1), false,));
        assert!(!state.is_dirty());

        let daemon = state.daemons.get(&daemon_id).unwrap();
        assert_eq!(daemon.pid, None);
        assert!(daemon.status.is_stopped());
        assert_eq!(daemon.last_exit_success, Some(true));
    }

    #[test]
    fn record_daemon_stop_preserves_existing_exit_result() {
        let mut state = StateFile::new(PathBuf::from("/tmp/test.toml"));
        let daemon_id = DaemonId::new("project", "worker");
        state.daemons.insert(
            daemon_id.clone(),
            Daemon {
                id: daemon_id.clone(),
                pid: Some(100),
                status: DaemonStatus::Errored(1),
                last_exit_success: Some(false),
                active_port: Some(4321),
                ..Daemon::default()
            },
        );

        assert!(state.record_daemon_stop(&daemon_id, 100, None));

        let daemon = state.daemons.get(&daemon_id).unwrap();
        assert_eq!(daemon.pid, None);
        assert!(daemon.status.is_stopped());
        assert_eq!(daemon.last_exit_success, Some(false));
        assert_eq!(daemon.active_port, None);
    }

    #[test]
    fn record_daemon_stop_does_not_overwrite_replacement_process() {
        let mut state = StateFile::new(PathBuf::from("/tmp/test.toml"));
        let daemon_id = DaemonId::new("project", "worker");
        state.daemons.insert(
            daemon_id.clone(),
            Daemon {
                id: daemon_id.clone(),
                pid: Some(200),
                status: DaemonStatus::Running,
                last_exit_success: None,
                active_port: Some(4321),
                ..Daemon::default()
            },
        );

        assert!(!state.record_daemon_stop(&daemon_id, 100, Some(true)));
        assert!(!state.is_dirty());

        let daemon = state.daemons.get(&daemon_id).unwrap();
        assert_eq!(daemon.pid, Some(200));
        assert!(daemon.status.is_running());
        assert_eq!(daemon.last_exit_success, None);
        assert_eq!(daemon.active_port, Some(4321));
    }

    #[test]
    fn record_daemon_stop_does_not_overwrite_monitor_result() {
        let mut state = StateFile::new(PathBuf::from("/tmp/test.toml"));
        let daemon_id = DaemonId::new("project", "worker");
        state.daemons.insert(
            daemon_id.clone(),
            Daemon {
                id: daemon_id.clone(),
                pid: None,
                status: DaemonStatus::Errored(1),
                last_exit_success: Some(false),
                active_port: None,
                ..Daemon::default()
            },
        );

        assert!(!state.record_daemon_stop(&daemon_id, 100, None));
        assert!(!state.is_dirty());

        let daemon = state.daemons.get(&daemon_id).unwrap();
        assert_eq!(daemon.pid, None);
        assert!(matches!(&daemon.status, DaemonStatus::Errored(1)));
        assert_eq!(daemon.last_exit_success, Some(false));
    }

    #[test]
    fn set_daemon_status_if_owned_does_not_restore_cleared_process() {
        let mut state = StateFile::new(PathBuf::from("/tmp/test.toml"));
        let daemon_id = DaemonId::new("project", "worker");
        state.daemons.insert(
            daemon_id.clone(),
            Daemon {
                id: daemon_id.clone(),
                pid: None,
                status: DaemonStatus::Errored(1),
                last_exit_success: Some(false),
                ..Daemon::default()
            },
        );

        assert!(!state.set_daemon_status_if_owned(&daemon_id, 100, DaemonStatus::Running));
        assert!(!state.is_dirty());

        let daemon = state.daemons.get(&daemon_id).unwrap();
        assert_eq!(daemon.pid, None);
        assert!(matches!(&daemon.status, DaemonStatus::Errored(1)));
        assert_eq!(daemon.last_exit_success, Some(false));
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

    #[test]
    fn test_project_sessions_nested_map_roundtrip() {
        let mut state = StateFile::new(PathBuf::from("/tmp/test.toml"));
        state.set_project_session(
            1234,
            PathBuf::from("/projects/a"),
            ProjectSession {
                liveness_title: Some("sleep".to_string()),
            },
        );
        state.set_project_session(
            1234,
            PathBuf::from("/projects/b"),
            ProjectSession {
                liveness_title: None,
            },
        );
        state.set_project_session(
            5678,
            PathBuf::from("/projects/a"),
            ProjectSession {
                liveness_title: Some("code".to_string()),
            },
        );

        let toml_str = toml::to_string(&state).unwrap();
        let parsed: StateFile = toml::from_str(&toml_str).expect("roundtrip parse");

        // All three sessions survive.
        assert_eq!(parsed.iter_project_sessions().len(), 3);
        assert!(
            parsed
                .get_project_session(1234, &PathBuf::from("/projects/a"))
                .is_some()
        );
        assert!(
            parsed
                .get_project_session(1234, &PathBuf::from("/projects/b"))
                .is_some()
        );
        assert!(
            parsed
                .get_project_session(5678, &PathBuf::from("/projects/a"))
                .is_some()
        );

        // Removal prunes the per-PID subtable when it becomes empty.
        let mut state = parsed;
        state.remove_project_session(5678, &PathBuf::from("/projects/a"));
        assert!(
            state
                .get_project_session(5678, &PathBuf::from("/projects/a"))
                .is_none()
        );
        assert!(!state.project_sessions.contains_key("5678"));
        assert_eq!(state.iter_project_sessions().len(), 2);
    }
}
