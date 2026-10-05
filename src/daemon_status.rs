use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, strum::Display, strum::EnumIs)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum DaemonStatus {
    Failed(String),
    Waiting,
    Running,
    Stopping,
    /// The supervisor is restarting the daemon (a watched file changed, or a
    /// `restart`/`start --force` replaced it): the old process is being or
    /// has been stopped and the new one has not started yet.
    ///
    /// Not a final state: it becomes `running`/`waiting` once the new process
    /// starts, or a terminal status if the restart fails. `pitchfork wait`
    /// keeps following the daemon through it.
    Restarting,
    /// Exit code of the process, or -1 if unknown.
    Errored(i32),
    /// A `oneshot = true` daemon whose process ran to completion with exit
    /// code 0.
    ///
    /// Distinct from `Stopped` so the CLI, the TUI and the web UI can tell
    /// "finished its work" from "never ran" or "was interrupted", and so a
    /// failed run stays distinguishable from a successful one.
    ///
    /// It does not mark the task as permanently done: a start re-runs a
    /// completed oneshot, including when it is reached as a dependency, which
    /// is why the guide requires the command to be idempotent. What waits on
    /// a oneshot is the start that is running it, not this status.
    Completed,
    #[default]
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
            DaemonStatus::Restarting => console::style(s).yellow().to_string(),
            DaemonStatus::Stopped => console::style(s).dim().to_string(),
            DaemonStatus::Completed => console::style(s).green().dim().to_string(),
            DaemonStatus::Errored(_) => console::style(s).red().to_string(),
        }
    }

    /// Whether the daemon's process is being, or has just been, stopped on
    /// purpose: a plain stop in flight (`stopping`) or a restart.
    pub fn is_stopping_or_restarting(&self) -> bool {
        self.is_stopping() || self.is_restarting()
    }

    pub fn error_message(&self) -> Option<String> {
        match self {
            DaemonStatus::Failed(msg) => Some(msg.clone()),
            DaemonStatus::Errored(code) if *code != -1 => Some(format!("exit code {code}")),
            DaemonStatus::Errored(_) => Some("unknown exit code".to_string()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_variants() -> Vec<(&'static str, DaemonStatus)> {
        vec![
            ("running", DaemonStatus::Running),
            ("stopped", DaemonStatus::Stopped),
            ("waiting", DaemonStatus::Waiting),
            ("stopping", DaemonStatus::Stopping),
            ("restarting", DaemonStatus::Restarting),
            ("failed", DaemonStatus::Failed("some error".to_string())),
            ("errored", DaemonStatus::Errored(1)),
            ("errored_unknown", DaemonStatus::Errored(-1)),
            ("completed", DaemonStatus::Completed),
        ]
    }

    #[test]
    fn test_completed_serializes_as_completed() {
        // `mise daemons ls` and other consumers read this string; keep it
        // stable and distinct from "stopped".
        assert_eq!(DaemonStatus::Completed.to_string(), "completed");
        assert_eq!(
            serde_json::to_string(&DaemonStatus::Completed).unwrap(),
            "\"completed\""
        );
    }

    #[test]
    fn test_restarting_serializes_as_restarting() {
        assert_eq!(DaemonStatus::Restarting.to_string(), "restarting");
        assert_eq!(
            serde_json::to_string(&DaemonStatus::Restarting).unwrap(),
            "\"restarting\""
        );
        assert!(DaemonStatus::Restarting.error_message().is_none());
        assert!(DaemonStatus::Restarting.is_stopping_or_restarting());
        assert!(!DaemonStatus::Running.is_stopping_or_restarting());
    }

    #[test]
    fn test_completed_has_no_error_message() {
        assert!(DaemonStatus::Completed.error_message().is_none());
    }

    #[test]
    fn test_daemon_status_json_roundtrip() {
        for (name, status) in all_variants() {
            let json_str = serde_json::to_string(&status)
                .unwrap_or_else(|_| panic!("Failed to serialize {name}"));
            let result: Result<DaemonStatus, _> = serde_json::from_str(&json_str);
            assert!(
                result.is_ok(),
                "Failed to deserialize {name}: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn test_daemon_status_toml_roundtrip() {
        #[derive(Serialize, Deserialize, Debug)]
        struct Wrapper {
            status: DaemonStatus,
        }

        for (name, status) in all_variants() {
            let w = Wrapper { status };
            let toml_str =
                toml::to_string(&w).unwrap_or_else(|e| panic!("Failed to serialize {name}: {e}"));
            let result: Result<Wrapper, _> = toml::from_str(&toml_str);
            assert!(
                result.is_ok(),
                "Failed to deserialize {name}: {:?}\nTOML was: {toml_str:?}",
                result.err()
            );
        }
    }
}
