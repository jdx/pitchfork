//! Hook execution for daemon lifecycle events
//!
//! Executes configured hooks for:
//! - on_ready: When daemon passes its ready check
//! - on_fail: When daemon exits with non-zero code
//! - on_retry: Before each retry attempt
//! - on_cron_trigger: When cron schedule triggers

use crate::daemon_id::DaemonId;
use crate::env;
use crate::pitchfork_toml::PitchforkToml;
use crate::shell::Shell;
use std::collections::HashMap;
use std::path::Path;

/// Execute a hook command asynchronously in the background
pub async fn execute_hook(
    hook_name: &str,
    daemon_id: &DaemonId,
    command: &str,
    config_root: &Path,
    extra_env: HashMap<String, String>,
) {
    let hook_name = hook_name.to_string();
    let daemon_id = daemon_id.clone();
    let command = command.to_string();
    let config_root = config_root.to_path_buf();

    tokio::spawn(async move {
        // Use platform-appropriate default shell
        let shell = Shell::default_for_platform();

        info!(
            "executing {} hook for daemon {}: {}",
            hook_name, daemon_id, command
        );

        let mut cmd = shell.command(&command);
        cmd.current_dir(&config_root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Set extra environment variables
        for (key, value) in &extra_env {
            cmd.env(key, value);
        }

        // Pass daemon information as environment variables
        cmd.env("PITCHFORK_DAEMON_ID", daemon_id.qualified());
        cmd.env("PITCHFORK_DAEMON_NAMESPACE", daemon_id.namespace());
        cmd.env("PITCHFORK_DAEMON_NAME", daemon_id.name());
        cmd.env("PITCHFORK_HOOK_NAME", &hook_name);

        match cmd.output().await {
            Ok(output) => {
                if output.status.success() {
                    debug!(
                        "{} hook for daemon {} completed successfully",
                        hook_name, daemon_id
                    );
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let exit_code = output.status.code().unwrap_or(-1);
                    warn!(
                        "{} hook for daemon {} failed with exit code {}: {}",
                        hook_name, daemon_id, exit_code, stderr
                    );
                }
            }
            Err(e) => {
                error!(
                    "failed to execute {} hook for daemon {}: {}",
                    hook_name, daemon_id, e
                );
            }
        }
    });
}

/// Get the config root directory for a daemon from its config
fn get_config_root_from_daemon(
    daemon_config: &crate::pitchfork_toml::PitchforkTomlDaemon,
) -> std::path::PathBuf {
    daemon_config
        .path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| env::CWD.clone())
}

/// Execute the on_ready hook for a daemon if configured
pub async fn execute_on_ready(daemon_id: &DaemonId) {
    let pt = PitchforkToml::all_merged();
    if let Some(daemon_config) = pt.daemons.get(daemon_id)
        && let Some(ref command) = daemon_config.on_ready
    {
        let config_root = get_config_root_from_daemon(daemon_config);
        execute_hook("on_ready", daemon_id, command, &config_root, HashMap::new()).await;
    }
}

/// Execute the on_fail hook for a daemon if configured
pub async fn execute_on_fail(daemon_id: &DaemonId, exit_code: Option<i32>) {
    let pt = PitchforkToml::all_merged();
    if let Some(daemon_config) = pt.daemons.get(daemon_id)
        && let Some(ref command) = daemon_config.on_fail
    {
        let config_root = get_config_root_from_daemon(daemon_config);

        let mut extra_env = HashMap::new();
        if let Some(code) = exit_code {
            extra_env.insert("PITCHFORK_EXIT_CODE".to_string(), code.to_string());
        }

        execute_hook("on_fail", daemon_id, command, &config_root, extra_env).await;
    }
}

/// Execute the on_retry hook for a daemon if configured
pub async fn execute_on_retry(daemon_id: &DaemonId, retry_count: u32, max_retries: u32) {
    let pt = PitchforkToml::all_merged();
    if let Some(daemon_config) = pt.daemons.get(daemon_id)
        && let Some(ref command) = daemon_config.on_retry
    {
        let config_root = get_config_root_from_daemon(daemon_config);

        let mut extra_env = HashMap::new();
        extra_env.insert("PITCHFORK_RETRY_COUNT".to_string(), retry_count.to_string());
        extra_env.insert("PITCHFORK_MAX_RETRIES".to_string(), max_retries.to_string());

        execute_hook("on_retry", daemon_id, command, &config_root, extra_env).await;
    }
}

/// Execute the on_cron_trigger hook for a daemon if configured
pub async fn execute_on_cron_trigger(daemon_id: &DaemonId) {
    let pt = PitchforkToml::all_merged();
    if let Some(daemon_config) = pt.daemons.get(daemon_id)
        && let Some(ref command) = daemon_config.on_cron_trigger
    {
        let config_root = get_config_root_from_daemon(daemon_config);
        execute_hook(
            "on_cron_trigger",
            daemon_id,
            command,
            &config_root,
            HashMap::new(),
        )
        .await;
    }
}
