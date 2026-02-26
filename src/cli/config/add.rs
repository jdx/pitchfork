use crate::Result;
use crate::pitchfork_toml::{
    CronRetrigger, PitchforkToml, PitchforkTomlAuto, PitchforkTomlCron, PitchforkTomlDaemon,
    PitchforkTomlHooks, Retry,
};
use indexmap::IndexMap;
use miette::bail;
use std::path::PathBuf;

/// Add a new daemon to ./pitchfork.toml
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "a",
    verbatim_doc_comment,
    long_about = "\
Add a new daemon to ./pitchfork.toml

Creates a new daemon configuration section in the pitchfork.toml file.
The daemon will be added to the nearest pitchfork.toml found in the
filesystem hierarchy starting from the current directory.

Examples:
  pitchfork config add api bun run server
                                Add daemon using positional args
  pitchfork config add api --run 'npm start'
                                Add daemon with explicit run command
  pitchfork config add api -- bun run server
                                Add daemon with explicit args after --
  pitchfork config add api --run 'npm start' --retry 3
                                Add with retry policy
  pitchfork config add api --run 'npm start' --watch 'src/**/*.ts'
                                Add with file watching
  pitchfork config add api --run 'npm start' --autostart --autostop
                                Add with auto start/stop hooks
  pitchfork config add worker --run './worker' --depends api
                                Add with daemon dependency
"
)]
pub struct Add {
    /// ID of the daemon to add
    id: String,
    /// Command to run (can also use positional args)
    #[clap(long)]
    run: Option<String>,
    /// Arguments to pass to the daemon (alternative to --run)
    #[clap(allow_hyphen_values = true, trailing_var_arg = true)]
    args: Vec<String>,
    /// Number of retry attempts on failure (use \"true\" for infinite)
    #[clap(long)]
    retry: Option<String>,
    /// Glob patterns to watch for changes (can be specified multiple times)
    #[clap(long = "watch")]
    watch: Vec<String>,
    /// Working directory for the daemon
    #[clap(long)]
    dir: Option<String>,
    /// Environment variables in KEY=value format (can be specified multiple times)
    #[clap(long = "env")]
    env: Vec<String>,
    /// Delay in seconds before considering daemon ready
    #[clap(long)]
    ready_delay: Option<u64>,
    /// Regex pattern to match in output for readiness
    #[clap(long)]
    ready_output: Option<String>,
    /// HTTP endpoint URL to poll for readiness
    #[clap(long)]
    ready_http: Option<String>,
    /// TCP port to check for readiness
    #[clap(long)]
    ready_port: Option<u16>,
    /// Shell command to poll for readiness
    #[clap(long)]
    ready_cmd: Option<String>,
    /// Ports the daemon is expected to bind to (can be specified multiple times or comma-separated)
    #[clap(long, value_delimiter = ',')]
    port: Vec<u16>,
    /// Automatically find an available port if the expected port is in use
    #[clap(long)]
    auto_bump_port: bool,
    /// Daemon dependencies that must start first (can be specified multiple times)
    #[clap(long = "depends")]
    depends: Vec<String>,
    /// Start this daemon automatically on system boot
    #[clap(long)]
    boot_start: bool,
    /// Autostart the daemon when entering the directory
    #[clap(long)]
    autostart: bool,
    /// Autostop the daemon when leaving the directory
    #[clap(long)]
    autostop: bool,
    /// Command to run when daemon becomes ready
    #[clap(long)]
    on_ready: Option<String>,
    /// Command to run when daemon fails
    #[clap(long)]
    on_fail: Option<String>,
    /// Command to run before each retry attempt
    #[clap(long)]
    on_retry: Option<String>,
    /// Cron schedule expression (6 fields: second minute hour day month weekday)
    #[clap(long)]
    cron_schedule: Option<String>,
    /// Cron retrigger behavior: finish, always, success, fail
    #[clap(long)]
    cron_retrigger: Option<String>,
}

