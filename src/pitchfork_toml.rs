use crate::daemon::Daemon;
use indexmap::IndexMap;
use std::path::{Path, PathBuf};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PitchforkToml {
    pub daemons: IndexMap<String, Daemon>,
    #[serde(skip)]
    pub path: PathBuf,
}

impl PitchforkToml {
    pub fn new(path: PathBuf) -> Self {
        Self {
            daemons: Default::default(),
            path,
        }
    }

    pub fn read<P: AsRef<Path>>(path: P) -> eyre::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }
        let _lock = xx::fslock::get(path, false)?;
        let raw = xx::file::read_to_string(path)?;
        let mut pt: Self = toml::from_str(&raw)?;
        pt.path = path.to_path_buf();
        Ok(pt)
    }

    pub fn write(&self) -> eyre::Result<()> {
        let _lock = xx::fslock::get(&self.path, false)?;
        let raw = toml::to_string(self)?;
        xx::file::write(&self.path, raw)?;
        Ok(())
    }
}
