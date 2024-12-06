use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use crate::Result;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PidFile {
    pids: BTreeMap<String, u32>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
}

impl PidFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            pids: Default::default(),
            path,
        }
    }
    
    pub fn read<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }
        let _lock = xx::fslock::get(path, false)?;
        let raw = xx::file::read_to_string(path)?;
        let mut pid_file: Self = toml::from_str(&raw)?;
        pid_file.path = path.to_path_buf();
        Ok(pid_file)
    }

    pub fn write(&self) -> Result<()> {
        let _lock = xx::fslock::get(&self.path, false)?;
        let raw = toml::to_string(self)?;
        xx::file::write(&self.path, raw)?;
        Ok(())
    }

    pub fn set(&mut self, key: String, value: u32) {
        self.pids.insert(key, value);
    }

    pub fn get(&self, key: &str) -> Option<&u32> {
        self.pids.get(key)
    }
}
