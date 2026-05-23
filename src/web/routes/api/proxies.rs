use axum::response::Json;
use serde::Serialize;
use std::collections::HashMap;

use crate::procs::PROCS;
use crate::supervisor::SUPERVISOR;

#[derive(Serialize)]
pub struct ApiProxyWorktreeEntry {
    slug: String,
    daemon_name: String,
    branch: String,
    sanitized_branch: String,
    namespace: Option<String>,
    path: String,
    port: Option<u16>,
    status: Option<String>,
    pid: Option<u32>,
    proxy_url: Option<String>,
    daemon_qualified: String,
    uptime_secs: Option<u64>,
    error: Option<String>,
}

pub async fn list() -> Json<Vec<ApiProxyWorktreeEntry>> {
    let settings = crate::settings::settings();

    // Use the shared slug cache (populated by proxy server) to avoid
    // spawning subprocesses for worktree discovery on every request.
    let cached = crate::proxy::server::get_cached_slugs().await;

    // Still need namespaces for registration checks; this reads the same
    // global config file and is fast (no subprocesses).
    let all_namespaces = crate::pitchfork_toml::PitchforkToml::read_global_namespaces();

    #[allow(clippy::type_complexity)]
    let daemon_state: HashMap<String, (Option<u16>, String, Option<u32>, Option<u64>)> = {
        let state_file = SUPERVISOR.state_file.lock().await;
        state_file
            .daemons
            .iter()
            .map(|(id, d)| {
                let port = d.active_port.or_else(|| d.resolved_port.first().copied());
                let key = format!("{}/{}", id.namespace(), id.name());
                let uptime = d
                    .pid
                    .and_then(|pid| PROCS.get_stats(pid))
                    .map(|s| s.uptime_secs);
                (key, (port, d.status.to_string(), d.pid, uptime))
            })
            .collect()
    };

    let mut entries = Vec::new();

    for (slug, cached_entry) in cached.iter() {
        let ns_name = cached_entry.namespace.clone().unwrap_or_default();
        let is_registered = ns_name.is_empty() || all_namespaces.contains_key(&ns_name);
        let daemon_name = cached_entry.daemon_name.clone();
        let lookup_key = format!(
            "{}/{}",
            if ns_name.is_empty() {
                "global"
            } else {
                &ns_name
            },
            daemon_name
        );

        let (port, status, pid, uptime) = if !is_registered {
            // Namespace not registered - show unconfigured
            (None, Some("unconfigured".to_string()), None, None)
        } else {
            daemon_state
                .get(&lookup_key)
                .map(|(p, s, pid, up)| {
                    let status = Some(s.clone());
                    let is_running = status.as_deref() == Some("running");
                    (if is_running { *p } else { None }, status, *pid, *up)
                })
                .unwrap_or((None, Some("available".to_string()), None, None))
        };

        let error = if !is_registered {
            Some(format!(
                "Namespace '{}' is not registered. Run 'pitchfork supervisor namespace add {} /path/to/dir'",
                ns_name, ns_name
            ))
        } else {
            None
        };

        let ns = if ns_name.is_empty() {
            None
        } else {
            Some(ns_name.clone())
        };
        let proxy_url = crate::proxy::build_proxy_url(Some(slug), settings);
        let daemon_qualified = lookup_key;

        let wts = &cached_entry.worktrees;
        if wts.is_empty() {
            entries.push(ApiProxyWorktreeEntry {
                slug: slug.clone(),
                daemon_name: daemon_name.clone(),
                branch: "default".to_string(),
                sanitized_branch: "default".to_string(),
                namespace: ns.clone(),
                path: cached_entry.dir.to_string_lossy().to_string(),
                port,
                status,
                pid,
                proxy_url,
                daemon_qualified: daemon_qualified.clone(),
                uptime_secs: uptime,
                error: error.clone(),
            });
        } else {
            for wt in wts {
                entries.push(ApiProxyWorktreeEntry {
                    slug: slug.clone(),
                    daemon_name: daemon_name.clone(),
                    branch: wt.branch.clone(),
                    sanitized_branch: wt.sanitized_branch.clone(),
                    namespace: wt.namespace.clone(),
                    path: wt.path.to_string_lossy().to_string(),
                    port,
                    status: status.clone(),
                    pid,
                    proxy_url: proxy_url.clone(),
                    daemon_qualified: daemon_qualified.clone(),
                    uptime_secs: uptime,
                    error: error.clone(),
                });
            }
        }
    }

    Json(entries)
}
