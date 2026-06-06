use crate::Result;
use serde::Serialize;

/// Generate JSON documentation for the web API endpoints
#[derive(Debug, clap::Args)]
#[clap(hide = true)]
pub struct ApiSchema;

#[derive(Serialize)]
struct ApiDoc {
    endpoints: Vec<Endpoint>,
}

#[derive(Serialize)]
struct Endpoint {
    path: &'static str,
    method: &'static str,
    description: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    path_params: Vec<Param>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    query_params: Vec<Param>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_body: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_type: Option<&'static str>,
    auth: bool,
}

#[derive(Serialize)]
struct Param {
    name: &'static str,
    type_name: &'static str,
    description: &'static str,
    required: bool,
}

impl ApiSchema {
    pub async fn run(&self) -> Result<()> {
        let doc = ApiDoc {
            endpoints: vec![
                Endpoint {
                    path: "/api/stats",
                    method: "GET",
                    description: "Return system-level statistics: process count, CPU count, and memory usage.",
                    path_params: vec![],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some("ApiStats"),
                    auth: true,
                },
                Endpoint {
                    path: "/api/daemons",
                    method: "GET",
                    description: "List all daemons with full state including status, ports, CPU/memory usage, and proxy URLs.",
                    path_params: vec![],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some("Vec<ApiDaemonEntry>"),
                    auth: true,
                },
                Endpoint {
                    path: "/api/daemons/{id}",
                    method: "GET",
                    description: "Get a single daemon by its qualified ID.",
                    path_params: vec![Param {
                        name: "id",
                        type_name: "String",
                        description: "Qualified daemon ID (e.g. \"project/hello\")",
                        required: true,
                    }],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some("ApiDaemonEntry"),
                    auth: true,
                },
                Endpoint {
                    path: "/api/daemons/{id}/start",
                    method: "POST",
                    description: "Start a stopped or failed daemon.",
                    path_params: vec![Param {
                        name: "id",
                        type_name: "String",
                        description: "Qualified daemon ID",
                        required: true,
                    }],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some(r#"{ "ok": bool, "error": String|null }"#),
                    auth: true,
                },
                Endpoint {
                    path: "/api/daemons/{id}/stop",
                    method: "POST",
                    description: "Gracefully stop a running daemon.",
                    path_params: vec![Param {
                        name: "id",
                        type_name: "String",
                        description: "Qualified daemon ID",
                        required: true,
                    }],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some(r#"{ "ok": bool, "error": null }"#),
                    auth: true,
                },
                Endpoint {
                    path: "/api/daemons/{id}/restart",
                    method: "POST",
                    description: "Stop and then start a daemon.",
                    path_params: vec![Param {
                        name: "id",
                        type_name: "String",
                        description: "Qualified daemon ID",
                        required: true,
                    }],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some(r#"{ "ok": bool, "error": String|null }"#),
                    auth: true,
                },
                Endpoint {
                    path: "/api/daemons/{id}/enable",
                    method: "POST",
                    description: "Enable a daemon so it can be started.",
                    path_params: vec![Param {
                        name: "id",
                        type_name: "String",
                        description: "Qualified daemon ID",
                        required: true,
                    }],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some(r#"{ "ok": bool, "error": String|null }"#),
                    auth: true,
                },
                Endpoint {
                    path: "/api/daemons/{id}/disable",
                    method: "POST",
                    description: "Disable a daemon so it cannot be started.",
                    path_params: vec![Param {
                        name: "id",
                        type_name: "String",
                        description: "Qualified daemon ID",
                        required: true,
                    }],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some(r#"{ "ok": bool, "error": String|null }"#),
                    auth: true,
                },
                Endpoint {
                    path: "/api/logs/{id}/tail",
                    method: "GET",
                    description: "Stream daemon logs via SSE (Server-Sent Events). Each line is prefixed with \"data: \".",
                    path_params: vec![Param {
                        name: "id",
                        type_name: "String",
                        description: "Qualified daemon ID",
                        required: true,
                    }],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some("text/event-stream"),
                    auth: true,
                },
                Endpoint {
                    path: "/api/namespaces",
                    method: "GET",
                    description: "List all registered namespaces.",
                    path_params: vec![],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some("Vec<ApiNamespaceEntry>"),
                    auth: true,
                },
                Endpoint {
                    path: "/api/namespaces",
                    method: "POST",
                    description: "Register a new namespace by directory path.",
                    path_params: vec![],
                    query_params: vec![],
                    request_body: Some(r#"{ "dir": "String" }"#),
                    response_type: Some(r#"{ "ok": bool }"#),
                    auth: true,
                },
                Endpoint {
                    path: "/api/namespaces/{name}",
                    method: "DELETE",
                    description: "Remove a namespace by name.",
                    path_params: vec![Param {
                        name: "name",
                        type_name: "String",
                        description: "Namespace name",
                        required: true,
                    }],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some(r#"{ "ok": bool }"#),
                    auth: true,
                },
                Endpoint {
                    path: "/api/proxies",
                    method: "GET",
                    description: "List all configured proxy slugs with their target daemon, worktree, and URL.",
                    path_params: vec![],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some("Vec<ApiProxyWorktreeEntry>"),
                    auth: true,
                },
                Endpoint {
                    path: "/api/processes/{id}/tree",
                    method: "GET",
                    description: "Return the process tree rooted at the given daemon's PID, including child processes.",
                    path_params: vec![Param {
                        name: "id",
                        type_name: "String",
                        description: "Qualified daemon ID",
                        required: true,
                    }],
                    query_params: vec![],
                    request_body: None,
                    response_type: Some("Vec<ApiProcessTree>"),
                    auth: true,
                },
            ],
        };

        let json =
            serde_json::to_string_pretty(&doc).expect("failed to serialize API schema to JSON");
        println!("{json}");
        Ok(())
    }
}
