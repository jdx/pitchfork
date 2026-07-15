//! Hook execution for daemon lifecycle events
//!
//! Hooks are fire-and-forget shell commands that run in response to daemon
//! lifecycle events (ready, fail, retry). They are configured in pitchfork.toml
//! under `[daemons.<name>.hooks]`.

use crate::Result;
use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::settings;
use crate::supervisor::SUPERVISOR;
use crate::{env, pitchfork_toml, template};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::PathBuf;

/// The type of lifecycle hook to fire
#[allow(clippy::enum_variant_names)]
pub(crate) enum HookType {
    OnReady,
    OnFail,
    OnRetry,
    OnStop,
    OnExit,
}

impl std::fmt::Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookType::OnReady => write!(f, "on_ready"),
            HookType::OnFail => write!(f, "on_fail"),
            HookType::OnRetry => write!(f, "on_retry"),
            HookType::OnStop => write!(f, "on_stop"),
            HookType::OnExit => write!(f, "on_exit"),
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
        HookType::OnStop => h.on_stop.clone(),
        HookType::OnExit => h.on_exit.clone(),
    })
}

/// Create a tokio Command for a hook using the configured `general.shell`
/// setting (same shell used for daemon `run` commands). Returns an error
/// if the shell setting is empty or unparseable, matching daemon startup
/// validation — callers should log and skip the hook.
fn hook_command(cmd: &str) -> Result<tokio::process::Command> {
    let shell_setting = settings().general.shell.clone();
    match shell_words::split(&shell_setting) {
        Ok(parts) if !parts.is_empty() => {
            let (program, args) = parts.split_first().unwrap();
            let mut command = tokio::process::Command::new(program);
            command.args(args);
            command.arg(cmd);
            Ok(command)
        }
        Ok(_) => Err(miette::miette!(
            "general.shell setting is empty, cannot run hook"
        )),
        Err(e) => Err(miette::miette!(
            "failed to parse general.shell setting {shell_setting:?}: {e}"
        )),
    }
}

/// Fire a hook command as a fire-and-forget tokio task.
///
/// Reads the hook command from fresh config (`PitchforkToml::all_merged()`),
/// then spawns it in the background. Errors are logged but never block the caller.
///
/// The spawned task is also registered in `SUPERVISOR.hook_tasks` so that
/// supervisor shutdown (`close()`) can await all in-flight hooks before calling
/// `exit(0)`, ensuring hooks are not silently dropped during shutdown.
pub(crate) async fn fire_hook(
    hook_type: HookType,
    daemon_id: DaemonId,
    daemon_dir: PathBuf,
    retry_count: u32,
    daemon_env: Option<IndexMap<String, String>>,
    extra_env: Vec<(String, String)>,
) {
    let handle = tokio::spawn(async move {
        let pt = PitchforkToml::all_merged_all_namespaces().unwrap_or_else(|e| {
            warn!("Failed to load config for hook '{hook_type}': {e}");
            PitchforkToml::default()
        });
        let hook_cmd = pt
            .daemons
            .get(&daemon_id)
            .and_then(|d| get_hook_cmd(&d.hooks, &hook_type));

        let Some(cmd) = hook_cmd else { return };

        // Render Tera templates in hook command with context from state file
        let cmd = match render_hook_template(&cmd, &daemon_id, &pt).await {
            Ok(cmd) => cmd,
            Err(e) => {
                warn!("{hook_type} hook template error for daemon {daemon_id}: {e}");
                return;
            }
        };

        info!("firing {hook_type} hook for daemon {daemon_id}: {cmd}");

        let mut command = match hook_command(&cmd) {
            Ok(c) => c,
            Err(e) => {
                warn!("{hook_type} hook for daemon {daemon_id}: {e}");
                return;
            }
        };
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
            .env("PITCHFORK_DAEMON_ID", daemon_id.qualified())
            .env("PITCHFORK_DAEMON_NAMESPACE", daemon_id.namespace())
            .env("PITCHFORK_RETRY_COUNT", retry_count.to_string());

        for (key, value) in &extra_env {
            command.env(key, value);
        }

        match command.status().await {
            Ok(status) => {
                if !status.success() {
                    warn!("{hook_type} hook for daemon {daemon_id} exited with {status}");
                }
            }
            Err(e) => {
                error!("failed to execute {hook_type} hook for daemon {daemon_id}: {e}");
            }
        }
    });

    // Register the handle so supervisor shutdown can await it.
    // Use lock().await instead of try_lock() to guarantee registration — fire_hook
    // is called from async monitoring tasks (not latency-sensitive hot paths), so
    // awaiting the lock is safe and eliminates the silent-drop window where a
    // JoinHandle could be lost under lock contention.
    let mut tasks = SUPERVISOR.hook_tasks.lock().await;
    // Prune already-finished handles to avoid unbounded growth
    tasks.retain(|h| !h.is_finished());
    tasks.push(handle);
}

