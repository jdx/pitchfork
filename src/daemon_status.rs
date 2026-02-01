use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, strum::Display, strum::EnumIs)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_status_json_roundtrip() {
        let variants = vec![
            ("running", DaemonStatus::Running),
            ("stopped", DaemonStatus::Stopped),
            ("waiting", DaemonStatus::Waiting),
            ("stopping", DaemonStatus::Stopping),
            ("failed", DaemonStatus::Failed("some error".to_string())),
            ("errored_some", DaemonStatus::Errored(Some(1))),
            ("errored_none", DaemonStatus::Errored(None)),
        ];

        for (name, status) in variants {
            let json_str = serde_json::to_string(&status)
                .unwrap_or_else(|_| panic!("Failed to serialize {name}"));
            println!("Status {name}: {json_str}");

            let result: Result<DaemonStatus, _> = serde_json::from_str(&json_str);
            assert!(
                result.is_ok(),
                "Failed to deserialize {name}: {:?}",
                result.err()
            );
        }
    }
}
