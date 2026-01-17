#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::Display, strum::EnumIs)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum DaemonStatus {
    Failed(String),
    Waiting,
    Running,
    Stopping,
    Errored(Option<i32>),
    Stopped,
}

impl DaemonStatus {
    pub fn style(&self) -> String {
        let s = self.to_string();
        match self {
            DaemonStatus::Failed(_) => console::style(s).red().to_string(),
            DaemonStatus::Waiting => console::style(s).yellow().to_string(),
            DaemonStatus::Running => console::style(s).green().to_string(),
            DaemonStatus::Stopping => console::style(s).yellow().to_string(),
            DaemonStatus::Stopped => console::style(s).dim().to_string(),
            DaemonStatus::Errored(_) => console::style(s).red().to_string(),
        }
    }

    pub fn error_message(&self) -> Option<String> {
        match self {
            DaemonStatus::Failed(msg) => Some(msg.clone()),
            DaemonStatus::Errored(Some(code)) => Some(format!("exit code {code}")),
            DaemonStatus::Errored(None) => Some("unknown exit code".to_string()),
            _ => None,
        }
    }
}
