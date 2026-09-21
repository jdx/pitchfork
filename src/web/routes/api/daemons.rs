use axum::{extract::Path, response::Json};
use serde::Serialize;

use crate::daemon_list::{DaemonListEntry, get_all_daemons_direct, get_daemon_direct};
use crate::daemon_status::DaemonStatus;
use crate::procs::PROCS;
use crate::supervisor::SUPERVISOR;

/// Serializable daemon entry for the API
#[derive(Serialize, Clone, Default)]
pub struct ApiDaemonEntry {
    id: ApiDaemonId,
    title: Option<String>,
    pid: Option<u32>,
    shell_pid: Option<u32>,
    status: ApiDaemonStatus,
    dir: Option<String>,
    autostop: bool,
    cron_schedule: Option<String>,
    /// When the cron watcher last started this daemon, RFC 3339. `null`
    /// until a scheduled run has actually happened.
    cron_last_run: Option<String>,
    /// Whether the run at `cron_last_run` finished successfully. `null` while
    /// it is still going: `last_exit_success` holds the previous run's result
    /// until the live one exits.
    cron_last_success: Option<bool>,
    /// The next time the schedule comes due, RFC 3339. A time in the past is
    /// a window missed while the supervisor was down.
    cron_next_run: Option<String>,
    last_exit_success: Option<bool>,
    retry_count: u32,
    resolved_port: Vec<u16>,
    active_port: Option<u16>,
    slug: Option<String>,
    is_disabled: bool,
    is_available: bool,
    command: Option<String>,
    cpu_percent: Option<f32>,
    memory_bytes: Option<u64>,
    uptime_secs: Option<u64>,
    proxy_url: Option<String>,
    ready_delay: Option<u64>,
    ready_output: Option<String>,
    ready_http_url: Option<String>,
    ready_port: Option<u16>,
    ready_cmd: Option<String>,
    health_cmd: Option<String>,
    health_http_url: Option<String>,
    health_port: Option<u16>,
    port_config: Option<String>,
    depends: Vec<String>,
    env: Option<Vec<String>>,
    watch: Vec<String>,
    watch_mode: String,
    watch_base_dir: Option<String>,
    mise: Option<bool>,
    user: Option<String>,
    memory_limit: Option<String>,
    cpu_limit: Option<String>,
    stop_signal: Option<String>,
    stop_timeout: Option<String>,
    pty: Option<bool>,
    proxy: Option<bool>,
}

#[derive(Serialize, Clone, Default)]
pub struct ApiDaemonId {
    namespace: String,
    name: String,
    qualified: String,
    safe_path: String,
}

#[derive(Serialize, Clone, Default)]
#[serde(tag = "type")]
pub enum ApiDaemonStatus {
    #[serde(rename = "failed")]
    Failed { message: String },
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "stopping")]
    Stopping,
    #[serde(rename = "errored")]
    Errored { code: i32 },
    #[serde(rename = "stopped")]
    #[default]
    Stopped,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "available")]
    Available,
}

impl ApiDaemonEntry {
    /// Namespace of the daemon, as used to scope a worktree's stack.
    pub(crate) fn namespace(&self) -> &str {
        &self.id.namespace
    }

    /// Fully qualified `namespace/name` id.
    pub(crate) fn qualified(&self) -> &str {
        &self.id.qualified
    }

    /// Status discriminant, for counting daemons by state.
    pub(crate) fn status_kind(&self) -> &'static str {
        match self.status {
            ApiDaemonStatus::Failed { .. } => "failed",
            ApiDaemonStatus::Waiting => "waiting",
            ApiDaemonStatus::Running => "running",
            ApiDaemonStatus::Stopping => "stopping",
            ApiDaemonStatus::Errored { .. } => "errored",
            ApiDaemonStatus::Stopped => "stopped",
            ApiDaemonStatus::Completed => "completed",
            ApiDaemonStatus::Available => "available",
        }
    }

    /// Seconds the daemon's process has been up, when it is running.
    pub(crate) fn uptime_secs(&self) -> Option<u64> {
        self.uptime_secs
    }

    #[cfg(test)]
    pub(crate) fn stub(qualified: &str, status: ApiDaemonStatus, uptime_secs: Option<u64>) -> Self {
        let id = crate::daemon_id::DaemonId::parse(qualified).expect("valid daemon id");
        Self {
            is_available: matches!(status, ApiDaemonStatus::Available),
            id: api_id(&id),
            status,
            uptime_secs,
            ..Default::default()
        }
    }
}

