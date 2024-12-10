use crate::{env, Result};
use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateFile {
    pub daemons: BTreeMap<String, StateFileDaemon>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateFileDaemon {
    pub name: String,
    pub pid: u32,
    pub status: DaemonStatus,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::Display)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum DaemonStatus {
    Failed(String),
    Waiting,
    Running,
}

impl DaemonStatus {
    pub fn style(&self) -> String {
        let s = self.to_string();
        match self {
            DaemonStatus::Failed(_) => console::style(s).red().to_string(),
            DaemonStatus::Waiting => console::style(s).yellow().to_string(),
            DaemonStatus::Running => console::style(s).green().to_string(),
        }
    }
}

impl StateFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            daemons: Default::default(),
            path,
        }
    }

    pub fn get() -> &'static Self {
        static STATE_FILE: Lazy<StateFile> = Lazy::new(|| {
            let path = &*env::PITCHFORK_STATE_FILE;
            StateFile::read(path).expect("Error reading state file")
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
            daemon.name = name.clone();
        }
        Ok(state_file)
    }

    pub fn write(&self) -> Result<()> {
        let _lock = xx::fslock::get(&self.path, false)?;
        let raw = toml::to_string(self).into_diagnostic()?;
        xx::file::write(&self.path, raw)?;
        Ok(())
    }
}
