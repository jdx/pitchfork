use crate::Result;
use crate::cli::daemons::resolve_project_config_path;
use crate::env;
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::{SETTINGS_META, SettingsPartial, settings};
use clap::Parser;
use miette::{IntoDiagnostic, bail};
use std::path::PathBuf;

const LOG_LEVEL_VALUES: &[&str] = &["trace", "debug", "info", "warn", "error"];

/// View and modify pitchfork settings
#[derive(Debug, Parser)]
#[clap(
    visible_alias = "setting",
    verbatim_doc_comment,
    long_about = "\
View and modify pitchfork settings

Settings can be configured in multiple ways (in order of precedence):
1. Environment variables (highest priority)
2. Project-level pitchfork.toml or pitchfork.local.toml in [settings] section
3. User-level ~/.config/pitchfork/config.toml in [settings] section
4. System-level /etc/pitchfork/config.toml in [settings] section
5. Built-in defaults (lowest priority)

Subcommands:
  list    List all available settings with types and defaults
  get     Get the current value of a setting
  set     Set a setting value in a config file

Examples:
  pitchfork settings                        Show all current settings
  pitchfork settings list                   List all available settings
  pitchfork settings get general.log_level  Get a specific setting
  pitchfork settings set general.log_level debug
  pitchfork settings set web.auto_start true --global
  pitchfork settings set supervisor.stop_timeout 10s --local
  pitchfork settings set supervisor.stop_timeout 10s --project"
)]
pub struct Settings {
    #[clap(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Parser)]
enum Commands {
    /// List all available settings with types and defaults
    #[clap(visible_alias = "ls")]
    List(ListCmd),
    /// Get the current value of a setting
    Get(GetCmd),
    /// Set a setting value in a config file
    Set(SetCmd),
}

/// List all available settings with types and defaults
#[derive(Debug, Parser)]
#[clap(verbatim_doc_comment)]
pub struct ListCmd {
    /// Only show settings in a specific group (e.g., "general", "web", "supervisor")
    #[clap(long)]
    group: Option<String>,
}

/// Get the current value of a setting
#[derive(Debug, Parser)]
#[clap(verbatim_doc_comment)]
pub struct GetCmd {
    /// Setting key in dot notation (e.g., general.log_level, web.auto_start)
    key: String,
}

/// Set a setting value in a config file
#[derive(Debug, Parser)]
#[clap(verbatim_doc_comment)]
pub struct SetCmd {
    /// Setting key in dot notation (e.g., general.log_level, web.auto_start)
    key: String,
    /// Value to set (type must match the setting: string, integer, boolean, or duration)
    value: String,
    /// Write to the user-level global config (~/.config/pitchfork/config.toml)
    #[clap(long)]
    global: bool,
    /// Write to the project-level pitchfork.local.toml (overrides pitchfork.toml)
    #[clap(long)]
    local: bool,
    /// Write to the project-level pitchfork.toml (default if no flag specified)
    #[clap(long)]
    project: bool,
}

impl Settings {
    pub async fn run(self) -> Result<()> {
        match self.command {
            Some(Commands::List(cmd)) => cmd.run(),
            Some(Commands::Get(cmd)) => cmd.run(),
            Some(Commands::Set(cmd)) => cmd.run().await,
            None => show_all_settings(),
        }
    }
}

impl ListCmd {
    fn run(&self) -> Result<()> {
        let meta = &*SETTINGS_META;
        for (key, info) in meta.iter() {
            if let Some(ref group) = self.group {
                if !key.starts_with(&format!("{group}.")) && key != group {
                    continue;
                }
            }
            let default = info.default_value.unwrap_or("(none)");
            let env_hint = info.env_var.map(|e| format!(" [{e}]")).unwrap_or_default();
            println!("{key} ({}) default={default}{env_hint}", info.typ);
            if !info.description.is_empty() {
                let desc = info.description.lines().next().unwrap_or("");
                println!("  {desc}");
            }
        }
        Ok(())
    }
}

impl GetCmd {
    fn run(&self) -> Result<()> {
        let key = &self.key;
        validate_setting_key(key)?;

        let s = settings();
        let value = get_setting_value(s, key);
        println!("{value}");
        Ok(())
    }
}

impl SetCmd {
    async fn run(&self) -> Result<()> {
        let key = &self.key;
        let value = &self.value;
        validate_setting_key(key)?;
        validate_setting_value(key, value)?;

        let config_path = resolve_config_path(self.global, self.local, self.project).await?;

        let mut pt = if tokio::fs::try_exists(&config_path).await.unwrap_or(false) {
            let config_path_clone = config_path.clone();
            let result =
                tokio::task::spawn_blocking(move || PitchforkToml::read(&config_path_clone))
                    .await
                    .into_diagnostic()?;
            result.map_err(|e| miette::miette!("{e}"))?
        } else {
            if let Some(parent) = config_path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    miette::miette!(
                        "Failed to create config directory {}: {e}",
                        parent.display()
                    )
                })?;
            }
            PitchforkToml::new(config_path.clone())
        };
        pt.path = Some(config_path.clone());

        apply_setting_to_partial(&mut pt.settings, key, value)?;

        tokio::task::spawn_blocking(move || pt.write())
            .await
            .into_diagnostic()?
            .map_err(|e| miette::miette!("{e}"))?;

        let path_display = config_path.display();
        println!("set {key} = {value} in {path_display}");

        notify_supervisor_reload().await;

        Ok(())
    }
}

