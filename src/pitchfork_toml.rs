use crate::{Result, env};
use indexmap::IndexMap;
use miette::{IntoDiagnostic, bail};
use schemars::JsonSchema;
use std::path::{Path, PathBuf};

/// Configuration schema for pitchfork.toml daemon supervisor configuration files
#[derive(Debug, Default, serde::Serialize, serde::Deserialize, JsonSchema)]
#[schemars(title = "Pitchfork Configuration")]
pub struct PitchforkToml {
    /// Map of daemon names to their configurations
    pub daemons: IndexMap<String, PitchforkTomlDaemon>,
    #[serde(skip)]
    #[schemars(skip)]
    pub path: Option<PathBuf>,
}

impl PitchforkToml {
    pub fn list_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        paths.push(env::PITCHFORK_GLOBAL_CONFIG_SYSTEM.clone());
        paths.push(env::PITCHFORK_GLOBAL_CONFIG_USER.clone());
        paths.extend(xx::file::find_up_all(&env::CWD, &["pitchfork.toml"]));
        paths
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

    pub fn merge(&mut self, pt: Self) {
        for (id, d) in pt.daemons {
            self.daemons.insert(id, d);
        }
    }
}

/// Configuration for a single daemon
#[derive(Debug, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct PitchforkTomlDaemon {
    /// The command to run. Prepend with 'exec' to avoid shell process overhead.
    #[schemars(example = "example_run_command")]
    pub run: String,
    /// Automatic start/stop behavior based on shell hooks
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub auto: Vec<PitchforkTomlAuto>,
    /// Cron scheduling configuration for periodic execution
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cron: Option<PitchforkTomlCron>,
    /// Number of times to retry if the daemon fails (0 = no retries)
    #[serde(default)]
    pub retry: u32,
    /// Delay in milliseconds before considering the daemon ready
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_delay: Option<u64>,
    /// Regex pattern to match in stdout/stderr to determine readiness
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_output: Option<String>,
    /// HTTP URL to poll for readiness (expects 2xx response)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_http: Option<String>,
    /// TCP port to check for readiness (connection success = ready)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[schemars(range(min = 1, max = 65535))]
    pub ready_port: Option<u16>,
    /// Whether to start this daemon automatically on system boot
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub boot_start: Option<bool>,
    /// List of daemon names that must be started before this one
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<String>,
    #[serde(skip)]
    #[schemars(skip)]
    pub path: Option<PathBuf>,
}

fn example_run_command() -> &'static str {
    "exec node server.js"
}

/// Cron scheduling configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct PitchforkTomlCron {
    /// Cron expression (e.g., '0 * * * *' for hourly, '*/5 * * * *' for every 5 minutes)
    #[schemars(example = "example_cron_schedule")]
    pub schedule: String,
    /// Behavior when cron triggers while previous run is still active
    #[serde(default = "default_retrigger")]
    pub retrigger: CronRetrigger,
}

fn default_retrigger() -> CronRetrigger {
    CronRetrigger::Finish
}

fn example_cron_schedule() -> &'static str {
    "0 * * * *"
}

/// Retrigger behavior for cron-scheduled daemons
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CronRetrigger {
    /// Retrigger only if the previous run has finished (success or error)
    Finish,
    /// Always retrigger, stopping the previous run if still active
    Always,
    /// Retrigger only if the previous run succeeded
    Success,
    /// Retrigger only if the previous run failed
    Fail,
}

/// Automatic behavior triggered by shell hooks
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PitchforkTomlAuto {
    /// Automatically start when entering the directory
    Start,
    /// Automatically stop when leaving the directory
    Stop,
}
