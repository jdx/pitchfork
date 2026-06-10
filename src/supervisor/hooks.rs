//! Hook execution for daemon lifecycle events
//!
//! Hooks are shell commands that run in response to daemon lifecycle events.
//! They are configured in pitchfork.toml under `[daemons.<name>.hooks]`.
//!
//! Each hook can be:
//! - **Fire-and-forget** (`block = false`, default): runs in background, errors are logged
//! - **Blocking** (`block = true`): lifecycle pauses until the command exits with code 0
//!
//! ## Block support by hook type
//!
//! | Hook | `block = true` supported? | Notes |
//! |------|---------------------------|-------|
//! | `pre_start` | Yes | Blocks `run()`, failure returns error |
//! | `on_ready` | Yes | Blocks `run()` during wait_ready; fire-and-forget otherwise |
//! | `pre_stop` | Yes | Blocks `stop()`, failure returns error |
//! | `on_stop` | Yes (explicit stop only) | Blocks `stop()`; fire-and-forget in monitor task |
//! | `on_exit` | No | Always fire-and-forget |
//! | `on_fail` | No | Always fire-and-forget |
//! | `on_retry` | Yes (startup only) | Blocks during startup retry loop; fire-and-forget in interval watcher |
//! | `on_crash` | No | Always fire-and-forget (runtime only) |
//! | `on_recover` | No | Always fire-and-forget (runtime only) |
//!
//! Hooks that run in the monitor task or interval watcher cannot block because
//! doing so would stall those background loops. This is a known limitation —
//! see the lifecycle hooks guide for details.
//!
//! ## on_ready hook behavior
//!
//! The `on_ready` hook fires exactly once, in the monitor task:
//! - When `block = true`: the hook runs **before** the ready signal is sent to
//!   `run_once()`, so `pitchfork start` blocks until the hook completes.
//! - When `block = false`: the hook is fire-and-forget; the ready signal is sent
//!   immediately and the hook runs concurrently.
//! - For daemons with no readiness config, `on_ready` fires immediately after
//!   process spawn.

use crate::config_types::HookConfig;
use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::settings;
use crate::shell::Shell;
use crate::supervisor::SUPERVISOR;
use crate::{env, pitchfork_toml, template};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// HookType
// ---------------------------------------------------------------------------

#[allow(clippy::enum_variant_names)]
pub(crate) enum HookType {
    PreStart,
    OnReady,
    PreStop,
    OnStop,
    OnExit,
    OnFail,
    OnRetry,
    OnCrash,
    OnRecover,
}

impl std::fmt::Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookType::PreStart => write!(f, "pre_start"),
            HookType::OnReady => write!(f, "on_ready"),
            HookType::PreStop => write!(f, "pre_stop"),
            HookType::OnStop => write!(f, "on_stop"),
            HookType::OnExit => write!(f, "on_exit"),
            HookType::OnFail => write!(f, "on_fail"),
            HookType::OnRetry => write!(f, "on_retry"),
            HookType::OnCrash => write!(f, "on_crash"),
            HookType::OnRecover => write!(f, "on_recover"),
        }
    }
}

fn get_hook_config<'a>(
    hooks: &'a Option<pitchfork_toml::PitchforkTomlHooks>,
    hook_type: &HookType,
) -> Option<&'a HookConfig> {
    hooks.as_ref().and_then(|h| match hook_type {
        HookType::PreStart => h.pre_start.as_ref(),
        HookType::OnReady => h.on_ready.as_ref(),
        HookType::PreStop => h.pre_stop.as_ref(),
        HookType::OnStop => h.on_stop.as_ref(),
        HookType::OnExit => h.on_exit.as_ref(),
        HookType::OnFail => h.on_fail.as_ref(),
        HookType::OnRetry => h.on_retry.as_ref(),
        HookType::OnCrash => h.on_crash.as_ref(),
        HookType::OnRecover => h.on_recover.as_ref(),
    })
}

// ---------------------------------------------------------------------------
// run_hook — blocking hook execution
// ---------------------------------------------------------------------------

