use crate::{env, Result};
use indexmap::IndexMap;
use miette::{bail, IntoDiagnostic};
use std::path::{Path, PathBuf};
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PitchforkToml {
    pub daemons: IndexMap<String, PitchforkTomlDaemon>,
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

impl PitchforkToml {
    pub fn list_paths() -> Vec<PathBuf> {
        xx::file::find_up_all(&env::CWD, &["pitchfork.toml"])
    }

    pub fn all_merged() -> PitchforkToml {
        let mut pt = Self::default();
        for p in Self::list_paths() {
            match Self::read(&p) {
                Ok(pt2) => pt.merge(pt2),
                Err(e) => eprintln!("error reading {}: {}", p.display(), e),
            }
        }
        pt
    }
}

impl PitchforkToml {
    pub fn new(path: PathBuf) -> Self {
        Self {
            daemons: Default::default(),
            path: Some(path),
        }
    }

    pub fn read<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }
        let _lock = xx::fslock::get(path, false)?;
        let raw = xx::file::read_to_string(path)?;
        let mut pt: Self = toml::from_str(&raw).into_diagnostic()?;
        pt.path = Some(path.to_path_buf());
        for (_id, d) in pt.daemons.iter_mut() {
            d.path = pt.path.clone();
        }
        Ok(pt)
    }

    pub fn write(&self) -> Result<()> {
        if let Some(path) = &self.path {
            let _lock = xx::fslock::get(path, false)?;
            let raw = toml::to_string(self).into_diagnostic()?;
            xx::file::write(path, raw)?;
            Ok(())
        } else {
            bail!("no path to write to");
        }
    }

    fn merge(&mut self, pt: Self) {
        for (id, d) in pt.daemons {
            self.daemons.insert(id, d);
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PitchforkTomlDaemon {
    pub run: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub auto: Vec<PitchforkTomlAuto>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cron: Option<PitchforkTomlCron>,
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PitchforkTomlCron {
    pub schedule: String,
    #[serde(default = "default_retrigger")]
    pub retrigger: CronRetrigger,
}

fn default_retrigger() -> CronRetrigger {
    CronRetrigger::Finish
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CronRetrigger {
    /// Retrigger if the previous command is finished (success or error)
    Finish,
    /// Always retrigger, stop the previous command if running
    Always,
    /// Retrigger only if the previous command succeeded
    Success,
    /// Retrigger only if the previous command failed
    Fail,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(PartialEq)]
pub enum PitchforkTomlAuto {
    Start,
    Stop,
}
