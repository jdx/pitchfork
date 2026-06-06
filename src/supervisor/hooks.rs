//! Hook and gate execution for daemon lifecycle events
//!
//! Hooks are fire-and-forget shell commands that run in response to daemon
//! lifecycle events (ready, fail, retry). They are configured in pitchfork.toml
//! under `[daemons.<name>.hooks]`.
//!
//! Gates block the lifecycle until the command exits with code 0. They are
//! configured under `[daemons.<name>.gates]`.

use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::{GateConfig, PitchforkToml};
use crate::settings::settings;
use crate::shell::Shell;
use crate::supervisor::SUPERVISOR;
use crate::{env, pitchfork_toml, template};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// HookType / GateType
// ---------------------------------------------------------------------------

#[allow(clippy::enum_variant_names)]
pub(crate) enum HookType {
    OnReady,
    OnFail,
    OnRetry,
    OnStop,
    OnExit,
}

pub(crate) enum GateType {
    PreStart,
    PostStart,
    PreStop,
    PostStop,
}

impl std::fmt::Display for GateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateType::PreStart => write!(f, "pre_start"),
            GateType::PostStart => write!(f, "post_start"),
            GateType::PreStop => write!(f, "pre_stop"),
            GateType::PostStop => write!(f, "post_stop"),
        }
    }
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

// ---------------------------------------------------------------------------
// GateContext — encapsulates all data needed to run a gate
// ---------------------------------------------------------------------------

pub(crate) struct GateContext {
    gate_type: GateType,
    daemon_id: DaemonId,
    daemon_dir: PathBuf,
    retry_count: u32,
    daemon_env: Option<IndexMap<String, String>>,
    extra_env: Vec<(String, String)>,
}

impl GateContext {
    pub(crate) fn new(
        gate_type: GateType,
        daemon_id: DaemonId,
        daemon_dir: PathBuf,
        retry_count: u32,
        daemon_env: Option<IndexMap<String, String>>,
    ) -> Self {
        Self {
            gate_type,
            daemon_id,
            daemon_dir,
            retry_count,
            daemon_env,
            extra_env: vec![],
        }
    }

    pub(crate) fn extra_env(mut self, env: Vec<(String, String)>) -> Self {
        self.extra_env = env;
        self
    }

    /// Run the gate, blocking until it succeeds or fails.
    ///
    /// Unlike hooks (fire-and-forget), gates block the lifecycle until the command
    /// exits with code 0. Returns `Ok(())` on success, `Err` on failure or timeout.
    ///
    /// If no gate is configured for the given type, returns `Ok(())` immediately.
    /// If the config file cannot be loaded, returns an error (gates must not be
    /// silently skipped — they are blocking by design).
    pub(crate) async fn run(self) -> crate::Result<()> {
        let gate_type = self.gate_type;
        let daemon_id = self.daemon_id;

        let pt = PitchforkToml::all_merged().map_err(|e| {
            miette::miette!("failed to load config for {gate_type} gate of daemon {daemon_id}: {e}")
        })?;

        let gate_config = pt
            .daemons
            .get(&daemon_id)
            .and_then(|d| get_gate_config(&d.gates, &gate_type));

        let Some(config) = gate_config else {
            return Ok(());
        };

        let cmd = match render_hook_template(&config.run, &daemon_id, &pt).await {
            Ok(cmd) => cmd,
            Err(e) => {
                return Err(miette::miette!(
                    "{gate_type} gate template error for daemon {daemon_id}: {e}"
                ));
            }
        };

        info!("running {gate_type} gate for daemon {daemon_id}: {cmd}");

        let mut command = Shell::default_for_platform().command(&cmd);
        command
            .current_dir(&self.daemon_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        if let Some(ref path) = *env::ORIGINAL_PATH {
            command.env("PATH", path);
        }

        if let Some(ref env_vars) = self.daemon_env {
            command.envs(env_vars);
        }

        command
            .env("PITCHFORK_DAEMON_ID", daemon_id.qualified())
            .env("PITCHFORK_DAEMON_NAMESPACE", daemon_id.namespace())
            .env("PITCHFORK_RETRY_COUNT", self.retry_count.to_string());

        for (key, value) in &self.extra_env {
            command.env(key, value);
        }

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Err(miette::miette!(
                    "failed to spawn {gate_type} gate for daemon {daemon_id}: {e}"
                ));
            }
        };

