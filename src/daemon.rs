use crate::daemon_status::DaemonStatus;
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
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RunOptions {
    pub id: String,
    pub cmd: Vec<String>,
    pub force: bool,
    pub shell_pid: Option<u32>,
    pub dir: Option<PathBuf>,
    pub autostop: bool,
}

impl Display for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}
