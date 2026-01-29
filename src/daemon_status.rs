#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, strum::Display, strum::EnumIs)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum DaemonStatus {
    Failed(String),
    Waiting,
    Running,
    Stopping,
    Errored(i32),
    ErroredUnknown,
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
            DaemonStatus::Errored(_) | DaemonStatus::ErroredUnknown => {
                console::style(s).red().to_string()
            }
        }
    }

    pub fn error_message(&self) -> Option<String> {
        match self {
            DaemonStatus::Failed(msg) => Some(msg.clone()),
            DaemonStatus::Errored(code) => Some(format!("exit code {code}")),
            DaemonStatus::ErroredUnknown => Some("unknown exit code".to_string()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_status_toml_roundtrip() {
        use std::collections::BTreeMap;

        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        struct Daemon {
            id: String,
            status: DaemonStatus,
        }

        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            daemons: BTreeMap<String, Daemon>,
        }

        let variants = vec![
            ("running", DaemonStatus::Running),
            ("stopped", DaemonStatus::Stopped),
            ("waiting", DaemonStatus::Waiting),
            ("stopping", DaemonStatus::Stopping),
            ("failed", DaemonStatus::Failed("some error".to_string())),
            ("errored", DaemonStatus::Errored(1)),
            ("errored_unknown", DaemonStatus::ErroredUnknown),
        ];

        for (name, status) in variants {
            let daemon = Daemon {
                id: "test".to_string(),
                status: status.clone(),
            };

            let mut daemons = BTreeMap::new();
            daemons.insert("docs".to_string(), daemon);
            let wrapper = Wrapper { daemons };

            let toml_str =
                toml::to_string(&wrapper).unwrap_or_else(|_| panic!("Failed to serialize {name}"));
            println!("Status {name}:\n{toml_str}");

            let result = toml::from_str::<Wrapper>(&toml_str);
            assert!(
                result.is_ok(),
                "Failed to deserialize {name}: {:?}",
                result.err()
            );
        }
    }
}