/// Fire the `on_output` hook for a daemon as a fire-and-forget task.
///
/// `cmd` is the hook command string resolved at call time (from `on_output.run`).
/// `matched_line` is exposed to the command via `PITCHFORK_MATCHED_LINE`.
pub(crate) async fn fire_output_hook(
    daemon_id: DaemonId,
    daemon_dir: PathBuf,
    retry_count: u32,
    daemon_env: Option<IndexMap<String, String>>,
    cmd: String,
    matched_line: String,
) {
    let handle = tokio::spawn(async move {
        // Render Tera templates in output hook command
        let pt = PitchforkToml::all_merged_all_namespaces().unwrap_or_default();
        let cmd = match render_hook_template(&cmd, &daemon_id, &pt).await {
            Ok(cmd) => cmd,
            Err(e) => {
                warn!("on_output hook template error for daemon {daemon_id}: {e}");
                return;
            }
        };

        info!("firing on_output hook for daemon {daemon_id}: {cmd}");

        let mut command = match hook_command(&cmd) {
            Ok(c) => c,
            Err(e) => {
                warn!("on_output hook for daemon {daemon_id}: {e}");
                return;
            }
        };
        command
            .current_dir(&daemon_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        if let Some(ref path) = *env::ORIGINAL_PATH {
            command.env("PATH", path);
        }

        if let Some(ref env_vars) = daemon_env {
            command.envs(env_vars);
        }

        command
            .env("PITCHFORK_DAEMON_ID", daemon_id.qualified())
            .env("PITCHFORK_DAEMON_NAMESPACE", daemon_id.namespace())
            .env("PITCHFORK_RETRY_COUNT", retry_count.to_string())
            .env("PITCHFORK_MATCHED_LINE", &matched_line);

        match command.status().await {
            Ok(status) => {
                if !status.success() {
                    warn!("on_output hook for daemon {daemon_id} exited with {status}");
                }
            }
            Err(e) => {
                error!("failed to execute on_output hook for daemon {daemon_id}: {e}");
            }
        }
    });

    let mut tasks = SUPERVISOR.hook_tasks.lock().await;
    tasks.retain(|h| !h.is_finished());
    tasks.push(handle);
}

/// Render Tera templates in a hook command using the current state file data.
///
/// In the supervisor, daemons are already running, so `resolved_port` and
/// `active_port` are available from the state file.
async fn render_hook_template(
    template_str: &str,
    daemon_id: &DaemonId,
    pt: &PitchforkToml,
) -> Result<String, template::RenderError> {
    // Collect resolved ports from all running daemons in the state file
    let resolved_daemons: HashMap<DaemonId, Vec<u16>> = {
        let state_file = SUPERVISOR.state_file.lock().await;
        state_file
            .daemons
            .iter()
            .filter_map(|(id, d)| {
                if d.resolved_port.is_empty() {
                    None
                } else {
                    Some((id.clone(), d.resolved_port.clone()))
                }
            })
            .collect()
    };

    let daemon_config = pt.daemons.get(daemon_id);
    let ctx = template::TemplateContext::new(
        daemon_id,
        daemon_config.unwrap_or(&Default::default()),
        &resolved_daemons,
        &pt.daemons,
    );
    template::render_template(template_str, &ctx)
}
