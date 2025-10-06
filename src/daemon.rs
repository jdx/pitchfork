use crate::daemon_status::DaemonStatus;
use crate::pitchfork_toml::CronRetrigger;
use std::fmt::Display;
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Daemon {
    pub id: String,
    pub title: Option<String>,
    pub pid: Option<u32>,
    pub shell_pid: Option<u32>,
    pub status: DaemonStatus,
    pub dir: Option<PathBuf>,
    pub autostop: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cron_schedule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cron_retrigger: Option<CronRetrigger>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_exit_success: Option<bool>,
    #[serde(default)]
    pub retry: u32,
    #[serde(default)]
    pub retry_count: u32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RunOptions {
    pub id: String,
    pub cmd: Vec<String>,
    pub force: bool,
    pub shell_pid: Option<u32>,
    pub dir: PathBuf,
    pub autostop: bool,
    pub cron_schedule: Option<String>,
    pub cron_retrigger: Option<CronRetrigger>,
    pub retry: u32,
    pub retry_count: u32,
}

impl Display for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}
