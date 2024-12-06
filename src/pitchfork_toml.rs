use std::path::Path;
use indexmap::IndexMap;
use crate::daemon::Daemon;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PitchforkToml {
    pub daemons: IndexMap<String, Daemon>,
}

impl PitchforkToml {
    pub fn read<P: AsRef<Path>>(path: P) -> eyre::Result<Self> {
        if !path.as_ref().exists() {
            return Ok(Self::default());
        }
        let raw = xx::file::read_to_string(path)?;
        let pids = toml::from_str(&raw)?;
        Ok(pids)
    }

    pub fn write<P: AsRef<Path>>(&self, path: P) -> eyre::Result<()> {
        let raw = toml::to_string(self)?;
        xx::file::write(path, raw)?;
        Ok(())
    }
}
