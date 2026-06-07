use axum::{extract::Path, response::Json};
use serde::Serialize;

use crate::daemon_list::{DaemonListEntry, get_all_daemons_direct, get_daemon_direct};
use crate::daemon_status::DaemonStatus;
use crate::procs::PROCS;
use crate::supervisor::SUPERVISOR;

/// Serializable daemon entry for the API
#[derive(Serialize)]
pub struct ApiDaemonEntry {
    id: ApiDaemonId,
    title: Option<String>,
    pid: Option<u32>,
    shell_pid: Option<u32>,
    status: ApiDaemonStatus,
    dir: Option<String>,
    autostop: bool,
    cron_schedule: Option<String>,
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

#[derive(Serialize)]
pub struct ApiDaemonId {
    namespace: String,
    name: String,
    qualified: String,
    safe_path: String,
}

#[derive(Serialize)]
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
    Stopped,
    #[serde(rename = "available")]
    Available,
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
    }
}

/// Convert a single `DaemonListEntry` into `ApiDaemonEntry`.
fn entry_to_api(
    entry: &DaemonListEntry,
    stats_map: &std::collections::HashMap<u32, crate::procs::ProcessStats>,
    global_slugs: &indexmap::IndexMap<String, crate::pitchfork_toml::SlugEntry>,
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
            let slug = crate::pitchfork_toml::PitchforkToml::find_slug_for_daemon_in_registry(
                &entry.id,
                global_slugs,
            );
            crate::proxy::build_proxy_url(slug.as_deref(), settings)
        } else {
            None
        },
        ready_delay: d.ready_delay,
        ready_output: d.ready_output.clone(),
        ready_http_url: d.ready_http.as_ref().map(|r| r.url.clone()),
        ready_port: d.ready_port,
        ready_cmd: d.ready_cmd.clone(),
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

async fn build_daemon_entries() -> crate::Result<Vec<ApiDaemonEntry>> {
    let entries = get_all_daemons_direct(&SUPERVISOR).await?;

    // Batch refresh process stats for all running daemons
    let pids: Vec<u32> = entries.iter().filter_map(|e| e.daemon.pid).collect();
    let stats_map = if !pids.is_empty() {
        PROCS.refresh_and_get_batch_stats(&pids)
    } else {
        std::collections::HashMap::new()
    };

    // Pre-load global slug registry once for proxy URL lookups
    let global_slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
    let settings = crate::settings::settings();

    Ok(entries
        .iter()
        .map(|e| entry_to_api(e, &stats_map, &global_slugs, &settings))
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

    // Fetch stats for just this daemon's PID (if running)
    let stats_map = if let Some(pid) = entry.daemon.pid {
        PROCS.refresh_and_get_batch_stats(&[pid])
    } else {
        std::collections::HashMap::new()
    };

    let global_slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
    let settings = crate::settings::settings();

    Ok(Json(entry_to_api(
        &entry,
        &stats_map,
        &global_slugs,
        &settings,
    )))
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
