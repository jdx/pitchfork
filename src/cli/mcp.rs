use crate::Result;
use crate::daemon_id::DaemonId;
use crate::daemon_list::get_all_daemons;
use crate::ipc::batch::StartOptions;
use crate::ipc::client::IpcClient;
use crate::log_store::sqlite::LOG_STORE;
use crate::pitchfork_toml::PitchforkToml;
use rmcp::{
    RoleServer, ServiceExt,
    handler::server::{ServerHandler, tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResult, Content, ErrorCode, ErrorData, Implementation,
        InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    },
    schemars::JsonSchema,
    service::RequestContext,
    tool, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// Runs a Model Context Protocol (MCP) server over stdin/stdout
///
/// This command starts an MCP server that exposes pitchfork daemon management
/// to AI assistants (Claude, Cursor, etc.) over stdin/stdout using JSON-RPC.
///
/// Tools available:
/// - pitchfork_status - List all daemons and their state
/// - pitchfork_start  - Start a named daemon
/// - pitchfork_stop   - Stop a named daemon
/// - pitchfork_restart - Restart a named daemon
/// - pitchfork_logs   - Return recent log output for a daemon
#[derive(Debug, clap::Args)]
#[clap(
    verbatim_doc_comment,
    after_long_help = AFTER_LONG_HELP,
    long_about = "\
Runs a Model Context Protocol (MCP) server over stdin/stdout

This command starts an MCP server that exposes pitchfork daemon management
to AI assistants (Claude, Cursor, etc.) over stdin/stdout using JSON-RPC.

Typically used as a subprocess by an MCP-aware AI agent.

Examples:
  # In claude_desktop_config.json or similar:
  {
    \"mcpServers\": {
      \"pitchfork\": {
        \"command\": \"pitchfork\",
        \"args\": [\"mcp\"]
      }
    }
  }

Tools provided:
  pitchfork_status    List all daemons and their state
  pitchfork_start     Start a named daemon
  pitchfork_stop      Stop a named daemon
  pitchfork_restart   Restart a named daemon
  pitchfork_logs      Return recent log output for a daemon"
)]
pub struct Mcp {}

#[derive(Clone)]
struct PitchforkServer {
    tool_router: ToolRouter<Self>,
}

