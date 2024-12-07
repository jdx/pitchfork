use crate::Result;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::{Path, PathBuf};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct StateFile {
    pub daemons: BTreeMap<String, StateFileDaemon>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct StateFileDaemon {
    pub pid: u32,
    pub status: DaemonStatus,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonStatus {
    Waiting,
    Running,
}

impl StateFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            daemons: Default::default(),
            path,
        }
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
        Ok(state_file)
    }

    pub fn write(&self) -> Result<()> {
        let _lock = xx::fslock::get(&self.path, false)?;
        let raw = toml::to_string(self)?;
        xx::file::write(&self.path, raw)?;
        Ok(())
    }
}