fn show_all_settings() -> Result<()> {
    let s = settings();
    let meta = &*SETTINGS_META;

    let mut current_group = String::new();
    for (key, info) in meta.iter() {
        let group = key.split('.').next().unwrap_or("");
        if group != current_group {
            if !current_group.is_empty() {
                println!();
            }
            current_group = group.to_string();
            println!("[settings.{group}]");
        }

        let field_name = key.split('.').nth(1).unwrap_or(key);
        let current = get_setting_value(s, key);
        let default = info.default_value.unwrap_or("");
        let is_default = current == default;

        if is_default {
            if current.is_empty() {
                println!("# {field_name}  (default: empty)");
            } else {
                println!("# {field_name} = {current}  (default)");
            }
        } else {
            println!("{field_name} = {current}");
        }
    }

    Ok(())
}

fn validate_setting_key(key: &str) -> Result<()> {
    let meta = &*SETTINGS_META;
    if meta.contains_key(key) {
        return Ok(());
    }

    let mut suggestions: Vec<&str> = meta
        .keys()
        .filter(|k| {
            let dist = levenshtein_distance(key, k);
            dist <= 3 || k.contains(key)
        })
        .copied()
        .collect();

    if suggestions.is_empty() {
        bail!(
            "unknown setting '{key}'. Run 'pitchfork settings list' to see all available settings"
        );
    }

    suggestions.sort();
    bail!(
        "unknown setting '{key}'. Did you mean one of: {}?",
        suggestions.join(", ")
    )
}

fn validate_setting_value(key: &str, value: &str) -> Result<()> {
    let meta = &*SETTINGS_META;
    let info = meta.get(key).unwrap();

    match info.typ {
        "Bool" if value != "true" && value != "false" => {
            bail!("invalid boolean value '{value}' for '{key}'. Expected 'true' or 'false'");
        }
        "Integer" if value.parse::<i64>().is_err() => {
            bail!("invalid integer value '{value}' for '{key}'. Expected a number");
        }
        "Duration" if humantime::parse_duration(value).is_err() => {
            bail!(
                "invalid duration value '{value}' for '{key}'. Expected a duration like '10s', '5m', '1h', '500ms'"
            );
        }
        "String" | "Path"
            if (key == "general.log_level" || key == "general.log_file_level")
                && !LOG_LEVEL_VALUES.contains(&value) =>
        {
            bail!(
                "invalid log level '{value}' for '{key}'. Expected one of: {}",
                LOG_LEVEL_VALUES.join(", ")
            );
        }
        _ => {}
    }

    Ok(())
}