// ── Tool parameter types ────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct StartParams {
    /// Daemon name(s) to start (e.g. "api" or "api,worker")
    id: Vec<String>,
    /// Force restart if already running
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct StopParams {
    /// Daemon name(s) to stop
    id: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct RestartParams {
    /// Daemon name(s) to restart
    id: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct LogsParams {
    /// Daemon name(s) to fetch logs for. If empty, returns logs for all daemons.
    #[serde(default)]
    id: Vec<String>,
    /// Number of recent lines to return (default: 50)
    #[serde(default = "default_log_lines")]
    n: usize,
}

fn default_log_lines() -> usize {
    50
}

// ── Helper: create an internal ErrorData ─────────────────────────────

fn internal_err(msg: String) -> ErrorData {
    ErrorData::new(ErrorCode::INTERNAL_ERROR, msg, None::<serde_json::Value>)
}

// ── Tool implementations ────────────────────────────────────────────

#[tool_router]
impl PitchforkServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    /// List all daemons and their current state (PID, status, errors)
    #[tool(
        description = "List all pitchfork daemons and their current state including PID, status, and errors"
    )]
    async fn pitchfork_status(&self) -> std::result::Result<CallToolResult, ErrorData> {
        let client = IpcClient::connect(true)
            .await
            .map_err(|e| internal_err(format!("Failed to connect to supervisor: {e}")))?;

        let entries = get_all_daemons(&client)
            .await
            .map_err(|e| internal_err(format!("Failed to list daemons: {e}")))?;

        let daemons: Vec<_> = entries
            .iter()
            .map(|entry| {
                let status_text = if entry.is_available {
                    "available".to_string()
                } else {
                    entry.daemon.status.to_string()
                };

                json!({
                    "name": entry.id.qualified(),
                    "pid": entry.daemon.pid,
                    "status": status_text,
                    "disabled": entry.is_disabled,
                    "error": entry.daemon.status.error_message(),
                })
            })
            .collect();

        let text = serde_json::to_string_pretty(&daemons)
            .map_err(|e| internal_err(format!("Failed to serialize daemon status: {e}")))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Start one or more named daemons
    #[tool(
        description = "Start one or more pitchfork daemons by name. Use force=true to restart if already running."
    )]
    async fn pitchfork_start(
        &self,
        Parameters(params): Parameters<StartParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        if params.id.is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "At least one daemon ID must be provided",
            )]));
        }

        let ipc = Arc::new(
            IpcClient::connect(true)
                .await
                .map_err(|e| internal_err(format!("Failed to connect to supervisor: {e}")))?,
        );

        let ids = PitchforkToml::resolve_ids(&params.id)
            .map_err(|e| internal_err(format!("Failed to resolve daemon IDs: {e}")))?;

        let opts = StartOptions {
            force: params.force,
            ..Default::default()
        };

        let result = ipc
            .start_daemons(&ids, opts)
            .await
            .map_err(|e| internal_err(format!("Failed to start daemons: {e}")))?;

        let started_names: Vec<String> = result
            .started
            .iter()
            .map(|(id, _, _)| id.qualified())
            .collect();

        if result.any_failed {
            let msg = if started_names.is_empty() {
                "All daemons failed to start".to_string()
            } else {
                format!(
                    "Some daemons failed. Successfully started: {}",
                    started_names.join(", ")
                )
            };
            Ok(CallToolResult::error(vec![Content::text(msg)]))
        } else if started_names.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No daemons needed starting (already running or no matching daemons found)",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "Started: {}",
                started_names.join(", ")
            ))]))
        }
    }

    /// Stop one or more named daemons
    #[tool(description = "Stop one or more pitchfork daemons by name")]
    async fn pitchfork_stop(
        &self,
        Parameters(params): Parameters<StopParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        if params.id.is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "At least one daemon ID must be provided",
            )]));
        }

        let ipc = Arc::new(
            IpcClient::connect(false)
                .await
                .map_err(|e| internal_err(format!("Failed to connect to supervisor: {e}")))?,
        );

        let ids = PitchforkToml::resolve_ids(&params.id)
            .map_err(|e| internal_err(format!("Failed to resolve daemon IDs: {e}")))?;

        // Snapshot running daemons before stop to determine what was actually stopped
        let running_before: std::collections::HashSet<_> = ipc
            .get_running_daemons()
            .await
            .map_err(|e| internal_err(format!("Failed to query running daemons: {e}")))?
            .into_iter()
            .collect();

        let actually_running: Vec<_> = ids
            .iter()
            .filter(|id| running_before.contains(id))
            .cloned()
            .collect();

        if actually_running.is_empty() {
            let names: Vec<String> = ids.iter().map(|id| id.qualified()).collect();
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No daemons were running: {}",
                names.join(", ")
            ))]));
        }

        let result = ipc
            .stop_daemons(&ids)
            .await
            .map_err(|e| internal_err(format!("Failed to stop daemons: {e}")))?;

        if result.any_failed {
            Ok(CallToolResult::error(vec![Content::text(
                "Some daemons failed to stop",
            )]))
        } else {
            let names: Vec<String> = actually_running.iter().map(|id| id.qualified()).collect();
            Ok(CallToolResult::success(vec![Content::text(format!(
                "Stopped: {}",
                names.join(", ")
            ))]))
        }
    }

    /// Restart one or more named daemons (stop then start)
    #[tool(
        description = "Restart one or more pitchfork daemons by name (equivalent to start --force)"
    )]
    async fn pitchfork_restart(
        &self,
        Parameters(params): Parameters<RestartParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        if params.id.is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "At least one daemon ID must be provided",
            )]));
        }

        let ipc = Arc::new(
            IpcClient::connect(true)
                .await
                .map_err(|e| internal_err(format!("Failed to connect to supervisor: {e}")))?,
        );

        let ids = PitchforkToml::resolve_ids(&params.id)
            .map_err(|e| internal_err(format!("Failed to resolve daemon IDs: {e}")))?;

        let opts = StartOptions {
            force: true,
            ..Default::default()
        };

        let result = ipc
            .start_daemons(&ids, opts)
            .await
            .map_err(|e| internal_err(format!("Failed to restart daemons: {e}")))?;

        let started_names: Vec<String> = result
            .started
            .iter()
            .map(|(id, _, _)| id.qualified())
            .collect();

        if result.any_failed {
            let msg = if started_names.is_empty() {
                "All daemons failed to restart".to_string()
            } else {
                format!(
                    "Some daemons failed. Successfully restarted: {}",
                    started_names.join(", ")
                )
            };
            Ok(CallToolResult::error(vec![Content::text(msg)]))
        } else if started_names.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No daemons were restarted",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "Restarted: {}",
                started_names.join(", ")
            ))]))
        }
    }

    /// Return recent log output for one or more daemons
    #[tool(
        description = "Return recent log output for one or more pitchfork daemons. Returns last N lines (default 50)."
    )]
    async fn pitchfork_logs(
        &self,
        Parameters(params): Parameters<LogsParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        use crate::log_store::{LogQuery, LogStore};

        let daemon_ids = if params.id.is_empty() {
            let ids = tokio::task::spawn_blocking(|| LOG_STORE.list_daemon_ids())
                .await
                .map_err(|e| internal_err(format!("Failed to list daemon IDs: {e}")))?
                .map_err(|e| internal_err(format!("Failed to list daemon IDs: {e}")))?;
            ids.into_iter()
                .filter_map(|id| DaemonId::parse(&id).ok())
                .collect()
        } else {
            PitchforkToml::resolve_ids(&params.id)
                .map_err(|e| internal_err(format!("Failed to resolve daemon IDs: {e}")))?
        };

        if daemon_ids.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No daemon logs found",
            )]));
        }

        let qualified: Vec<String> = daemon_ids.iter().map(|d| d.qualified()).collect();
        let limit = params.n;
        let entries = tokio::task::spawn_blocking(move || {
            LOG_STORE.query(&LogQuery {
                daemon_ids: qualified,
                from: None,
                to: None,
                limit: Some(limit),
                order_desc: true,
                after_id: None,
                message_filters: Vec::new(),
            })
        })
        .await
        .map_err(|e| internal_err(format!("Failed to query logs: {e}")))?
        .map_err(|e| internal_err(format!("Failed to query logs: {e}")))?;

        let mut output = String::new();
        for daemon_id in &daemon_ids {
            let daemon_entries: Vec<_> = entries
                .iter()
                .filter(|e| e.daemon_id == daemon_id.qualified())
                .collect();
            if daemon_entries.is_empty() {
                continue;
            }
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            output.push_str(&format!("=== {} ===\n", daemon_id.qualified()));
            for entry in daemon_entries.into_iter().rev() {
                let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S");
                output.push_str(&format!("{} {}\n", ts, entry.message));
            }
        }

        if output.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "No logs available",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(output)]))
        }
    }
}

