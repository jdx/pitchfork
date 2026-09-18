use crate::log_store::LogEntry;
use serde::Serialize;

#[derive(Serialize)]
pub struct JsonListEntry {
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub pid: Option<u32>,
    pub status: String,
    pub disabled: bool,
    pub available: bool,
    /// Deprecated alias of `url`, kept so existing consumers keep working.
    pub proxy_url: Option<String>,
    /// The daemon's proxy URL.
    pub url: Option<String>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_port: Option<u16>,
    pub port: Vec<u16>,
}

#[derive(Serialize)]
pub struct JsonStatusEntry {
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub pid: Option<u32>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_port: Option<u16>,
    pub port: Vec<u16>,
    /// Deprecated alias of `url`, kept so existing consumers keep working.
    pub proxy_url: Option<String>,
    /// The daemon's proxy URL.
    pub url: Option<String>,
}

#[derive(Serialize)]
pub struct JsonLogEntry {
    /// SQLite row id, used by the webui for backward pagination (scroll-up
    /// history loading). Skipped in serialization when 0 to avoid changing
    /// the CLI --json output shape for non-web callers.
    #[serde(skip_serializing_if = "is_zero_id")]
    pub id: i64,
    pub timestamp: String,
    pub daemon_id: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logger: Option<String>,
    /// Parsed structured fields as a JSON object, or null if the line was
    /// not structured (plain text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Value>,
}

fn is_zero_id(id: &i64) -> bool {
    *id == 0
}

impl From<LogEntry> for JsonLogEntry {
    fn from(e: LogEntry) -> Self {
        let fields = e
            .fields_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        JsonLogEntry {
            id: e.id,
            timestamp: e.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
            daemon_id: e.daemon_id,
            message: console::strip_ansi_codes(&e.message).to_string(),
            level: e.level,
            msg: e.msg,
            logger: e.logger,
            fields,
        }
    }
}

#[derive(Serialize)]
pub struct JsonDaemonConfigEntry {
    pub id: String,
    pub run: String,
}

#[derive(Serialize)]
pub struct JsonProxyStatus {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tld: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lan: Option<JsonLanInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_cert: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted: Option<bool>,
    pub slugs: Vec<JsonSlugEntry>,
    /// Automatic hostnames, grouped by project and worktree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<JsonProxyProject>,
    /// Labels claimed by more than one project, worktree or daemon. Nothing is
    /// routed under them until the clash is resolved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
}

#[derive(Serialize)]
pub struct JsonProxyProject {
    pub project: String,
    pub dir: String,
    /// URL of the project page.
    pub url: String,
    pub daemons: Vec<JsonProxyHost>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub worktrees: Vec<JsonProxyWorktree>,
}

#[derive(Serialize)]
pub struct JsonProxyWorktree {
    pub worktree: String,
    pub dir: String,
    /// URL of the stack page.
    pub url: String,
    pub daemons: Vec<JsonProxyHost>,
}

#[derive(Serialize)]
pub struct JsonProxyHost {
    pub daemon: String,
    /// Hostname without the TLD.
    pub host: String,
    pub url: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

#[derive(Serialize)]
pub struct JsonLanInfo {
    pub enabled: bool,
    pub ip: String,
}

#[derive(Serialize)]
pub struct JsonSlugEntry {
    pub slug: String,
    /// Absent when the slug is not routable (see `status`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub dir: String,
    pub daemon: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

#[derive(Serialize)]
pub struct JsonSupervisorStatus {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_ui: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct JsonSettingEntry {
    pub key: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_var: Option<&'static str>,
    /// Where the winning value came from, named so a script can act on it:
    /// `PITCHFORK_LOG`, `/path/to/pitchfork.toml#general.log_level`, `the
    /// default`. Absent only for a setting nothing set and that has no
    /// default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

pub fn print_json<T: Serialize>(value: &T) -> crate::Result<()> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| miette::miette!("failed to serialize JSON: {e}"))?;
    println!("{json}");
    Ok(())
}
