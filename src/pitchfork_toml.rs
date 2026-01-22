use crate::error::{ConfigParseError, FileError};
use crate::{Result, env};
use indexmap::IndexMap;
use miette::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
        let _lock = xx::fslock::get(path, false)
            .wrap_err_with(|| format!("failed to acquire lock on {}", path.display()))?;
        let raw = std::fs::read_to_string(path).map_err(|e| FileError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;
        let mut pt: Self = toml::from_str(&raw)
            .map_err(|e| ConfigParseError::from_toml_error(path, raw.clone(), e))?;
        pt.path = Some(path.to_path_buf());
        for (_id, d) in pt.daemons.iter_mut() {
            d.path = pt.path.clone();
        }
        Ok(pt)
    }

    pub fn write(&self) -> Result<()> {
        if let Some(path) = &self.path {
            let _lock = xx::fslock::get(path, false)
                .wrap_err_with(|| format!("failed to acquire lock on {}", path.display()))?;
            let raw = toml::to_string(self).map_err(|e| FileError::SerializeError {
                path: path.clone(),
                source: e,
            })?;
            xx::file::write(path, &raw).map_err(|e| FileError::WriteError {
                path: path.clone(),
                details: Some(e.to_string()),
            })?;
            Ok(())
        } else {
            Err(FileError::NoPath.into())
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
    #[schemars(example = example_run_command())]
    pub run: String,
    /// Automatic start/stop behavior based on shell hooks
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub auto: Vec<PitchforkTomlAuto>,
    /// Cron scheduling configuration for periodic execution
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cron: Option<PitchforkTomlCron>,
    /// Number of times to retry if the daemon fails.
    /// Can be a number (e.g., `3`) or `true` for infinite retries.
    #[serde(default)]
    pub retry: Retry,
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
    /// Shell command to poll for readiness (exit code 0 = ready)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ready_cmd: Option<String>,
    /// Whether to start this daemon automatically on system boot
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub boot_start: Option<bool>,
    /// List of daemon names that must be started before this one
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub watch: Vec<String>,
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
    #[schemars(example = example_cron_schedule())]
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

/// Retry configuration that accepts either a boolean or a count.
/// - `true` means retry indefinitely (u32::MAX)
/// - `false` or `0` means no retries
/// - A number means retry that many times
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
pub struct Retry(pub u32);

impl std::fmt::Display for Retry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_infinite() {
            write!(f, "infinite")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

impl Retry {
    pub const INFINITE: Retry = Retry(u32::MAX);

    pub fn count(&self) -> u32 {
        self.0
    }

    pub fn is_infinite(&self) -> bool {
        self.0 == u32::MAX
    }
}

impl From<u32> for Retry {
    fn from(n: u32) -> Self {
        Retry(n)
    }
}

impl From<bool> for Retry {
    fn from(b: bool) -> Self {
        if b { Retry::INFINITE } else { Retry(0) }
    }
}

impl Serialize for Retry {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize infinite as true, otherwise as number
        if self.is_infinite() {
            serializer.serialize_bool(true)
        } else {
            serializer.serialize_u32(self.0)
        }
    }
}

impl<'de> Deserialize<'de> for Retry {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, Visitor};

        struct RetryVisitor;

        impl Visitor<'_> for RetryVisitor {
            type Value = Retry;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a boolean or non-negative integer")
            }

            fn visit_bool<E>(self, v: bool) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Retry::from(v))
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    Err(de::Error::custom("retry count cannot be negative"))
                } else if v > u32::MAX as i64 {
                    Ok(Retry::INFINITE)
                } else {
                    Ok(Retry(v as u32))
                }
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v > u32::MAX as u64 {
                    Ok(Retry::INFINITE)
                } else {
                    Ok(Retry(v as u32))
                }
            }
        }

        deserializer.deserialize_any(RetryVisitor)
    }
}
