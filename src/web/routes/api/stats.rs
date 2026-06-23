use axum::response::Json;
use serde::Serialize;

use crate::daemon_id::DaemonId;
use crate::env;
use crate::pitchfork_toml::PitchforkToml;
use crate::state_file::StateFile;

#[derive(Serialize)]
pub struct ApiStats {
    total: usize,
    running: usize,
    stopped: usize,
    errored: usize,
    available: usize,
}

pub async fn stats() -> Json<ApiStats> {
    let state = match StateFile::read(&*env::PITCHFORK_STATE_FILE) {
        Ok(s) => s,
        Err(_) => StateFile::new(env::PITCHFORK_STATE_FILE.clone()),
    };

    let pt = match PitchforkToml::all_merged_all_namespaces() {
        Ok(pt) => pt,
        Err(_) => {
            return Json(ApiStats {
                total: 0,
                running: 0,
                stopped: 0,
                errored: 0,
                available: 0,
            });
        }
    };

    let pitchfork_id = DaemonId::pitchfork();
    let user_daemons: Vec<_> = state
        .daemons
        .iter()
        .filter(|(id, _)| **id != pitchfork_id)
        .collect();

    let running = user_daemons
        .iter()
        .filter(|(_, d)| d.status.is_running())
        .count();
    let stopped = user_daemons
        .iter()
        .filter(|(_, d)| d.status.is_stopped())
        .count();
    let errored = user_daemons
        .iter()
        .filter(|(_, d)| d.status.is_errored())
        .count();

    // Config-only daemons (in config but not in state) are "available".
    let state_ids: std::collections::HashSet<&DaemonId> =
        user_daemons.iter().map(|(id, _)| *id).collect();
    let available = pt
        .daemons
        .keys()
        .filter(|id| **id != pitchfork_id && !state_ids.contains(id))
        .count();

    let mut all_ids = std::collections::HashSet::new();
    for (id, _) in user_daemons {
        all_ids.insert(id.clone());
    }
    for id in pt.daemons.keys() {
        all_ids.insert(id.clone());
    }
    let total = all_ids.len();

    Json(ApiStats {
        total,
        running,
        stopped,
        errored,
        available,
    })
}