// ── ServerHandler implementation ────────────────────────────────────

impl ServerHandler for PitchforkServer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(
            ServerCapabilities::builder().enable_tools().build(),
        )
        .with_server_info(Implementation::new(
            "pitchfork",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(
            "Pitchfork MCP server — manage daemon lifecycle (start, stop, restart, status, logs)",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            meta: None,
            tools: self.tool_router.list_all(),
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let tool_call_context =
            rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tool_call_context).await
    }
}

// ── CLI entry point ─────────────────────────────────────────────────

impl Mcp {
    pub async fn run(&self) -> Result<()> {
        eprintln!("Starting pitchfork MCP server...");

        let server = PitchforkServer::new();

        let service = server
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| miette::miette!("Failed to start MCP service: {e}"))?;

        service
            .waiting()
            .await
            .map_err(|e| miette::miette!("MCP service error: {e}"))?;

        Ok(())
    }
}

// ── Log helpers ─────────────────────────────────────────────────────
// (Legacy text log helpers removed; all log reads now go through the SQLite log store.)

// ── Help text ───────────────────────────────────────────────────────

static AFTER_LONG_HELP: &str = r#"Examples:

  # Start the MCP server (used by AI assistant tools)
  $ pitchfork mcp

  # Claude Desktop configuration (claude_desktop_config.json):
  {
    "mcpServers": {
      "pitchfork": {
        "command": "pitchfork",
        "args": ["mcp"]
      }
    }
  }

  # Interactive testing with JSON-RPC:
  $ echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' | pitchfork mcp

  # Available tools:
  - pitchfork_status  - List all daemons and their state
  - pitchfork_start   - Start daemon(s) by name
  - pitchfork_stop    - Stop daemon(s) by name
  - pitchfork_restart - Restart daemon(s) by name
  - pitchfork_logs    - Return recent log output for daemon(s)
"#;
