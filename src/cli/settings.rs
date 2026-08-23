use crate::Result;
use crate::cli::daemons::resolve_config_path;
use crate::cli::json_output::{JsonSettingEntry, print_json};
use crate::pitchfork_toml::PitchforkToml;
use crate::settings::{SettingsPartial, settings_resolved};
use miette::{IntoDiagnostic, bail};
use usage_rs::config::{PropMeta, Registry, Resolved, SourceKind, Ty};

const LOG_LEVEL_VALUES: &[&str] = &["trace", "debug", "info", "warn", "error"];

/// The settings registry: what the derive on `crate::settings::Settings`
/// generated, replacing the old build-time SETTINGS_META table.
const REGISTRY: Registry = crate::settings::Settings::SETTINGS_REGISTRY;

/// View and modify pitchfork settings
#[derive(Debug, usage_rs::Args)]
#[usage(
    verbatim_doc_comment,
    args_conflicts_with_subcommands = true,
    long_about = "\
View and modify pitchfork settings

Settings can be configured in multiple ways (in order of precedence):
1. Environment variables (highest priority)
2. Project-level pitchfork.toml or pitchfork.local.toml in [settings] section
3. User-level ~/.config/pitchfork/config.toml in [settings] section
4. System-level /etc/pitchfork/config.toml in [settings] section
5. Built-in defaults (lowest priority)

Subcommands:

    list     List all available settings with types and defaults
    get      Get the current value of a setting
    set      Set a setting value in a config file
    explain  Show where a setting's value came from

Examples:

    pitchfork settings                            Show all current settings
    pitchfork settings list                       List all available settings
    pitchfork settings get general.log_level      Get a specific setting
    pitchfork settings explain general.log_level  Show what set it
    pitchfork settings set general.log_level debug
    pitchfork settings set web.auto_start true --global
    pitchfork settings set supervisor.stop_timeout 10s --local
    pitchfork settings set supervisor.stop_timeout 10s --project"
)]
pub struct Settings {
    #[usage(subcommand)]
    command: Option<Commands>,

    /// Output in JSON format
    #[usage(long)]
    json: bool,
}

#[derive(Debug, usage_rs::Subcommands)]
enum Commands {
    /// List all available settings with types and defaults
    #[usage(alias = "ls")]
    List(ListCmd),
    /// Get the current value of a setting
    Get(GetCmd),
    /// Set a setting value in a config file
    Set(SetCmd),
    /// Show where a setting's value came from
    #[usage(alias = "why")]
    Explain(ExplainCmd),
}

/// List all available settings with types and defaults
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
pub struct ListCmd {
    /// Only show settings in a specific group (e.g., "general", "web", "supervisor")
    #[usage(long)]
    group: Option<String>,

    /// Output in JSON format
    #[usage(long)]
    json: bool,
}

/// Get the current value of a setting
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
pub struct GetCmd {
    /// Setting key in dot notation (e.g., general.log_level, web.auto_start)
    key: String,

    /// Output in JSON format
    #[usage(long)]
    json: bool,
}

/// Show where a setting's value came from
///
/// Names the exact place that won — the variable, or the file and the key
/// inside it — along with everything else that was considered and every other
/// way the setting can be reached.
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
pub struct ExplainCmd {
    /// Setting key in dot notation (e.g., general.log_level, web.auto_start)
    key: String,
}

/// Set a setting value in a config file
#[derive(Debug, usage_rs::Args)]
#[usage(verbatim_doc_comment)]
pub struct SetCmd {
    /// Setting key in dot notation (e.g., general.log_level, web.auto_start)
    key: String,
    /// Value to set (type must match the setting: string, integer, boolean, or duration)
    value: String,
    /// Write to the user-level global config (~/.config/pitchfork/config.toml)
    #[usage(long)]
    global: bool,
    /// Write to the project-level pitchfork.local.toml (overrides pitchfork.toml)
    #[usage(long)]
    local: bool,
    /// Write to the project-level pitchfork.toml (default if no flag specified)
    #[usage(long)]
    project: bool,
}

impl Settings {
    pub async fn run(self) -> Result<()> {
        match self.command {
            Some(Commands::List(cmd)) => cmd.run(),
            Some(Commands::Get(cmd)) => cmd.run(),
            Some(Commands::Set(cmd)) => cmd.run().await,
            Some(Commands::Explain(cmd)) => cmd.run(),
            None => show_all_settings(self.json),
        }
    }
}

