use axum::{extract::Path, response::Json};
use serde::Serialize;

use crate::procs::PROCS;
use crate::supervisor::SUPERVISOR;

#[derive(Serialize)]
pub struct ApiProcessTree {
    pid: u32,
    name: String,
    exe: Option<String>,
    cpu_percent: f32,
    memory_bytes: u64,
    virtual_memory_bytes: u64,
    uptime_secs: u64,
    thread_count: usize,
    status: String,
    children: Vec<ApiProcessTree>,
}

pub async fn tree(
    Path(id): Path<String>,
) -> Result<Json<Vec<ApiProcessTree>>, axum::http::StatusCode> {
    let daemon_id =
        crate::daemon_id::DaemonId::parse(&id).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    let pid = {
        let state_file = SUPERVISOR.state_file.lock().await;
        state_file.daemons.get(&daemon_id).and_then(|d| d.pid)
    };

    let Some(root_pid) = pid else {
        return Ok(Json(vec![]));
    };

    PROCS.refresh_processes();
    let tree = build_tree(root_pid);
    Ok(Json(tree))
}

/// Build the process tree recursively with cycle detection.
///
/// Uses a shared `visited` set to guard against infinite recursion caused
/// by PID reuse or malformed parent pointers.
fn build_tree(root_pid: u32) -> Vec<ApiProcessTree> {
    let (parent_map, process_names) = PROCS.collect_process_tree_info();
    let mut visited = std::collections::HashSet::new();
    build_tree_recursive(root_pid, &parent_map, &process_names, &mut visited)
}

fn build_tree_recursive(
    pid: u32,
    parent_map: &std::collections::HashMap<u32, Vec<u32>>,
    process_names: &std::collections::HashMap<u32, (String, Option<String>)>,
    visited: &mut std::collections::HashSet<u32>,
) -> Vec<ApiProcessTree> {
    if !visited.insert(pid) {
        return vec![];
    }

    let stats = match PROCS.get_extended_stats(pid) {
        Some(s) => s,
        None => return vec![],
    };

    let children: Vec<ApiProcessTree> = parent_map
        .get(&pid)
        .map(|kids| {
            kids.iter()
                .flat_map(|&child| build_tree_recursive(child, parent_map, process_names, visited))
                .collect()
        })
        .unwrap_or_default();

    vec![ApiProcessTree {
        pid,
        name: process_names
            .get(&pid)
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| stats.name.clone()),
        exe: process_names.get(&pid).and_then(|(_, e)| e.clone()),
        cpu_percent: stats.cpu_percent,
        memory_bytes: stats.memory_bytes,
        virtual_memory_bytes: stats.virtual_memory_bytes,
        uptime_secs: stats.uptime_secs,
        thread_count: stats.thread_count,
        status: stats.status,
        children,
    }]
}