fn get_setting_value(s: &crate::settings::Settings, key: &str) -> String {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() != 2 {
        return String::new();
    }

    match parts[0] {
        "general" => get_general_value(&s.general, parts[1]),
        "ipc" => get_ipc_value(&s.ipc, parts[1]),
        "logs" => get_logs_value(&s.logs, parts[1]),
        "web" => get_web_value(&s.web, parts[1]),
        "api" => get_api_value(&s.api, parts[1]),
        "tui" => get_tui_value(&s.tui, parts[1]),
        "supervisor" => get_supervisor_value(&s.supervisor, parts[1]),
        "proxy" => get_proxy_value(&s.proxy, parts[1]),
        _ => String::new(),
    }
}

fn get_general_value(g: &crate::settings::SettingsGeneral, field: &str) -> String {
    match field {
        "autostop_delay" => g.autostop_delay.clone(),
        "interval" => g.interval.clone(),
        "log_level" => g.log_level.clone(),
        "log_file_level" => g.log_file_level.clone(),
        "mise" => g.mise.to_string(),
        "mise_bin" => g.mise_bin.clone(),
        "startup_log_timestamps" => g.startup_log_timestamps.to_string(),
        _ => String::new(),
    }
}

fn get_ipc_value(g: &crate::settings::SettingsIpc, field: &str) -> String {
    match field {
        "connect_attempts" => g.connect_attempts.to_string(),
        "connect_min_delay" => g.connect_min_delay.clone(),
        "connect_max_delay" => g.connect_max_delay.clone(),
        "request_timeout" => g.request_timeout.clone(),
        "rate_limit" => g.rate_limit.to_string(),
        "rate_limit_window" => g.rate_limit_window.clone(),
        _ => String::new(),
    }
}

fn get_logs_value(g: &crate::settings::SettingsLogs, field: &str) -> String {
    match field {
        "time_retention" => g.time_retention.clone(),
        "line_retention" => g.line_retention.to_string(),
        _ => String::new(),
    }
}

fn get_web_value(g: &crate::settings::SettingsWeb, field: &str) -> String {
    match field {
        "auto_start" => g.auto_start.to_string(),
        "bind_address" => g.bind_address.clone(),
        "bind_port" => g.bind_port.to_string(),
        "port_attempts" => g.port_attempts.to_string(),
        "log_lines" => g.log_lines.to_string(),
        "base_path" => g.base_path.clone(),
        "sse_poll_interval" => g.sse_poll_interval.clone(),
        _ => String::new(),
    }
}

fn get_api_value(g: &crate::settings::SettingsApi, field: &str) -> String {
    match field {
        "auto_start" => g.auto_start.to_string(),
        "bind_address" => g.bind_address.clone(),
        "bind_port" => g.bind_port.to_string(),
        "port_attempts" => g.port_attempts.to_string(),
        "token" => g.token.clone(),
        _ => String::new(),
    }
}

fn get_tui_value(g: &crate::settings::SettingsTui, field: &str) -> String {
    match field {
        "refresh_rate" => g.refresh_rate.clone(),
        "tick_rate" => g.tick_rate.clone(),
        "stat_history" => g.stat_history.to_string(),
        "message_duration" => g.message_duration.clone(),
        _ => String::new(),
    }
}

