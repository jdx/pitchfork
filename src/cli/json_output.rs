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
    pub proxy_url: Option<String>,
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
    pub proxy_url: Option<String>,
}

#[derive(Serialize)]
pub struct JsonLogEntry {
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
}

#[derive(Serialize)]
pub struct JsonLanInfo {
    pub enabled: bool,
    pub ip: String,
}

#[derive(Serialize)]
pub struct JsonSlugEntry {
    pub slug: String,
    pub url: String,
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
    pub default: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_var: Option<&'static str>,
}

pub fn print_json<T: Serialize>(value: &T) -> crate::Result<()> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| miette::miette!("failed to serialize JSON: {e}"))?;
    println!("{json}");
    Ok(())
}