impl Add {
    pub async fn run(&self) -> Result<()> {
        // Find an existing project-level config or default to ./pitchfork.toml
        let paths = PitchforkToml::list_paths();
        let project_paths: Vec<_> = paths
            .iter()
            .filter(|p| {
                // Filter to only project-level configs (not system or user)
                !p.starts_with("/etc") && !p.starts_with(&*crate::env::HOME_DIR)
            })
            .collect();
        let path = project_paths
            .last()
            .map(|p| (*p).clone())
            .unwrap_or_else(|| PathBuf::from("pitchfork.toml"));

        let mut pt = PitchforkToml::read(&path).unwrap_or_default();
        pt.path = Some(path.clone());

        // Build the run command
        let run_cmd = if let Some(ref run) = self.run {
            run.clone()
        } else if !self.args.is_empty() {
            shell_words::join(&self.args)
        } else {
            bail!("Either --run or command arguments must be provided");
        };

        // Parse retry option
        let retry = if let Some(ref retry_str) = self.retry {
            Self::parse_retry(retry_str)?
        } else {
            Retry::default()
        };

        // Parse environment variables
        let env = if self.env.is_empty() {
            None
        } else {
            let mut map = IndexMap::new();
            for env_str in &self.env {
                let parts: Vec<&str> = env_str.splitn(2, '=').collect();
                if parts.len() != 2 {
                    bail!(
                        "Invalid environment variable format: {}. Expected KEY=value",
                        env_str
                    );
                }
                map.insert(parts[0].to_string(), parts[1].to_string());
            }
            Some(map)
        };

        // Build auto vector
        let mut auto = vec![];
        if self.autostart {
            auto.push(PitchforkTomlAuto::Start);
        }
        if self.autostop {
            auto.push(PitchforkTomlAuto::Stop);
        }

        // Build hooks if any are specified
        let hooks = if self.on_ready.is_some() || self.on_fail.is_some() || self.on_retry.is_some()
        {
            Some(PitchforkTomlHooks {
                on_ready: self.on_ready.clone(),
                on_fail: self.on_fail.clone(),
                on_retry: self.on_retry.clone(),
            })
        } else {
            None
        };

        // Build cron config if schedule is specified
        let cron = if let Some(ref schedule) = self.cron_schedule {
            let retrigger = self
                .cron_retrigger
                .as_ref()
                .map(|s| Self::parse_cron_retrigger(s))
                .transpose()?
                .unwrap_or(CronRetrigger::Finish);
            Some(PitchforkTomlCron {
                schedule: schedule.clone(),
                retrigger,
            })
        } else {
            None
        };

        // Build boot_start
        let boot_start = if self.boot_start { Some(true) } else { None };

        pt.daemons.insert(
            self.id.clone(),
            PitchforkTomlDaemon {
                run: run_cmd,
                auto,
                cron,
                retry,
                ready_delay: self.ready_delay,
                ready_output: self.ready_output.clone(),
                ready_http: self.ready_http.clone(),
                ready_port: self.ready_port,
                ready_cmd: self.ready_cmd.clone(),
                port: self.port.clone(),
                auto_bump_port: self.auto_bump_port,
                boot_start,
                depends: self.depends.clone(),
                watch: self.watch.clone(),
                dir: self.dir.clone(),
                env,
                hooks,
                path: None,
            },
        );
        pt.write()?;
        println!("added {} to {}", self.id, path.display());
        Ok(())
    }

    fn parse_retry(s: &str) -> Result<Retry> {
        if s.eq_ignore_ascii_case("true") {
            Ok(Retry::INFINITE)
        } else if s.eq_ignore_ascii_case("false") {
            Ok(Retry(0))
        } else {
            match s.parse::<u32>() {
                Ok(n) => Ok(Retry(n)),
                Err(_) => bail!(
                    "Invalid retry value: {}. Expected a number or 'true'/'false'",
                    s
                ),
            }
        }
    }

    fn parse_cron_retrigger(s: &str) -> Result<CronRetrigger> {
        match s.to_lowercase().as_str() {
            "finish" => Ok(CronRetrigger::Finish),
            "always" => Ok(CronRetrigger::Always),
            "success" => Ok(CronRetrigger::Success),
            "fail" => Ok(CronRetrigger::Fail),
            _ => bail!(
                "Invalid cron retrigger value: {}. Expected 'finish', 'always', 'success', or 'fail'",
                s
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_retry_numeric() {
        assert_eq!(Add::parse_retry("0").unwrap().count(), 0);
        assert_eq!(Add::parse_retry("3").unwrap().count(), 3);
        assert_eq!(Add::parse_retry("10").unwrap().count(), 10);
    }

    #[test]
    fn test_parse_retry_boolean() {
        assert!(Add::parse_retry("true").unwrap().is_infinite());
        assert!(Add::parse_retry("TRUE").unwrap().is_infinite());
        assert!(Add::parse_retry("True").unwrap().is_infinite());
        assert_eq!(Add::parse_retry("false").unwrap().count(), 0);
        assert_eq!(Add::parse_retry("FALSE").unwrap().count(), 0);
    }

    #[test]
    fn test_parse_retry_invalid() {
        assert!(Add::parse_retry("invalid").is_err());
        assert!(Add::parse_retry("").is_err());
    }

    #[test]
    fn test_parse_cron_retrigger_valid() {
        assert_eq!(
            Add::parse_cron_retrigger("finish").unwrap(),
            CronRetrigger::Finish
        );
        assert_eq!(
            Add::parse_cron_retrigger("FINISH").unwrap(),
            CronRetrigger::Finish
        );
        assert_eq!(
            Add::parse_cron_retrigger("always").unwrap(),
            CronRetrigger::Always
        );
        assert_eq!(
            Add::parse_cron_retrigger("success").unwrap(),
            CronRetrigger::Success
        );
        assert_eq!(
            Add::parse_cron_retrigger("fail").unwrap(),
            CronRetrigger::Fail
        );
    }

    #[test]
    fn test_parse_cron_retrigger_invalid() {
        assert!(Add::parse_cron_retrigger("invalid").is_err());
        assert!(Add::parse_cron_retrigger("").is_err());
        assert!(Add::parse_cron_retrigger("never").is_err());
    }
}