fn get_supervisor_value(g: &crate::settings::SettingsSupervisor, field: &str) -> String {
    match field {
        "ready_check_interval" => g.ready_check_interval.clone(),
        "file_watch_debounce" => g.file_watch_debounce.clone(),
        "log_flush_interval" => g.log_flush_interval.clone(),
        "stop_timeout" => g.stop_timeout.clone(),
        "restart_delay" => g.restart_delay.clone(),
        "cron_check_interval" => g.cron_check_interval.clone(),
        "watch_interval" => g.watch_interval.clone(),
        "watch_poll_interval" => g.watch_poll_interval.clone(),
        "http_client_timeout" => g.http_client_timeout.clone(),
        "port_bump_attempts" => g.port_bump_attempts.to_string(),
        "container" => g.container.to_string(),
        "cleanup_orphans" => g.cleanup_orphans.to_string(),
        "user" => g.user.clone(),
        "cpu_violation_threshold" => g.cpu_violation_threshold.to_string(),
        _ => String::new(),
    }
}

fn get_proxy_value(g: &crate::settings::SettingsProxy, field: &str) -> String {
    match field {
        "enable" => g.enable.to_string(),
        "tld" => g.tld.clone(),
        "host" => g.host.clone(),
        "port" => g.port.to_string(),
        "https" => g.https.to_string(),
        "tls_cert" => g.tls_cert.clone(),
        "tls_key" => g.tls_key.clone(),
        "auto_trust" => g.auto_trust.to_string(),
        "auto_start" => g.auto_start.to_string(),
        "auto_start_timeout" => g.auto_start_timeout.clone(),
        "sync_hosts" => g.sync_hosts.to_string(),
        "wildcard" => g.wildcard.to_string(),
        "worktree" => g.worktree.to_string(),
        "lan" => g.lan.to_string(),
        "lan_ip" => g.lan_ip.clone(),
        _ => String::new(),
    }
}

fn apply_setting_to_partial(partial: &mut SettingsPartial, key: &str, value: &str) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() != 2 {
        bail!("setting key must be in 'group.field' format (e.g., 'general.log_level')");
    }

    let meta = &*SETTINGS_META;
    let info = meta.get(key).unwrap();

    match parts[0] {
        "general" => apply_general_value(&mut partial.general, parts[1], value, info.typ)?,
        "ipc" => apply_ipc_value(&mut partial.ipc, parts[1], value, info.typ)?,
        "logs" => apply_logs_value(&mut partial.logs, parts[1], value, info.typ)?,
        "web" => apply_web_value(&mut partial.web, parts[1], value, info.typ)?,
        "api" => apply_api_value(&mut partial.api, parts[1], value, info.typ)?,
        "tui" => apply_tui_value(&mut partial.tui, parts[1], value, info.typ)?,
        "supervisor" => apply_supervisor_value(&mut partial.supervisor, parts[1], value, info.typ)?,
        "proxy" => apply_proxy_value(&mut partial.proxy, parts[1], value, info.typ)?,
        _ => bail!("unknown setting group '{}'", parts[0]),
    }

    Ok(())
}

fn parse_bool_value(value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("invalid boolean value '{value}'. Expected 'true' or 'false'"),
    }
}

fn parse_int_value(value: &str) -> Result<i64> {
    value
        .parse::<i64>()
        .map_err(|_| miette::miette!("invalid integer value '{value}'. Expected a number"))
}

fn apply_general_value(
    partial: &mut crate::settings::SettingsGeneralPartial,
    field: &str,
    value: &str,
    typ: &str,
) -> Result<()> {
    match field {
        "autostop_delay" => partial.autostop_delay = Some(value.to_string()),
        "interval" => partial.interval = Some(value.to_string()),
        "log_level" => partial.log_level = Some(value.to_string()),
        "log_file_level" => partial.log_file_level = Some(value.to_string()),
        "mise" => partial.mise = Some(parse_bool_value(value)?),
        "mise_bin" => partial.mise_bin = Some(value.to_string()),
        "startup_log_timestamps" => partial.startup_log_timestamps = Some(parse_bool_value(value)?),
        _ => bail!("unknown general setting '{field}'"),
    }
    let _ = typ;
    Ok(())
}