/// The setting's type as the old SETTINGS_META spelled it, for output
/// compatibility ("Bool", "Integer", "String", "Duration", "Path").
fn legacy_type(meta: &PropMeta) -> &'static str {
    match meta.ty.inner() {
        Ty::Bool => "Bool",
        Ty::Int | Ty::Uint => "Integer",
        Ty::Duration => "Duration",
        Ty::Path => "Path",
        _ => "String",
    }
}

/// The declared default, written the way settings.toml used to write it.
fn default_string(meta: &PropMeta) -> Option<String> {
    meta.default.map(|c| c.to_value().display())
}

/// The current value of a setting, as text.
fn current_value(resolved: &Resolved, key: &str) -> String {
    resolved
        .get_key(key)
        .map(|v| v.display())
        .unwrap_or_default()
}

/// Whether nothing but the declared default supplied this value.
///
/// Asked of the merge, not of the text. Comparing the rendered value against
/// the rendered default agrees with the origin almost always and is wrong in
/// the one case worth reporting: `log_level = "info"` written into
/// `pitchfork.toml` on purpose, over a `~/.config` that says `debug`. That is
/// an override the user needs to see, and it renders identically to the
/// default it happens to equal.
fn is_default(resolved: &Resolved, key: &str) -> bool {
    resolved
        .origin_key(key)
        .is_none_or(|origin| origin.kind == SourceKind::DEFAULTS)
}

/// Where the winning value came from, named the way the user could act on
/// it: `PITCHFORK_LOG`, `/path/to/pitchfork.toml#general.log_level`.
fn origin_string(resolved: &Resolved, key: &str) -> Option<String> {
    resolved
        .origin_key(key)
        .map(|origin| origin.describe().to_string())
}

fn json_entry(resolved: &Resolved, meta: &PropMeta) -> JsonSettingEntry {
    JsonSettingEntry {
        key: meta.key.to_string(),
        value: current_value(resolved, meta.key),
        default: default_string(meta),
        r#type: Some(legacy_type(meta)),
        env_var: meta.envs.first().copied(),
        origin: origin_string(resolved, meta.key),
    }
}

fn in_group(key: &str, group: &Option<String>) -> bool {
    match group {
        Some(group) => key.starts_with(&format!("{group}.")) || key == group,
        None => true,
    }
}