fn api_id(id: &crate::daemon_id::DaemonId) -> ApiDaemonId {
    ApiDaemonId {
        namespace: id.namespace().to_string(),
        name: id.name().to_string(),
        qualified: id.qualified(),
        safe_path: id.safe_path(),
    }
}

fn api_status(status: &DaemonStatus, is_available: bool) -> ApiDaemonStatus {
    if is_available {
        return ApiDaemonStatus::Available;
    }
    match status {
        DaemonStatus::Failed(msg) => ApiDaemonStatus::Failed {
            message: msg.clone(),
        },
        DaemonStatus::Waiting => ApiDaemonStatus::Waiting,
        DaemonStatus::Running => ApiDaemonStatus::Running,
        DaemonStatus::Stopping => ApiDaemonStatus::Stopping,
        DaemonStatus::Errored(code) => ApiDaemonStatus::Errored { code: *code },
        DaemonStatus::Stopped => ApiDaemonStatus::Stopped,
        DaemonStatus::Completed => ApiDaemonStatus::Completed,
    }
}

/// Convert a single `DaemonListEntry` into `ApiDaemonEntry`.
fn entry_to_api(
    entry: &DaemonListEntry,
    stats_map: &std::collections::HashMap<u32, crate::procs::ProcessStats>,
    hosts: &std::collections::HashMap<crate::daemon_id::DaemonId, String>,
    settings: &crate::settings::Settings,
) -> ApiDaemonEntry {
    let d = &entry.daemon;
    let cmd = d.cmd.as_ref().map(|c| c.join(" "));
    let (cpu, mem, uptime) = d
        .pid
        .and_then(|pid| stats_map.get(&pid))
        .map(|s| {
            (
                Some(s.cpu_percent),
                Some(s.memory_bytes),
                Some(s.uptime_secs),
            )
        })
        .unwrap_or((None, None, None));

    ApiDaemonEntry {
        id: api_id(&entry.id),
        title: d.title.clone(),
        pid: d.pid,
        shell_pid: d.shell_pid,
        status: api_status(&d.status, entry.is_available),
        dir: d.dir.as_ref().map(|p| p.to_string_lossy().to_string()),
        autostop: d.autostop,
        cron_schedule: d.cron_schedule.clone(),
        cron_last_run: d.last_cron_run.map(|t| t.to_rfc3339()),
        cron_last_success: d
            .last_cron_run
            .filter(|_| !d.status.is_running())
            .and(d.last_exit_success),
        cron_next_run: d
            .next_cron_run(chrono::Local::now())
            .map(|t| t.to_rfc3339()),
        last_exit_success: d.last_exit_success,
        retry_count: d.retry_count,
        resolved_port: if d.status.is_running() {
            d.resolved_port.clone()
        } else {
            Vec::new()
        },
        active_port: if d.status.is_running() {
            d.active_port
        } else {
            None
        },
        slug: d.slug.clone(),
        is_disabled: entry.is_disabled,
        is_available: entry.is_available,
        command: cmd,
        cpu_percent: cpu,
        memory_bytes: mem,
        uptime_secs: uptime,
        proxy_url: if d.status.is_running() {
            crate::proxy::build_proxy_url(hosts.get(&entry.id).map(String::as_str), settings)
        } else {
            None
        },
        ready_delay: d.ready_delay,
        ready_output: d.ready_output.as_ref().map(|o| o.pattern.clone()),
        ready_http_url: d.ready_http.as_ref().map(|r| r.url.clone()),
        ready_port: d.ready_port.as_ref().and_then(|p| p.as_port()),
        ready_cmd: d.ready_cmd.as_ref().map(|r| r.run.clone()),
        health_cmd: d.health_cmd.as_ref().map(|c| c.run.clone()),
        health_http_url: d.health_http.as_ref().map(|h| h.url.clone()),
        health_port: d.health_port.as_ref().and_then(|p| p.as_port()),
        port_config: d.port.as_ref().map(|p| {
            if p.bump.0 == 0 {
                p.expect
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                format!(
                    "{}+{} bumps",
                    p.expect.first().map(|e| e.to_string()).unwrap_or_default(),
                    p.bump.0
                )
            }
        }),
        depends: d.depends.iter().map(|id| id.qualified()).collect(),
        env: d
            .env
            .as_ref()
            .map(|m| m.keys().cloned().collect::<Vec<_>>()),
        watch: d.watch.clone(),
        watch_mode: format!("{:?}", d.watch_mode).to_lowercase(),
        watch_base_dir: d
            .watch_base_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        mise: d.mise,
        user: d.user.clone(),
        memory_limit: d.memory_limit.map(|m| m.to_string()),
        cpu_limit: d.cpu_limit.map(|c| format!("{:.1}%", c.0)),
        stop_signal: d.stop_signal.map(|s| s.signal.name().to_string()),
        stop_timeout: d
            .stop_signal
            .and_then(|s| s.timeout.map(|d| humantime::format_duration(d).to_string())),
        pty: d.pty,
        proxy: d.proxy,
    }
}

