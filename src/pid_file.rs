use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Debug;
use std::path::Path;
use crate::Result;

#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub struct PidFile {
    pids: BTreeMap<String, u32>,
}

impl PidFile {
    pub fn read<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let _lock = xx::fslock::get(path, false)?;
        let raw = xx::file::read_to_string(path)?;
        let pids = toml::from_str(&raw)?;
        Ok(pids)
    }

    pub fn write<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();
        let _lock = xx::fslock::get(path, false)?;
        let raw = toml::to_string(self)?;
        xx::file::write(path, raw)?;
        Ok(())
    }

    pub fn set(&mut self, key: String, value: u32) {
        self.pids.insert(key, value);
    }

    pub fn get(&self, key: &str) -> Option<&u32> {
        self.pids.get(key)
    }
}
