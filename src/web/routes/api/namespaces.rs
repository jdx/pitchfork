use axum::{extract::Path, response::Json};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::pitchfork_toml::PitchforkToml;

#[derive(Serialize)]
pub struct ApiNamespaceEntry {
    name: String,
    dir: String,
}

#[derive(Deserialize)]
pub struct RegisterNamespaceReq {
    dir: String,
}

pub async fn list() -> Json<Vec<ApiNamespaceEntry>> {
    let entries = PitchforkToml::read_global_namespaces()
        .into_iter()
        .map(|(name, entry)| ApiNamespaceEntry {
            name,
            dir: entry.dir.to_string_lossy().to_string(),
        })
        .collect();
    Json(entries)
}

pub async fn register(
    Json(req): Json<RegisterNamespaceReq>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let dir = PathBuf::from(&req.dir);
    let name = match PitchforkToml::namespace_for_dir(&dir) {
        Ok(ns) => ns,
        Err(e) => {
            log::error!("Failed to infer namespace for {}: {e}", req.dir);
            return Ok(Json(
                serde_json::json!({"ok": false, "error": format!("Failed to infer namespace: {e}") }),
            ));
        }
    };
    match PitchforkToml::register_namespace(&name, &req.dir) {
        Ok(()) => Ok(Json(serde_json::json!({"ok": true, "name": name}))),
        Err(e) => {
            log::error!("Failed to register namespace {}: {e}", name);
            Ok(Json(
                serde_json::json!({"ok": false, "error": e.to_string()}),
            ))
        }
    }
}

pub async fn remove(
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    match PitchforkToml::remove_namespace(&name) {
        Ok(true) => Ok(Json(serde_json::json!({"ok": true}))),
        Ok(false) => Ok(Json(
            serde_json::json!({"ok": false, "error": "namespace not found"}),
        )),
        Err(e) => {
            log::error!("Failed to remove namespace {name}: {e}");
            Ok(Json(
                serde_json::json!({"ok": false, "error": e.to_string()}),
            ))
        }
    }
}
