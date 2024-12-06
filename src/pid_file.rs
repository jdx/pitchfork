use std::collections::BTreeMap;
use std::path::Path;
use crate::Result;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PidFile {
    pub pids: BTreeMap<String, u32>,
}

impl PidFile {
    pub fn read<P: AsRef<Path>>(path: P) -> Result<Self> {
        let raw = xx::file::read_to_string(path)?;
        let pids = toml::from_str(&raw)?;
        Ok(pids)
    }

    pub fn write<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let raw = toml::to_string(self)?;
        xx::file::write(path, raw)?;
        Ok(())
    }
}
