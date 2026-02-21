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
    pub daemons: BTreeMap<DaemonId, Daemon>,
    pub disabled: BTreeSet<DaemonId>,
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
                warn!("Could not read state file: {e}, starting fresh");
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
        // Canonicalize path to ensure consistent locking across processes
        // (e.g., /var vs /private/var on macOS)
        let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
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
            Err(e) => {
                // Check if this looks like an old format (unqualified daemon IDs)
                if raw.contains("[daemons.") && !raw.contains("[daemons.global/") {
                    warn!(
                        "State file appears to be in old format (unqualified daemon IDs). \
                         Please delete {} and restart pitchfork to recreate it with the new format.",
                        path.display()
                    );
                } else {
                    warn!("Error parsing state file {path:?}: {e}");
                }
                Ok(Self::new(path.to_path_buf()))
            }
        }
    }

    pub fn write(&self) -> Result<()> {
        // Canonicalize path to ensure consistent locking across processes
        // (e.g., /var vs /private/var on macOS)
        let canonical_path = self
            .path
            .canonicalize()
            .unwrap_or_else(|_| self.path.clone());
        let _lock = xx::fslock::get(&canonical_path, false)?;
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
}