/// Build an API entry for a daemon that only exists in config, never started.
pub(crate) fn config_daemon_entry(
    id: &crate::daemon_id::DaemonId,
    daemon_config: &crate::pitchfork_toml::PitchforkTomlDaemon,
    hosts: &std::collections::HashMap<crate::daemon_id::DaemonId, String>,
    settings: &crate::settings::Settings,
    is_disabled: bool,
) -> ApiDaemonEntry {
    let entry = DaemonListEntry {
        id: id.clone(),
        daemon: crate::daemon_list::build_placeholder_daemon(id, daemon_config),
        is_disabled,
        is_available: true,
    };
    entry_to_api(&entry, &std::collections::HashMap::new(), hosts, settings)
}

/// The proxy hostname each of these config-only daemons would have.
pub(crate) fn config_proxy_hosts(
    daemons: &[(
        crate::daemon_id::DaemonId,
        crate::pitchfork_toml::PitchforkTomlDaemon,
    )],
) -> std::collections::HashMap<crate::daemon_id::DaemonId, String> {
    if !crate::settings::settings().proxy.enable {
        return std::collections::HashMap::new();
    }
    let global_slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
    daemons
        .iter()
        .filter_map(|(id, config)| {
            let host = crate::proxy::hostname::host_for_daemon(id, Some(config), &global_slugs)?;
            Some((id.clone(), host))
        })
        .collect()
}

/// Live state of every daemon the supervisor knows about, as API entries.
pub(crate) async fn build_api_daemons() -> crate::Result<Vec<ApiDaemonEntry>> {
    build_daemon_entries().await
}