impl ListCmd {
    fn run(&self) -> Result<()> {
        let resolved = settings_resolved();
        if self.json {
            let entries: Vec<JsonSettingEntry> = REGISTRY
                .props
                .iter()
                .filter(|meta| in_group(meta.key, &self.group))
                .map(|meta| json_entry(&resolved, meta))
                .collect();
            return print_json(&entries);
        }
        for meta in REGISTRY.props {
            if !in_group(meta.key, &self.group) {
                continue;
            }
            let default = default_string(meta).unwrap_or_else(|| "(none)".to_string());
            let env_hint = meta
                .envs
                .first()
                .map(|e| format!(" [{e}]"))
                .unwrap_or_default();
            println!(
                "{} ({}) default={default}{env_hint}",
                meta.key,
                legacy_type(meta)
            );
            if let Some(desc) = meta.help.and_then(|h| h.lines().next()) {
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

        let resolved = settings_resolved();
        let value = current_value(&resolved, key);
        if self.json {
            let meta = REGISTRY.lookup(key).map(|found| REGISTRY.get(found.id));
            return print_json(&JsonSettingEntry {
                key: key.clone(),
                value,
                default: meta.and_then(default_string),
                r#type: meta.map(legacy_type),
                env_var: meta.and_then(|m| m.envs.first().copied()),
                origin: origin_string(&resolved, key),
            });
        }
        println!("{value}");
        Ok(())
    }
}

impl ExplainCmd {
    fn run(&self) -> Result<()> {
        let key = &self.key;
        // For the near-miss suggestion. `explain` returns None for a key the
        // registry does not have and deliberately does not guess what was
        // meant, which is this command's job and already written.
        validate_setting_key(key)?;

        let resolved = settings_resolved();
        let explanation = usage_rs::config::explain(&resolved, key)
            .ok_or_else(|| miette::miette!("unknown setting '{key}'"))?;
        print!("{explanation}");
        Ok(())
    }
}

impl SetCmd {
    async fn run(&self) -> Result<()> {
        let key = &self.key;
        let value = &self.value;
        validate_setting_key(key)?;
        validate_setting_value(key, value)?;

        let config_path = resolve_config_path(self.global, self.local, self.project, false).await?;

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

fn show_all_settings(json: bool) -> Result<()> {
    let resolved = settings_resolved();

    if json {
        let entries: Vec<JsonSettingEntry> = REGISTRY
            .props
            .iter()
            .map(|meta| json_entry(&resolved, meta))
            .collect();
        return print_json(&entries);
    }

    let mut current_group = String::new();
    for meta in REGISTRY.props {
        let group = meta.key.split('.').next().unwrap_or("");
        if group != current_group {
            if !current_group.is_empty() {
                println!();
            }
            current_group = group.to_string();
            println!("[settings.{group}]");
        }

        let field_name = meta
            .key
            .split_once('.')
            .map(|(_, rest)| rest)
            .unwrap_or(meta.key);
        let current = current_value(&resolved, meta.key);

        if is_default(&resolved, meta.key) {
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
    if REGISTRY.lookup(key).is_some() {
        return Ok(());
    }

    let mut suggestions: Vec<&str> = REGISTRY
        .props
        .iter()
        .map(|meta| meta.key)
        .filter(|k| {
            let dist = levenshtein_distance(key, k);
            dist <= 3 || k.contains(key)
        })
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
    let found = REGISTRY.lookup(key).unwrap();
    let meta = REGISTRY.get(found.id);

    match legacy_type(meta) {
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

/// Write one setting into the partial by its dotted key.
///
/// Generic over the whole registry (the old code carried a hand-written match
/// arm per setting): the partial round-trips through a TOML table, the key's
/// path is created as nested tables, and the typed value is inserted.
fn apply_setting_to_partial(partial: &mut SettingsPartial, key: &str, value: &str) -> Result<()> {
    let found = REGISTRY
        .lookup(key)
        .expect("key was validated by validate_setting_key");
    let meta = REGISTRY.get(found.id);

    let toml_value =
        match legacy_type(meta) {
            "Bool" => toml::Value::Boolean(match value {
                "true" => true,
                "false" => false,
                _ => bail!("invalid boolean value '{value}'. Expected 'true' or 'false'"),
            }),
            "Integer" => toml::Value::Integer(value.parse::<i64>().map_err(|_| {
                miette::miette!("invalid integer value '{value}'. Expected a number")
            })?),
            _ => toml::Value::String(value.to_string()),
        };

    let mut table = toml::Table::try_from(&*partial)
        .map_err(|e| miette::miette!("failed to serialize settings: {e}"))?;
    let parts: Vec<&str> = meta.key.split('.').collect();
    let mut cursor = &mut table;
    for part in &parts[..parts.len() - 1] {
        cursor = cursor
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| miette::miette!("setting group '{part}' is not a table"))?;
    }
    cursor.insert(parts[parts.len() - 1].to_string(), toml_value);
    *partial = table
        .try_into()
        .map_err(|e| miette::miette!("failed to apply setting '{key}': {e}"))?;

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use usage_rs::config::{EnvLayer, Layers, resolve};

    /// The environment is described rather than reached for, so these tests do
    /// not touch the process (and need no mutex to run alongside each other).
    fn resolved_with(env: &[(&str, &str)]) -> Resolved {
        let layer = EnvLayer::new(
            env.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<_>>(),
        );
        resolve(REGISTRY, Layers::new().then(&layer)).expect("resolves")
    }

    #[test]
    fn an_explicit_value_equal_to_the_default_is_not_the_default() {
        // The case that made `pitchfork settings` misreport a merge: the text
        // is identical either way, and only the origin can tell them apart.
        let default = resolved_with(&[]);
        let explicit = resolved_with(&[("PITCHFORK_LOG", "info")]);

        assert_eq!(current_value(&default, "general.log_level"), "info");
        assert_eq!(current_value(&explicit, "general.log_level"), "info");

        assert!(is_default(&default, "general.log_level"));
        assert!(
            !is_default(&explicit, "general.log_level"),
            "PITCHFORK_LOG=info is an override, not an absence"
        );
    }

    #[test]
    fn an_entry_names_where_its_value_came_from() {
        let resolved = resolved_with(&[("PITCHFORK_LOG", "debug")]);
        assert_eq!(
            origin_string(&resolved, "general.log_level").as_deref(),
            Some("PITCHFORK_LOG"),
            "named as the user set it, not as \"the environment\""
        );
        assert_eq!(
            origin_string(&resolved, "general.interval").as_deref(),
            Some("the default")
        );
    }

    #[test]
    fn explain_answers_for_every_setting_the_registry_declares() {
        // `explain` returns None only for a key the registry does not have, so
        // this also pins that every key `settings list` prints is explainable.
        let resolved = resolved_with(&[]);
        for meta in REGISTRY.props {
            assert!(
                usage_rs::config::explain(&resolved, meta.key).is_some(),
                "no explanation for {}",
                meta.key
            );
        }
        assert!(usage_rs::config::explain(&resolved, "general.nonesuch").is_none());
    }
}
