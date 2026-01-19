use crate::daemon::Daemon;
use crate::error::FileError;
use crate::{Result, env};
use once_cell::sync::Lazy;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateFile {
    pub daemons: BTreeMap<String, Daemon>,
    pub disabled: BTreeSet<String>,
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
        let _lock = xx::fslock::get(path, false)?;
        let raw = xx::file::read_to_string(path).unwrap_or_else(|e| {
            warn!("Error reading state file {:?}: {}", path, e);
            String::new()
        });
        let mut state_file: Self = toml::from_str(&raw).unwrap_or_else(|e| {
            warn!("Error parsing state file {:?}: {}", path, e);
            Self::new(path.to_path_buf())
        });
        state_file.path = path.to_path_buf();
        for (name, daemon) in state_file.daemons.iter_mut() {
            daemon.id = name.clone();
        }
        Ok(state_file)
    }

    pub fn write(&self) -> Result<()> {
        let _lock = xx::fslock::get(&self.path, false)?;
        let raw = toml::to_string(self).map_err(|e| FileError::WriteError {
            path: self.path.clone(),
            details: Some(format!("serialization failed: {}", e)),
        })?;
        xx::file::write(&self.path, raw).map_err(|e| FileError::WriteError {
            path: self.path.clone(),
            details: Some(e.to_string()),
        })?;
        Ok(())
    }
}