/// Run a hook, respecting the `block` config.
///
/// For `block = true` hooks, the lifecycle pauses until the command exits with
/// code 0. Returns `Ok(())` on success, `Err` on failure or timeout.
///
/// For `block = false` hooks, this is equivalent to `fire_hook` — the command
/// is spawned in the background and errors are logged.
///
/// **Note:** `on_exit` does not support `block = true` — it is always
/// fire-and-forget. Callers should use `fire_hook` directly for `on_exit`.
///
/// If no hook is configured for the given type, returns `Ok(())` immediately.
/// If the config file cannot be loaded, returns an error (blocking hooks must
/// not be silently skipped).
pub(crate) async fn run_hook(
    hook_type: HookType,
    daemon_id: DaemonId,
    daemon_dir: PathBuf,
    retry_count: u32,
    recovery_count: u32,
    daemon_env: Option<IndexMap<String, String>>,
    extra_env: Vec<(String, String)>,
) -> crate::Result<()> {
    let pt = PitchforkToml::all_merged_all_namespaces().map_err(|e| {
        miette::miette!("failed to load config for {hook_type} hook of daemon {daemon_id}: {e}")
    })?;

    let hook_config = pt
        .daemons
        .get(&daemon_id)
        .and_then(|d| get_hook_config(&d.hooks, &hook_type));

    let Some(config) = hook_config else {
        return Ok(());
    };

    if config.block {
        run_blocking_hook(
            &hook_type,
            &daemon_id,
            &daemon_dir,
            retry_count,
            recovery_count,
            &daemon_env,
            &extra_env,
            config,
        )
        .await
    } else {
        fire_hook(
            hook_type,
            daemon_id,
            daemon_dir,
            retry_count,
            recovery_count,
            daemon_env,
            extra_env,
        )
        .await;
        Ok(())
    }
}

/// Internal: run a blocking hook command, waiting for it to succeed.
#[allow(clippy::too_many_arguments)]
async fn run_blocking_hook(
    hook_type: &HookType,
    daemon_id: &DaemonId,
    daemon_dir: &PathBuf,
    retry_count: u32,
    recovery_count: u32,
    daemon_env: &Option<IndexMap<String, String>>,
    extra_env: &[(String, String)],
    config: &HookConfig,
) -> crate::Result<()> {
    let pt = PitchforkToml::all_merged_all_namespaces().unwrap_or_default();
    let cmd = match render_hook_template(&config.run, daemon_id, &pt).await {
        Ok(cmd) => cmd,
        Err(e) => {
            return Err(miette::miette!(
                "{hook_type} hook template error for daemon {daemon_id}: {e}"
            ));
        }
    };

    info!("running {hook_type} hook for daemon {daemon_id}: {cmd}");

    let mut command = Shell::default_for_platform().command(&cmd);
    command
        .current_dir(daemon_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    if let Some(ref path) = *env::ORIGINAL_PATH {
        command.env("PATH", path);
    }

    if let Some(env_vars) = &daemon_env {
        command.envs(env_vars);
    }

    command
        .env("PITCHFORK_DAEMON_ID", daemon_id.qualified())
        .env("PITCHFORK_DAEMON_NAMESPACE", daemon_id.namespace())
        .env("PITCHFORK_RETRY_COUNT", retry_count.to_string())
        .env("PITCHFORK_RECOVERY_COUNT", recovery_count.to_string());

    for (key, value) in extra_env {
        command.env(key, value);
    }

    // Ensure the subprocess is killed if the future is dropped before completion
    // (e.g. supervisor shutdown). Without this, dropping the Child does not send
    // a signal, leaving an orphan process.
    command.kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Err(miette::miette!(
                "failed to spawn {hook_type} hook for daemon {daemon_id}: {e}"
            ));
        }
    };

    // Capture stderr in the background so we can include it in error messages.
    let stderr_handle = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        if let Some(stderr) = stderr_handle {
            let mut buf = Vec::new();
            use tokio::io::AsyncReadExt;
            let _ = tokio::io::BufReader::new(stderr)
                .read_to_end(&mut buf)
                .await;
            String::from_utf8_lossy(&buf).trim().to_string()
        } else {
            String::new()
        }
    });

    let global_timeout = settings().supervisor_hook_block_timeout();
    let timeout = config.timeout_duration().or_else(|| {
        if global_timeout.is_zero() {
            None
        } else {
            Some(global_timeout)
        }
    });

    let wait_result = if let Some(timeout) = timeout {
        match tokio::time::timeout(timeout, child.wait()).await {
            Ok(result) => result,
            Err(_) => {
                // Timeout: kill the process and wait for it to exit.
                // kill_on_drop(true) ensures the process is killed even if
                // kill() fails, but we still try explicitly for a clean shutdown.
                let _ = child.kill().await;
                let _ = child.wait().await;
                let stderr_output = stderr_task.await.unwrap_or_default();
                return Err(miette::miette!(
                    "{hook_type} hook for daemon {daemon_id} timed out after {timeout:?}{stderr_suffix}",
                    stderr_suffix = if stderr_output.is_empty() {
                        String::new()
                    } else {
                        format!(": {stderr_output}")
                    }
                ));
            }
        }
    } else {
        child.wait().await
    };

    let stderr_output = stderr_task.await.unwrap_or_default();

    match wait_result {
        Ok(status) if status.success() => {
            info!("{hook_type} hook for daemon {daemon_id} passed");
            Ok(())
        }
        Ok(status) => {
            if stderr_output.is_empty() {
                Err(miette::miette!(
                    "{hook_type} hook for daemon {daemon_id} exited with {status}"
                ))
            } else {
                Err(miette::miette!(
                    "{hook_type} hook for daemon {daemon_id} exited with {status}: {stderr_output}"
                ))
            }
        }
        Err(e) => Err(miette::miette!(
            "{hook_type} hook for daemon {daemon_id} failed: {e}"
        )),
    }
}