fn apply_ipc_value(
    partial: &mut crate::settings::SettingsIpcPartial,
    field: &str,
    value: &str,
    typ: &str,
) -> Result<()> {
    match field {
        "connect_attempts" => partial.connect_attempts = Some(parse_int_value(value)?),
        "connect_min_delay" => partial.connect_min_delay = Some(value.to_string()),
        "connect_max_delay" => partial.connect_max_delay = Some(value.to_string()),
        "request_timeout" => partial.request_timeout = Some(value.to_string()),
        "rate_limit" => partial.rate_limit = Some(parse_int_value(value)?),
        "rate_limit_window" => partial.rate_limit_window = Some(value.to_string()),
        _ => bail!("unknown ipc setting '{field}'"),
    }
    let _ = typ;
    Ok(())
}

fn apply_logs_value(
    partial: &mut crate::settings::SettingsLogsPartial,
    field: &str,
    value: &str,
    typ: &str,
) -> Result<()> {
    match field {
        "time_retention" => partial.time_retention = Some(value.to_string()),
        "line_retention" => partial.line_retention = Some(parse_int_value(value)?),
        _ => bail!("unknown logs setting '{field}'"),
    }
    let _ = typ;
    Ok(())
}

fn apply_web_value(
    partial: &mut crate::settings::SettingsWebPartial,
    field: &str,
    value: &str,
    typ: &str,
) -> Result<()> {
    match field {
        "auto_start" => partial.auto_start = Some(parse_bool_value(value)?),
        "bind_address" => partial.bind_address = Some(value.to_string()),
        "bind_port" => partial.bind_port = Some(parse_int_value(value)?),
        "port_attempts" => partial.port_attempts = Some(parse_int_value(value)?),
        "log_lines" => partial.log_lines = Some(parse_int_value(value)?),
        "base_path" => partial.base_path = Some(value.to_string()),
        "sse_poll_interval" => partial.sse_poll_interval = Some(value.to_string()),
        _ => bail!("unknown web setting '{field}'"),
    }
    let _ = typ;
    Ok(())
}

fn apply_api_value(
    partial: &mut crate::settings::SettingsApiPartial,
    field: &str,
    value: &str,
    typ: &str,
) -> Result<()> {
    match field {
        "auto_start" => partial.auto_start = Some(parse_bool_value(value)?),
        "bind_address" => partial.bind_address = Some(value.to_string()),
        "bind_port" => partial.bind_port = Some(parse_int_value(value)?),
        "port_attempts" => partial.port_attempts = Some(parse_int_value(value)?),
        "token" => partial.token = Some(value.to_string()),
        _ => bail!("unknown api setting '{field}'"),
    }
    let _ = typ;
    Ok(())
}

fn apply_tui_value(
    partial: &mut crate::settings::SettingsTuiPartial,
    field: &str,
    value: &str,
    typ: &str,
) -> Result<()> {
    match field {
        "refresh_rate" => partial.refresh_rate = Some(value.to_string()),
        "tick_rate" => partial.tick_rate = Some(value.to_string()),
        "stat_history" => partial.stat_history = Some(parse_int_value(value)?),
        "message_duration" => partial.message_duration = Some(value.to_string()),
        _ => bail!("unknown tui setting '{field}'"),
    }
    let _ = typ;
    Ok(())
}

