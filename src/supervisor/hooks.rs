//! Hook execution for daemon lifecycle events
//!
//! Hooks are fire-and-forget shell commands that run in response to daemon
//! lifecycle events (ready, fail, retry). They are configured in pitchfork.toml
//! under `[daemons.<name>.hooks]`.

use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::PitchforkToml;
use crate::shell::Shell;
use crate::{env, pitchfork_toml};
use indexmap::IndexMap;
use std::path::PathBuf;

/// The type of lifecycle hook to fire
#[allow(clippy::enum_variant_names)]
pub(crate) enum HookType {
    OnReady,
    OnFail,
    OnRetry,
}

impl std::fmt::Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookType::OnReady => write!(f, "on_ready"),
            HookType::OnFail => write!(f, "on_fail"),
            HookType::OnRetry => write!(f, "on_retry"),
        }
    }
}

fn get_hook_cmd(
    hooks: &Option<pitchfork_toml::PitchforkTomlHooks>,
    hook_type: &HookType,
) -> Option<String> {
    hooks.as_ref().and_then(|h| match hook_type {
        HookType::OnReady => h.on_ready.clone(),
        HookType::OnFail => h.on_fail.clone(),
        HookType::OnRetry => h.on_retry.clone(),
    })
}

/// Fire a hook command as a fire-and-forget tokio task.
///
/// Reads the hook command from fresh config (`PitchforkToml::all_merged()`),
/// then spawns it in the background. Errors are logged but never block the caller.
pub(crate) fn fire_hook(
    hook_type: HookType,
    daemon_id: DaemonId,
    daemon_dir: PathBuf,
    retry_count: u32,
    daemon_env: Option<IndexMap<String, String>>,
    extra_env: Vec<(String, String)>,
) {
    tokio::spawn(async move {
        let pt = PitchforkToml::all_merged().unwrap_or_else(|e| {
            warn!("Failed to load config for hook '{}': {}", hook_type, e);
            PitchforkToml::default()
        });
        let hook_cmd = pt
            .daemons
            .get(&daemon_id)
            .and_then(|d| get_hook_cmd(&d.hooks, &hook_type));

        let Some(cmd) = hook_cmd else { return };

        info!(
            "firing {} hook for daemon {}: {}",
            hook_type, daemon_id, cmd
        );

        let mut command = Shell::default_for_platform().command(&cmd);
        command
            .current_dir(&daemon_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        if let Some(ref path) = *env::ORIGINAL_PATH {
            command.env("PATH", path);
        }

        // Apply user env vars first
        if let Some(ref env_vars) = daemon_env {
            command.envs(env_vars);
        }

        // Inject pitchfork metadata env vars AFTER user env so they can't be overwritten
        command
            .env("PITCHFORK_DAEMON_ID", daemon_id.name())
            .env("PITCHFORK_RETRY_COUNT", retry_count.to_string());

        for (key, value) in &extra_env {
            command.env(key, value);
        }

        match command.status().await {
            Ok(status) => {
                if !status.success() {
                    warn!(
                        "{} hook for daemon {} exited with {}",
                        hook_type, daemon_id, status
                    );
                }
            }
            Err(e) => {
                error!(
                    "failed to execute {} hook for daemon {}: {}",
                    hook_type, daemon_id, e
                );
            }
        }
    });
}