// ---------------------------------------------------------------------------
// fire_hook — fire-and-forget lifecycle hook
// ---------------------------------------------------------------------------

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
    recovery_count: u32,
    daemon_env: Option<IndexMap<String, String>>,
    extra_env: Vec<(String, String)>,
) {
    let handle = tokio::spawn(async move {
        let pt = PitchforkToml::all_merged_all_namespaces().unwrap_or_else(|e| {
            warn!("Failed to load config for hook '{hook_type}': {e}");
            PitchforkToml::default()
        });
        let hook_config = pt
            .daemons
            .get(&daemon_id)
            .and_then(|d| get_hook_config(&d.hooks, &hook_type));

        let Some(config) = hook_config else { return };

        let cmd = match render_hook_template(&config.run, &daemon_id, &pt).await {
            Ok(cmd) => cmd,
            Err(e) => {
                warn!("{hook_type} hook template error for daemon {daemon_id}: {e}");
                return;
            }
        };

        info!("firing {hook_type} hook for daemon {daemon_id}: {cmd}");

        let mut command = Shell::default_for_platform().command(&cmd);
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
            .env("PITCHFORK_RECOVERY_COUNT", recovery_count.to_string());

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

    let mut tasks = SUPERVISOR.hook_tasks.lock().await;
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
    recovery_count: u32,
    daemon_env: Option<IndexMap<String, String>>,
    cmd: String,
    matched_line: String,
) {
    let handle = tokio::spawn(async move {
        let pt = PitchforkToml::all_merged_all_namespaces().unwrap_or_default();
        let cmd = match render_hook_template(&cmd, &daemon_id, &pt).await {
            Ok(cmd) => cmd,
            Err(e) => {
                warn!("on_output hook template error for daemon {daemon_id}: {e}");
                return;
            }
        };

        info!("firing on_output hook for daemon {daemon_id}: {cmd}");

        let mut command = Shell::default_for_platform().command(&cmd);
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
            .env("PITCHFORK_RECOVERY_COUNT", recovery_count.to_string())
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

// ---------------------------------------------------------------------------
// Template rendering
// ---------------------------------------------------------------------------

/// Render Tera templates in a hook command using the current state file data.
///
/// In the supervisor, daemons are already running, so `resolved_port` and
/// `active_port` are available from the state file.
async fn render_hook_template(
    template_str: &str,
    daemon_id: &DaemonId,
    pt: &PitchforkToml,
) -> Result<String, template::RenderError> {
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