fn apply_supervisor_value(
    partial: &mut crate::settings::SettingsSupervisorPartial,
    field: &str,
    value: &str,
    typ: &str,
) -> Result<()> {
    match field {
        "ready_check_interval" => partial.ready_check_interval = Some(value.to_string()),
        "file_watch_debounce" => partial.file_watch_debounce = Some(value.to_string()),
        "log_flush_interval" => partial.log_flush_interval = Some(value.to_string()),
        "stop_timeout" => partial.stop_timeout = Some(value.to_string()),
        "restart_delay" => partial.restart_delay = Some(value.to_string()),
        "cron_check_interval" => partial.cron_check_interval = Some(value.to_string()),
        "watch_interval" => partial.watch_interval = Some(value.to_string()),
        "watch_poll_interval" => partial.watch_poll_interval = Some(value.to_string()),
        "http_client_timeout" => partial.http_client_timeout = Some(value.to_string()),
        "port_bump_attempts" => partial.port_bump_attempts = Some(parse_int_value(value)?),
        "container" => partial.container = Some(parse_bool_value(value)?),
        "cleanup_orphans" => partial.cleanup_orphans = Some(parse_bool_value(value)?),
        "user" => partial.user = Some(value.to_string()),
        "cpu_violation_threshold" => {
            partial.cpu_violation_threshold = Some(parse_int_value(value)?)
        }
        _ => bail!("unknown supervisor setting '{field}'"),
    }
    let _ = typ;
    Ok(())
}

fn apply_proxy_value(
    partial: &mut crate::settings::SettingsProxyPartial,
    field: &str,
    value: &str,
    typ: &str,
) -> Result<()> {
    match field {
        "enable" => partial.enable = Some(parse_bool_value(value)?),
        "tld" => partial.tld = Some(value.to_string()),
        "host" => partial.host = Some(value.to_string()),
        "port" => partial.port = Some(parse_int_value(value)?),
        "https" => partial.https = Some(parse_bool_value(value)?),
        "tls_cert" => partial.tls_cert = Some(value.to_string()),
        "tls_key" => partial.tls_key = Some(value.to_string()),
        "auto_trust" => partial.auto_trust = Some(parse_bool_value(value)?),
        "auto_start" => partial.auto_start = Some(parse_bool_value(value)?),
        "auto_start_timeout" => partial.auto_start_timeout = Some(value.to_string()),
        "sync_hosts" => partial.sync_hosts = Some(parse_bool_value(value)?),
        "wildcard" => partial.wildcard = Some(parse_bool_value(value)?),
        "worktree" => partial.worktree = Some(parse_bool_value(value)?),
        "lan" => partial.lan = Some(parse_bool_value(value)?),
        "lan_ip" => partial.lan_ip = Some(value.to_string()),
        _ => bail!("unknown proxy setting '{field}'"),
    }
    let _ = typ;
    Ok(())
}

async fn resolve_config_path(global: bool, local: bool, project: bool) -> Result<PathBuf> {
    if global && (local || project) {
        bail!("cannot combine --global with --local or --project");
    }
    if global {
        return Ok(env::PITCHFORK_GLOBAL_CONFIG_USER.clone());
    }
    resolve_project_config_path(local, project, false).await
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_len = a.chars().count();
    let b_len = b.chars().count();
    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut matrix = vec![vec![0; b_len + 1]; a_len + 1];
    for (i, row) in matrix.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, val) in matrix[0].iter_mut().enumerate().take(b_len + 1) {
        *val = j;
    }

    for (i, a_char) in a.chars().enumerate() {
        for (j, b_char) in b.chars().enumerate() {
            let cost = if a_char == b_char { 0 } else { 1 };
            matrix[i + 1][j + 1] = (matrix[i][j + 1] + 1)
                .min(matrix[i + 1][j] + 1)
                .min(matrix[i][j] + cost);
        }
    }

    matrix[a_len][b_len]
}

/// Best-effort notification to the supervisor to reload settings.
///
/// If the supervisor is running, sends a ReloadConfig IPC request so it
/// picks up the config change immediately. If the supervisor is not running,
/// silently succeeds (settings will be fresh on next supervisor start).
async fn notify_supervisor_reload() {
    use crate::ipc::client::IpcClient;
    match IpcClient::connect(false).await {
        Ok(ipc) => {
            if let Err(e) = ipc.reload_config().await {
                debug!("failed to notify supervisor of config reload: {e}");
            }
        }
        Err(_) => {
            debug!("supervisor not running, skipping config reload notification");
        }
    }
}