        // Effective timeout: per-gate timeout > global default > none
        // Duration::ZERO means "disable timeout" (wait indefinitely).
        let global_timeout = settings().supervisor_gate_timeout();
        let timeout = config.timeout_duration().or_else(|| {
            if global_timeout.is_zero() {
                None
            } else {
                Some(global_timeout)
            }
        });

        if let Some(timeout) = timeout {
            match tokio::time::timeout(timeout, child.wait()).await {
                Ok(Ok(status)) if status.success() => {
                    info!("{gate_type} gate for daemon {daemon_id} passed");
                    Ok(())
                }
                Ok(Ok(status)) => Err(miette::miette!(
                    "{gate_type} gate for daemon {daemon_id} exited with {status}"
                )),
                Ok(Err(e)) => Err(miette::miette!(
                    "{gate_type} gate for daemon {daemon_id} failed: {e}"
                )),
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    Err(miette::miette!(
                        "{gate_type} gate for daemon {daemon_id} timed out after {timeout:?}"
                    ))
                }
            }
        } else {
            match child.wait().await {
                Ok(status) if status.success() => {
                    info!("{gate_type} gate for daemon {daemon_id} passed");
                    Ok(())
                }
                Ok(status) => Err(miette::miette!(
                    "{gate_type} gate for daemon {daemon_id} exited with {status}"
                )),
                Err(e) => Err(miette::miette!(
                    "{gate_type} gate for daemon {daemon_id} failed: {e}"
                )),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

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

fn get_gate_config(
    gates: &Option<pitchfork_toml::PitchforkTomlGates>,
    gate_type: &GateType,
) -> Option<GateConfig> {
    gates.as_ref().and_then(|g| match gate_type {
        GateType::PreStart => g.pre_start.clone(),
        GateType::PostStart => g.post_start.clone(),
        GateType::PreStop => g.pre_stop.clone(),
        GateType::PostStop => g.post_stop.clone(),
    })
}

// ---------------------------------------------------------------------------
// run_post_stop_gate — convenience wrapper for the PostStop gate
// ---------------------------------------------------------------------------

/// Run the `post_stop` gate for a daemon with the given exit information.
///
/// `exit_code` and `exit_reason` are exposed to the gate command via
/// `PITCHFORK_EXIT_CODE` and `PITCHFORK_EXIT_REASON` environment variables.
pub(crate) async fn run_post_stop_gate(
    daemon_id: DaemonId,
    daemon_dir: PathBuf,
    retry_count: u32,
    daemon_env: Option<IndexMap<String, String>>,
    exit_code: i32,
    exit_reason: &str,
) -> crate::Result<()> {
    GateContext::new(
        GateType::PostStop,
        daemon_id,
        daemon_dir,
        retry_count,
        daemon_env,
    )
    .extra_env(vec![
        ("PITCHFORK_EXIT_CODE".to_string(), exit_code.to_string()),
        ("PITCHFORK_EXIT_REASON".to_string(), exit_reason.to_string()),
    ])
    .run()
    .await
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
    daemon_env: Option<IndexMap<String, String>>,
    extra_env: Vec<(String, String)>,
) {
    let handle = tokio::spawn(async move {
        let pt = PitchforkToml::all_merged().unwrap_or_else(|e| {
            warn!("Failed to load config for hook '{hook_type}': {e}");
            PitchforkToml::default()
        });
        let hook_cmd = pt
            .daemons
            .get(&daemon_id)
            .and_then(|d| get_hook_cmd(&d.hooks, &hook_type));

        let Some(cmd) = hook_cmd else { return };

        let cmd = match render_hook_template(&cmd, &daemon_id, &pt).await {
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
    daemon_env: Option<IndexMap<String, String>>,
    cmd: String,
    matched_line: String,
) {
    let handle = tokio::spawn(async move {
        let pt = PitchforkToml::all_merged().unwrap_or_default();
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

/// Render Tera templates in a hook/gate command using the current state file data.
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