/// The proxy hostname of each daemon, resolved once on a blocking worker.
///
/// Reading the slug registry and every project's configuration is file I/O, and
/// deriving a hostname walks the project's checkouts, so it must not run on the
/// thread serving the request.
async fn proxy_hosts_for(
    ids: Vec<crate::daemon_id::DaemonId>,
) -> std::collections::HashMap<crate::daemon_id::DaemonId, String> {
    if !crate::settings::settings().proxy.enable {
        return std::collections::HashMap::new();
    }
    tokio::task::spawn_blocking(move || {
        let global_slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
        let config = crate::pitchfork_toml::PitchforkToml::all_merged_all_namespaces().ok();
        ids.into_iter()
            .filter_map(|id| {
                let host = crate::proxy::hostname::host_for_daemon(
                    &id,
                    config.as_ref().and_then(|pt| pt.daemons.get(&id)),
                    &global_slugs,
                )?;
                Some((id, host))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

async fn build_daemon_entries() -> crate::Result<Vec<ApiDaemonEntry>> {
    let entries = get_all_daemons_direct(&SUPERVISOR).await?;

    // Batch refresh process stats for all running daemons.
    // The full /proc scan is throttled (refresh_if_stale) and runs on a
    // blocking worker so the async handler never blocks the executor.
    let pids: Vec<u32> = entries.iter().filter_map(|e| e.daemon.pid).collect();
    let stats_map = if !pids.is_empty() {
        tokio::task::spawn_blocking(move || PROCS.refresh_and_get_batch_stats_if_stale(&pids))
            .await
            .map_err(|e| miette::miette!("process stats refresh failed: {e}"))?
    } else {
        std::collections::HashMap::new()
    };

    let hosts = proxy_hosts_for(entries.iter().map(|e| e.id.clone()).collect()).await;
    let settings = crate::settings::settings();

    Ok(entries
        .iter()
        .map(|e| entry_to_api(e, &stats_map, &hosts, &settings))
        .collect())
}

pub async fn list() -> Result<Json<Vec<ApiDaemonEntry>>, axum::http::StatusCode> {
    let entries = build_daemon_entries().await.map_err(|e| {
        log::error!("Failed to list daemons: {e}");
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(entries))
}

pub async fn show(Path(id): Path<String>) -> Result<Json<ApiDaemonEntry>, axum::http::StatusCode> {
    let daemon_id =
        crate::daemon_id::DaemonId::parse(&id).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    let entry = get_daemon_direct(&SUPERVISOR, &daemon_id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    // Fetch stats for just this daemon's PID (if running).
    // Throttled refresh on a blocking worker; see build_daemon_entries.
    let stats_map = if let Some(pid) = entry.daemon.pid {
        tokio::task::spawn_blocking(move || PROCS.refresh_and_get_batch_stats_if_stale(&[pid]))
            .await
            .map_err(|e| {
                log::error!("Failed to refresh process stats: {e}");
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            })?
    } else {
        std::collections::HashMap::new()
    };

    let hosts = proxy_hosts_for(vec![entry.id.clone()]).await;
    let settings = crate::settings::settings();

    Ok(Json(entry_to_api(&entry, &stats_map, &hosts, &settings)))
}

pub async fn start(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let daemon_id =
        crate::daemon_id::DaemonId::parse(&id).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    let client = crate::ipc::client::IpcClient::connect(true)
        .await
        .map_err(|e| {
            log::error!("Failed to connect to IPC: {e}");
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        })?;

    match client.start_daemon(&daemon_id, None).await {
        Ok(result) => {
            let mut json = serde_json::json!({"ok": result.started});
            if let Some(msg) = result.error_message {
                json["error"] = serde_json::Value::String(msg);
            } else if !result.started {
                // Already in the requested state: callers acting on a group
                // treat this as a no-op rather than a failed member.
                json["noop"] = serde_json::Value::Bool(true);
                json["error"] = serde_json::Value::String("daemon is already running".into());
            }
            Ok(Json(json))
        }
        Err(e) => {
            log::error!("Failed to start daemon: {e}");
            Ok(Json(serde_json::json!({
                "ok": false,
                "error": e.to_string(),
            })))
        }
    }
}

pub async fn stop(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let daemon_id =
        crate::daemon_id::DaemonId::parse(&id).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    let client = crate::ipc::client::IpcClient::connect(true)
        .await
        .map_err(|e| {
            log::error!("Failed to connect to IPC: {e}");
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        })?;

    match client.stop(daemon_id.clone()).await {
        Ok(true) => Ok(Json(serde_json::json!({
            "ok": true,
        }))),
        Ok(false) => Ok(Json(serde_json::json!({
            "ok": false,
            "noop": true,
            "error": "daemon is not running",
        }))),
        Err(e) => {
            log::error!("Failed to stop daemon: {e}");
            Ok(Json(serde_json::json!({
                "ok": false,
                "error": e.to_string(),
            })))
        }
    }
}

pub async fn restart(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let daemon_id =
        crate::daemon_id::DaemonId::parse(&id).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    let client = crate::ipc::client::IpcClient::connect(true)
        .await
        .map_err(|e| {
            log::error!("Failed to connect to IPC: {e}");
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        })?;

    match client.restart_daemon(&daemon_id, None).await {
        Ok(result) => {
            let mut json = serde_json::json!({"ok": result.started});
            if let Some(msg) = result.error_message {
                json["error"] = serde_json::Value::String(msg);
            }
            Ok(Json(json))
        }
        Err(e) => {
            log::error!("Failed to restart daemon: {e}");
            Ok(Json(serde_json::json!({
                "ok": false,
                "error": e.to_string(),
            })))
        }
    }
}

pub async fn enable(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let daemon_id =
        crate::daemon_id::DaemonId::parse(&id).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    let client = crate::ipc::client::IpcClient::connect(true)
        .await
        .map_err(|e| {
            log::error!("Failed to connect to IPC: {e}");
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        })?;

    match client.enable(daemon_id.clone()).await {
        Ok(true) => Ok(Json(serde_json::json!({
            "ok": true,
        }))),
        Ok(false) => Ok(Json(serde_json::json!({
            "ok": false,
            "noop": true,
            "error": "daemon is already enabled",
        }))),
        Err(e) => {
            log::error!("Failed to enable daemon: {e}");
            Ok(Json(serde_json::json!({
                "ok": false,
                "error": e.to_string(),
            })))
        }
    }
}

pub async fn disable(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let daemon_id =
        crate::daemon_id::DaemonId::parse(&id).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    let client = crate::ipc::client::IpcClient::connect(true)
        .await
        .map_err(|e| {
            log::error!("Failed to connect to IPC: {e}");
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        })?;

    match client.disable(daemon_id.clone()).await {
        Ok(true) => Ok(Json(serde_json::json!({
            "ok": true,
        }))),
        Ok(false) => Ok(Json(serde_json::json!({
            "ok": false,
            "noop": true,
            "error": "daemon is already disabled",
        }))),
        Err(e) => {
            log::error!("Failed to disable daemon: {e}");
            Ok(Json(serde_json::json!({
                "ok": false,
                "error": e.to_string(),
            })))
        }
    }
}
