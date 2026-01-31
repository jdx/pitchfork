use crate::Result;
use crate::daemon::Daemon;
use crate::env::PITCHFORK_LOGS_DIR;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::{
    CronRetrigger, PitchforkToml, PitchforkTomlAuto, PitchforkTomlCron, PitchforkTomlDaemon, Retry,
};
use crate::procs::{PROCS, ProcessStats};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Maximum number of stat samples to keep for each daemon (e.g., 60 samples at 2s intervals = 2 minutes)
const MAX_STAT_HISTORY: usize = 60;

/// Convert character index to byte index for UTF-8 strings
fn char_to_byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// A snapshot of stats at a point in time
#[derive(Debug, Clone, Copy)]
pub struct StatsSnapshot {
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
}

impl From<&ProcessStats> for StatsSnapshot {
    fn from(stats: &ProcessStats) -> Self {
        Self {
            cpu_percent: stats.cpu_percent,
            memory_bytes: stats.memory_bytes,
            disk_read_bytes: stats.disk_read_bytes,
            disk_write_bytes: stats.disk_write_bytes,
        }
    }
}

/// Historical stats for a daemon
#[derive(Debug, Clone, Default)]
pub struct StatsHistory {
    pub samples: VecDeque<StatsSnapshot>,
}

impl StatsHistory {
    pub fn push(&mut self, snapshot: StatsSnapshot) {
        self.samples.push_back(snapshot);
        while self.samples.len() > MAX_STAT_HISTORY {
            self.samples.pop_front();
        }
    }

    pub fn cpu_values(&self) -> Vec<f32> {
        self.samples.iter().map(|s| s.cpu_percent).collect()
    }

    pub fn memory_values(&self) -> Vec<u64> {
        self.samples.iter().map(|s| s.memory_bytes).collect()
    }

    pub fn disk_read_values(&self) -> Vec<u64> {
        self.samples.iter().map(|s| s.disk_read_bytes).collect()
    }

    pub fn disk_write_values(&self) -> Vec<u64> {
        self.samples.iter().map(|s| s.disk_write_bytes).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Dashboard,
    Logs,
    Help,
    Confirm,
    Loading,
    Details,
    ConfigEditor,
    ConfigFileSelect,
}

/// Edit mode for the config editor
#[derive(Debug, Clone, PartialEq)]
pub enum EditMode {
    Create,
    Edit { original_id: String },
}

/// Form field value types
#[derive(Debug, Clone)]
pub enum FormFieldValue {
    Text(String),
    OptionalText(Option<String>),
    Number(u32),
    OptionalNumber(Option<u64>),
    OptionalPort(Option<u16>),
    #[allow(dead_code)]
    Boolean(bool),
    OptionalBoolean(Option<bool>),
    AutoBehavior(Vec<PitchforkTomlAuto>),
    Retrigger(CronRetrigger),
    StringList(Vec<String>),
}

/// A form field with metadata
#[derive(Debug, Clone)]
pub struct FormField {
    pub name: &'static str,
    pub label: &'static str,
    pub value: FormFieldValue,
    pub required: bool,
    #[allow(dead_code)]
    pub help_text: &'static str,
    pub error: Option<String>,
    pub editing: bool,
    pub cursor: usize,
}

impl FormField {
    fn text(name: &'static str, label: &'static str, help: &'static str, required: bool) -> Self {
        Self {
            name,
            label,
            value: FormFieldValue::Text(String::new()),
            required,
            help_text: help,
            error: None,
            editing: false,
            cursor: 0,
        }
    }

    fn optional_text(name: &'static str, label: &'static str, help: &'static str) -> Self {
        Self {
            name,
            label,
            value: FormFieldValue::OptionalText(None),
            required: false,
            help_text: help,
            error: None,
            editing: false,
            cursor: 0,
        }
    }

    fn number(name: &'static str, label: &'static str, help: &'static str, default: u32) -> Self {
        Self {
            name,
            label,
            value: FormFieldValue::Number(default),
            required: false,
            help_text: help,
            error: None,
            editing: false,
            cursor: 0,
        }
    }

    fn optional_number(name: &'static str, label: &'static str, help: &'static str) -> Self {
        Self {
            name,
            label,
            value: FormFieldValue::OptionalNumber(None),
            required: false,
            help_text: help,
            error: None,
            editing: false,
            cursor: 0,
        }
    }

    fn optional_port(name: &'static str, label: &'static str, help: &'static str) -> Self {
        Self {
            name,
            label,
            value: FormFieldValue::OptionalPort(None),
            required: false,
            help_text: help,
            error: None,
            editing: false,
            cursor: 0,
        }
    }

    fn optional_bool(name: &'static str, label: &'static str, help: &'static str) -> Self {
        Self {
            name,
            label,
            value: FormFieldValue::OptionalBoolean(None),
            required: false,
            help_text: help,
            error: None,
            editing: false,
            cursor: 0,
        }
    }

    fn auto_behavior(name: &'static str, label: &'static str, help: &'static str) -> Self {
        Self {
            name,
            label,
            value: FormFieldValue::AutoBehavior(vec![]),
            required: false,
            help_text: help,
            error: None,
            editing: false,
            cursor: 0,
        }
    }

    fn retrigger(name: &'static str, label: &'static str, help: &'static str) -> Self {
        Self {
            name,
            label,
            value: FormFieldValue::Retrigger(CronRetrigger::Finish),
            required: false,
            help_text: help,
            error: None,
            editing: false,
            cursor: 0,
        }
    }

    fn string_list(name: &'static str, label: &'static str, help: &'static str) -> Self {
        Self {
            name,
            label,
            value: FormFieldValue::StringList(vec![]),
            required: false,
            help_text: help,
            error: None,
            editing: false,
            cursor: 0,
        }
    }

    pub fn get_text(&self) -> String {
        match &self.value {
            FormFieldValue::Text(s) => s.clone(),
            FormFieldValue::OptionalText(Some(s)) => s.clone(),
            FormFieldValue::OptionalText(None) => String::new(),
            FormFieldValue::Number(n) => n.to_string(),
            FormFieldValue::OptionalNumber(Some(n)) => n.to_string(),
            FormFieldValue::OptionalNumber(None) => String::new(),
            FormFieldValue::OptionalPort(Some(p)) => p.to_string(),
            FormFieldValue::OptionalPort(None) => String::new(),
            FormFieldValue::StringList(v) => v.join(", "),
            _ => String::new(),
        }
    }

    pub fn set_text(&mut self, text: String) {
        match &mut self.value {
            FormFieldValue::Text(s) => *s = text,
            FormFieldValue::OptionalText(opt) => {
                *opt = if text.is_empty() { None } else { Some(text) };
            }
            FormFieldValue::Number(n) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    *n = 0;
                    self.error = None;
                } else {
                    match trimmed.parse() {
                        Ok(value) => {
                            *n = value;
                            self.error = None;
                        }
                        Err(_) => {
                            *n = 0;
                            self.error = Some("Invalid number".to_string());
                        }
                    }
                }
            }
            FormFieldValue::OptionalNumber(opt) => {
                *opt = text.parse().ok();
            }
            FormFieldValue::OptionalPort(opt) => {
                *opt = text.parse().ok();
            }
            FormFieldValue::StringList(v) => {
                *v = text
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            _ => {}
        }
    }

    pub fn is_text_editable(&self) -> bool {
        matches!(
            self.value,
            FormFieldValue::Text(_)
                | FormFieldValue::OptionalText(_)
                | FormFieldValue::Number(_)
                | FormFieldValue::OptionalNumber(_)
                | FormFieldValue::OptionalPort(_)
                | FormFieldValue::StringList(_)
        )
    }
}

/// State for the daemon config editor
#[derive(Debug, Clone)]
pub struct EditorState {
    pub mode: EditMode,
    pub daemon_id: String,
    pub daemon_id_editing: bool,
    pub daemon_id_cursor: usize,
    pub daemon_id_error: Option<String>,
    pub fields: Vec<FormField>,
    pub focused_field: usize,
    pub config_path: PathBuf,
    pub unsaved_changes: bool,
    #[allow(dead_code)]
    pub scroll_offset: usize,
}

impl EditorState {
    pub fn new_create(config_path: PathBuf) -> Self {
        Self {
            mode: EditMode::Create,
            daemon_id: String::new(),
            daemon_id_editing: true,
            daemon_id_cursor: 0,
            daemon_id_error: None,
            fields: Self::default_fields(),
            focused_field: 0,
            config_path,
            unsaved_changes: false,
            scroll_offset: 0,
        }
    }

    pub fn new_edit(daemon_id: String, config: &PitchforkTomlDaemon, config_path: PathBuf) -> Self {
        Self {
            mode: EditMode::Edit {
                original_id: daemon_id.clone(),
            },
            daemon_id,
            daemon_id_editing: false,
            daemon_id_cursor: 0,
            daemon_id_error: None,
            fields: Self::fields_from_config(config),
            focused_field: 0,
            config_path,
            unsaved_changes: false,
            scroll_offset: 0,
        }
    }

    fn default_fields() -> Vec<FormField> {
        vec![
            FormField::text(
                "run",
                "Run Command",
                "Command to execute. Prepend 'exec' to avoid shell overhead.",
                true,
            ),
            FormField::auto_behavior(
                "auto",
                "Auto Behavior",
                "Auto start/stop based on directory hooks.",
            ),
            FormField::number(
                "retry",
                "Retry Count",
                "Number of retry attempts on failure (0 = no retries).",
                0,
            ),
            FormField::optional_number(
                "ready_delay",
                "Ready Delay (ms)",
                "Milliseconds to wait before considering daemon ready.",
            ),
            FormField::optional_text(
                "ready_output",
                "Ready Output Pattern",
                "Regex pattern in stdout/stderr indicating readiness.",
            ),
            FormField::optional_text(
                "ready_http",
                "Ready HTTP URL",
                "HTTP URL to poll for readiness (expects 2xx).",
            ),
            FormField::optional_port(
                "ready_port",
                "Ready Port",
                "TCP port to check for readiness (1-65535).",
            ),
            FormField::optional_bool(
                "boot_start",
                "Start on Boot",
                "Automatically start this daemon on system boot.",
            ),
            FormField::string_list(
                "depends",
                "Dependencies",
                "Comma-separated daemon names that must start first.",
            ),
            FormField::string_list(
                "watch",
                "Watch Files",
                "Comma-separated glob patterns to watch for auto-restart.",
            ),
            FormField::optional_text(
                "cron_schedule",
                "Cron Schedule",
                "Cron expression (e.g., '*/5 * * * *' for every 5 minutes).",
            ),
            FormField::retrigger(
                "cron_retrigger",
                "Cron Retrigger",
                "Behavior when cron triggers while previous run is active.",
            ),
        ]
    }

    fn fields_from_config(config: &PitchforkTomlDaemon) -> Vec<FormField> {
        let mut fields = Self::default_fields();

        for field in &mut fields {
            match field.name {
                "run" => field.value = FormFieldValue::Text(config.run.clone()),
                "auto" => field.value = FormFieldValue::AutoBehavior(config.auto.clone()),
                "retry" => field.value = FormFieldValue::Number(config.retry.count()),
                "ready_delay" => field.value = FormFieldValue::OptionalNumber(config.ready_delay),
                "ready_output" => {
                    field.value = FormFieldValue::OptionalText(config.ready_output.clone())
                }
                "ready_http" => {
                    field.value = FormFieldValue::OptionalText(config.ready_http.clone())
                }
                "ready_port" => field.value = FormFieldValue::OptionalPort(config.ready_port),
                "boot_start" => field.value = FormFieldValue::OptionalBoolean(config.boot_start),
                "depends" => field.value = FormFieldValue::StringList(config.depends.clone()),
                "watch" => field.value = FormFieldValue::StringList(config.watch.clone()),
                "cron_schedule" => {
                    field.value = FormFieldValue::OptionalText(
                        config.cron.as_ref().map(|c| c.schedule.clone()),
                    );
                }
                "cron_retrigger" => {
                    field.value = FormFieldValue::Retrigger(
                        config
                            .cron
                            .as_ref()
                            .map(|c| c.retrigger)
                            .unwrap_or(CronRetrigger::Finish),
                    );
                }
                _ => {}
            }
        }

        fields
    }

    pub fn to_daemon_config(&self) -> PitchforkTomlDaemon {
        let mut config = PitchforkTomlDaemon {
            run: String::new(),
            auto: vec![],
            cron: None,
            retry: Retry(0),
            ready_delay: None,
            ready_output: None,
            ready_http: None,
            ready_port: None,
            ready_cmd: None,
            boot_start: None,
            depends: vec![],
            watch: vec![],
            path: Some(self.config_path.clone()),
        };

        let mut cron_schedule: Option<String> = None;
        let mut cron_retrigger = CronRetrigger::Finish;

        for field in &self.fields {
            match (field.name, &field.value) {
                ("run", FormFieldValue::Text(s)) => config.run = s.clone(),
                ("auto", FormFieldValue::AutoBehavior(v)) => config.auto = v.clone(),
                ("retry", FormFieldValue::Number(n)) => config.retry = Retry(*n),
                ("ready_delay", FormFieldValue::OptionalNumber(n)) => config.ready_delay = *n,
                ("ready_output", FormFieldValue::OptionalText(s)) => {
                    config.ready_output = s.clone()
                }
                ("ready_http", FormFieldValue::OptionalText(s)) => config.ready_http = s.clone(),
                ("ready_port", FormFieldValue::OptionalPort(p)) => config.ready_port = *p,
                ("boot_start", FormFieldValue::OptionalBoolean(b)) => config.boot_start = *b,
                ("depends", FormFieldValue::StringList(v)) => config.depends = v.clone(),
                ("watch", FormFieldValue::StringList(v)) => config.watch = v.clone(),
                ("cron_schedule", FormFieldValue::OptionalText(s)) => cron_schedule = s.clone(),
                ("cron_retrigger", FormFieldValue::Retrigger(r)) => cron_retrigger = *r,
                _ => {}
            }
        }

        if let Some(schedule) = cron_schedule {
            config.cron = Some(PitchforkTomlCron {
                schedule,
                retrigger: cron_retrigger,
            });
        }

        config
    }

    pub fn next_field(&mut self) {
        // Stop editing current field if text editing
        if let Some(field) = self.fields.get_mut(self.focused_field) {
            field.editing = false;
        }

        // When leaving daemon_id editing, don't increment - just move to first form field
        if self.daemon_id_editing {
            self.daemon_id_editing = false;
            return;
        }

        if self.focused_field < self.fields.len() - 1 {
            self.focused_field += 1;
        }
    }

    pub fn prev_field(&mut self) {
        // Stop editing current field if text editing
        if let Some(field) = self.fields.get_mut(self.focused_field) {
            field.editing = false;
        }
        self.daemon_id_editing = false;

        if self.focused_field > 0 {
            self.focused_field -= 1;
        }
    }

    pub fn toggle_current_field(&mut self) {
        if let Some(field) = self.fields.get_mut(self.focused_field) {
            let toggled = match &mut field.value {
                FormFieldValue::Boolean(b) => {
                    *b = !*b;
                    true
                }
                FormFieldValue::OptionalBoolean(opt) => {
                    *opt = match opt {
                        None => Some(true),
                        Some(true) => Some(false),
                        Some(false) => None,
                    };
                    true
                }
                FormFieldValue::AutoBehavior(v) => {
                    // Cycle through: [] -> [Start] -> [Stop] -> [Start, Stop] -> []
                    let has_start = v.contains(&PitchforkTomlAuto::Start);
                    let has_stop = v.contains(&PitchforkTomlAuto::Stop);
                    *v = match (has_start, has_stop) {
                        (false, false) => vec![PitchforkTomlAuto::Start],
                        (true, false) => vec![PitchforkTomlAuto::Stop],
                        (false, true) => vec![PitchforkTomlAuto::Start, PitchforkTomlAuto::Stop],
                        (true, true) => vec![],
                    };
                    true
                }
                FormFieldValue::Retrigger(r) => {
                    *r = match r {
                        CronRetrigger::Finish => CronRetrigger::Always,
                        CronRetrigger::Always => CronRetrigger::Success,
                        CronRetrigger::Success => CronRetrigger::Fail,
                        CronRetrigger::Fail => CronRetrigger::Finish,
                    };
                    true
                }
                _ => false,
            };
            if toggled {
                self.unsaved_changes = true;
            }
        }
    }

    pub fn start_editing(&mut self) {
        if let Some(field) = self.fields.get_mut(self.focused_field) {
            if field.is_text_editable() {
                field.editing = true;
                field.cursor = field.get_text().chars().count();
            } else {
                // For non-text fields, toggle them
                self.toggle_current_field();
            }
        }
    }

    pub fn stop_editing(&mut self) {
        if let Some(field) = self.fields.get_mut(self.focused_field) {
            field.editing = false;
        }
        self.daemon_id_editing = false;
    }

    pub fn is_editing(&self) -> bool {
        self.daemon_id_editing
            || self
                .fields
                .get(self.focused_field)
                .map(|f| f.editing)
                .unwrap_or(false)
    }

    pub fn text_push(&mut self, c: char) {
        if self.daemon_id_editing {
            let byte_idx = char_to_byte_index(&self.daemon_id, self.daemon_id_cursor);
            self.daemon_id.insert(byte_idx, c);
            self.daemon_id_cursor += 1;
            self.unsaved_changes = true;
        } else if let Some(field) = self.fields.get_mut(self.focused_field)
            && field.editing
        {
            let mut text = field.get_text();
            let byte_idx = char_to_byte_index(&text, field.cursor);
            text.insert(byte_idx, c);
            field.cursor += 1;
            field.set_text(text);
            self.unsaved_changes = true;
        }
    }

    pub fn text_pop(&mut self) {
        if self.daemon_id_editing && self.daemon_id_cursor > 0 {
            self.daemon_id_cursor -= 1;
            let byte_idx = char_to_byte_index(&self.daemon_id, self.daemon_id_cursor);
            self.daemon_id.remove(byte_idx);
            self.unsaved_changes = true;
        } else if let Some(field) = self.fields.get_mut(self.focused_field)
            && field.editing
            && field.cursor > 0
        {
            let mut text = field.get_text();
            field.cursor -= 1;
            let byte_idx = char_to_byte_index(&text, field.cursor);
            text.remove(byte_idx);
            field.set_text(text);
            // For Number fields, sync cursor to end if value defaulted to "0"
            // This handles the case where backspacing to empty makes value 0
            if matches!(field.value, FormFieldValue::Number(_)) {
                field.cursor = field.get_text().chars().count();
            }
            self.unsaved_changes = true;
        }
    }

    pub fn validate(&mut self) -> bool {
        let mut valid = true;

        // Validate daemon ID
        self.daemon_id_error = None;
        if self.daemon_id.is_empty() {
            self.daemon_id_error = Some("Name is required".to_string());
            valid = false;
        } else if !self
            .daemon_id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            self.daemon_id_error =
                Some("Only letters, digits, hyphens, and underscores allowed".to_string());
            valid = false;
        }

        // Validate fields
        for field in &mut self.fields {
            field.error = None;

            match (field.name, &field.value) {
                ("run", FormFieldValue::Text(s)) if s.is_empty() => {
                    field.error = Some("Required".to_string());
                    valid = false;
                }
                ("ready_port", FormFieldValue::OptionalPort(Some(p))) if *p == 0 => {
                    field.error = Some("Port must be 1-65535".to_string());
                    valid = false;
                }
                ("ready_http", FormFieldValue::OptionalText(Some(url)))
                    if !(url.starts_with("http://") || url.starts_with("https://")) =>
                {
                    field.error = Some("Must start with http:// or https://".to_string());
                    valid = false;
                }
                _ => {}
            }
        }

        valid
    }
}

/// State for config file selection
#[derive(Debug, Clone)]
pub struct ConfigFileSelector {
    pub files: Vec<PathBuf>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    Stop(String),
    Restart(String),
    Disable(String),
    // Batch operations
    BatchStop(Vec<String>),
    BatchRestart(Vec<String>),
    BatchDisable(Vec<String>),
    // Config editor actions
    DeleteDaemon { id: String, config_path: PathBuf },
    DiscardEditorChanges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortColumn {
    #[default]
    Name,
    Status,
    Cpu,
    Memory,
    Uptime,
}

impl SortColumn {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Status,
            Self::Status => Self::Cpu,
            Self::Cpu => Self::Memory,
            Self::Memory => Self::Uptime,
            Self::Uptime => Self::Name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

impl SortOrder {
    pub fn toggle(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    pub fn indicator(self) -> &'static str {
        match self {
            Self::Ascending => "↑",
            Self::Descending => "↓",
        }
    }
}

pub struct App {
    pub daemons: Vec<Daemon>,
    pub disabled: Vec<String>,
    pub selected: usize,
    pub view: View,
    pub prev_view: View,
    pub log_content: Vec<String>,
    pub log_daemon_id: Option<String>,
    pub log_scroll: usize,
    pub log_follow: bool, // Auto-scroll to bottom as new lines appear
    pub message: Option<String>,
    pub message_time: Option<Instant>,
    pub process_stats: HashMap<u32, ProcessStats>, // PID -> stats
    pub stats_history: HashMap<String, StatsHistory>, // daemon_id -> history
    pub pending_action: Option<PendingAction>,
    pub loading_text: Option<String>,
    pub search_query: String,
    pub search_active: bool,
    // Sorting
    pub sort_column: SortColumn,
    pub sort_order: SortOrder,
    // Log search
    pub log_search_query: String,
    pub log_search_active: bool,
    pub log_search_matches: Vec<usize>, // Line indices that match
    pub log_search_current: usize,      // Current match index
    // Details view daemon (now used for full-page details view from 'l' key)
    pub details_daemon_id: Option<String>,
    // Whether logs are expanded to fill the screen (hides charts)
    pub logs_expanded: bool,
    // Multi-select state
    pub multi_select: HashSet<String>,
    // Config-only daemons (defined in pitchfork.toml but not currently active)
    pub config_daemon_ids: HashSet<String>,
    // Whether to show config-only daemons in the list
    pub show_available: bool,
    // Config editor state
    pub editor_state: Option<EditorState>,
    // Config file selector state
    pub file_selector: Option<ConfigFileSelector>,
}

impl App {
    pub fn new() -> Self {
        Self {
            daemons: Vec::new(),
            disabled: Vec::new(),
            selected: 0,
            view: View::Dashboard,
            prev_view: View::Dashboard,
            log_content: Vec::new(),
            log_daemon_id: None,
            log_scroll: 0,
            log_follow: true,
            message: None,
            message_time: None,
            process_stats: HashMap::new(),
            stats_history: HashMap::new(),
            pending_action: None,
            loading_text: None,
            search_query: String::new(),
            search_active: false,
            sort_column: SortColumn::default(),
            sort_order: SortOrder::default(),
            log_search_query: String::new(),
            log_search_active: false,
            log_search_matches: Vec::new(),
            log_search_current: 0,
            details_daemon_id: None,
            logs_expanded: false,
            multi_select: HashSet::new(),
            config_daemon_ids: HashSet::new(),
            show_available: true, // Show available daemons by default
            editor_state: None,
            file_selector: None,
        }
    }

    pub fn confirm_action(&mut self, action: PendingAction) {
        self.pending_action = Some(action);
        self.prev_view = self.view;
        self.view = View::Confirm;
    }

    pub fn cancel_confirm(&mut self) {
        self.pending_action = None;
        self.view = self.prev_view;
    }

    pub fn take_pending_action(&mut self) -> Option<PendingAction> {
        self.view = self.prev_view;
        self.pending_action.take()
    }

    pub fn start_loading(&mut self, text: impl Into<String>) {
        self.prev_view = self.view;
        self.loading_text = Some(text.into());
        self.view = View::Loading;
    }

    pub fn stop_loading(&mut self) {
        self.loading_text = None;
        self.view = self.prev_view;
    }

    // Search functionality
    pub fn start_search(&mut self) {
        self.search_active = true;
    }

    pub fn end_search(&mut self) {
        self.search_active = false;
    }

    pub fn clear_search(&mut self) {
        self.search_query.clear();
        self.search_active = false;
        self.selected = 0;
    }

    pub fn search_push(&mut self, c: char) {
        self.search_query.push(c);
        // Reset selection when search changes
        self.selected = 0;
    }

    pub fn search_pop(&mut self) {
        self.search_query.pop();
        self.selected = 0;
    }

    pub fn filtered_daemons(&self) -> Vec<&Daemon> {
        let mut filtered: Vec<&Daemon> = if self.search_query.is_empty() {
            self.daemons.iter().collect()
        } else {
            // Use fuzzy matching with SkimMatcherV2
            let matcher = SkimMatcherV2::default();
            let mut scored: Vec<_> = self
                .daemons
                .iter()
                .filter_map(|d| {
                    matcher
                        .fuzzy_match(&d.id, &self.search_query)
                        .map(|score| (d, score))
                })
                .collect();
            // Sort by score descending (best matches first)
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            scored.into_iter().map(|(d, _)| d).collect()
        };

        // Sort the filtered list
        filtered.sort_by(|a, b| {
            let cmp = match self.sort_column {
                SortColumn::Name => a.id.to_lowercase().cmp(&b.id.to_lowercase()),
                SortColumn::Status => {
                    let status_order = |d: &Daemon| match &d.status {
                        crate::daemon_status::DaemonStatus::Running => 0,
                        crate::daemon_status::DaemonStatus::Waiting => 1,
                        crate::daemon_status::DaemonStatus::Stopping => 2,
                        crate::daemon_status::DaemonStatus::Stopped => 3,
                        crate::daemon_status::DaemonStatus::Errored(_)
                        | crate::daemon_status::DaemonStatus::ErroredUnknown => 4,
                        crate::daemon_status::DaemonStatus::Failed(_) => 5,
                    };
                    status_order(a).cmp(&status_order(b))
                }
                SortColumn::Cpu => {
                    let cpu_a = a
                        .pid
                        .and_then(|p| self.get_stats(p))
                        .map(|s| s.cpu_percent)
                        .unwrap_or(0.0);
                    let cpu_b = b
                        .pid
                        .and_then(|p| self.get_stats(p))
                        .map(|s| s.cpu_percent)
                        .unwrap_or(0.0);
                    cpu_a
                        .partial_cmp(&cpu_b)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }
                SortColumn::Memory => {
                    let mem_a = a
                        .pid
                        .and_then(|p| self.get_stats(p))
                        .map(|s| s.memory_bytes)
                        .unwrap_or(0);
                    let mem_b = b
                        .pid
                        .and_then(|p| self.get_stats(p))
                        .map(|s| s.memory_bytes)
                        .unwrap_or(0);
                    mem_a.cmp(&mem_b)
                }
                SortColumn::Uptime => {
                    let up_a = a
                        .pid
                        .and_then(|p| self.get_stats(p))
                        .map(|s| s.uptime_secs)
                        .unwrap_or(0);
                    let up_b = b
                        .pid
                        .and_then(|p| self.get_stats(p))
                        .map(|s| s.uptime_secs)
                        .unwrap_or(0);
                    up_a.cmp(&up_b)
                }
            };
            match self.sort_order {
                SortOrder::Ascending => cmp,
                SortOrder::Descending => cmp.reverse(),
            }
        });

        filtered
    }

    // Sorting
    pub fn cycle_sort(&mut self) {
        // If clicking the same column, toggle order; otherwise switch column
        self.sort_column = self.sort_column.next();
        self.selected = 0;
    }

    pub fn toggle_sort_order(&mut self) {
        self.sort_order = self.sort_order.toggle();
        self.selected = 0;
    }

    pub fn selected_daemon(&self) -> Option<&Daemon> {
        let filtered = self.filtered_daemons();
        filtered.get(self.selected).copied()
    }

    pub fn select_next(&mut self) {
        let count = self.filtered_daemons().len();
        if count > 0 {
            self.selected = (self.selected + 1) % count;
        }
    }

    pub fn select_prev(&mut self) {
        let count = self.filtered_daemons().len();
        if count > 0 {
            self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
        }
    }

    // Log follow mode
    pub fn toggle_log_follow(&mut self) {
        self.log_follow = !self.log_follow;
        if self.log_follow && !self.log_content.is_empty() {
            // Jump to bottom when enabling follow
            self.log_scroll = self.log_content.len().saturating_sub(20);
        }
    }

    // Toggle logs expanded (hide/show charts)
    pub fn toggle_logs_expanded(&mut self) {
        self.logs_expanded = !self.logs_expanded;
    }

    // Multi-select methods
    pub fn toggle_select(&mut self) {
        if let Some(daemon) = self.selected_daemon() {
            let id = daemon.id.clone();
            if self.multi_select.contains(&id) {
                self.multi_select.remove(&id);
            } else {
                self.multi_select.insert(id);
            }
        }
    }

    pub fn select_all_visible(&mut self) {
        // Collect IDs first to avoid borrow conflict
        let ids: Vec<String> = self
            .filtered_daemons()
            .iter()
            .map(|d| d.id.clone())
            .collect();
        for id in ids {
            self.multi_select.insert(id);
        }
    }

    pub fn clear_selection(&mut self) {
        self.multi_select.clear();
    }

    pub fn is_selected(&self, daemon_id: &str) -> bool {
        self.multi_select.contains(daemon_id)
    }

    pub fn has_selection(&self) -> bool {
        !self.multi_select.is_empty()
    }

    pub fn selected_daemon_ids(&self) -> Vec<String> {
        self.multi_select.iter().cloned().collect()
    }

    pub fn set_message(&mut self, msg: impl Into<String>) {
        self.message = Some(msg.into());
        self.message_time = Some(Instant::now());
    }

    pub fn clear_stale_message(&mut self) {
        if let Some(time) = self.message_time
            && time.elapsed().as_secs() >= 3
        {
            self.message = None;
            self.message_time = None;
        }
    }

    pub fn get_stats(&self, pid: u32) -> Option<&ProcessStats> {
        self.process_stats.get(&pid)
    }

    fn refresh_process_stats(&mut self) {
        PROCS.refresh_processes();
        self.process_stats.clear();
        for daemon in &self.daemons {
            if let Some(pid) = daemon.pid
                && let Some(stats) = PROCS.get_stats(pid)
            {
                self.process_stats.insert(pid, stats);
                // Record history for this daemon
                let history = self.stats_history.entry(daemon.id.clone()).or_default();
                history.push(StatsSnapshot::from(&stats));
            }
        }
    }

    /// Get stats history for a daemon
    pub fn get_stats_history(&self, daemon_id: &str) -> Option<&StatsHistory> {
        self.stats_history.get(daemon_id)
    }

    pub async fn refresh(&mut self, client: &Arc<IpcClient>) -> Result<()> {
        self.daemons = client.active_daemons().await?;
        // Filter out the pitchfork supervisor from the list (like web UI does)
        self.daemons.retain(|d| d.id != "pitchfork");
        self.disabled = client.get_disabled_daemons().await?;

        // Load config daemons and add placeholder entries for ones not currently active
        self.refresh_config_daemons();

        // Refresh process stats (CPU, memory, uptime)
        self.refresh_process_stats();

        // Clear stale messages
        self.clear_stale_message();

        // Keep selection in bounds
        let total_count = self.total_daemon_count();
        if total_count > 0 && self.selected >= total_count {
            self.selected = total_count - 1;
        }

        // Refresh logs if viewing
        if self.view == View::Logs
            && let Some(id) = self.log_daemon_id.clone()
        {
            self.load_logs(&id);
        }

        Ok(())
    }

    fn refresh_config_daemons(&mut self) {
        use crate::daemon_status::DaemonStatus;

        let config = PitchforkToml::all_merged();
        let active_ids: HashSet<String> = self.daemons.iter().map(|d| d.id.clone()).collect();

        // Find daemons in config that aren't currently active
        self.config_daemon_ids.clear();
        for daemon_id in config.daemons.keys() {
            if !active_ids.contains(daemon_id) && daemon_id != "pitchfork" {
                self.config_daemon_ids.insert(daemon_id.clone());

                // Add a placeholder daemon entry if show_available is enabled
                if self.show_available {
                    let placeholder = Daemon {
                        id: daemon_id.clone(),
                        title: None,
                        pid: None,
                        shell_pid: None,
                        status: DaemonStatus::Stopped,
                        dir: None,
                        cmd: None,
                        autostop: false,
                        cron_schedule: None,
                        cron_retrigger: None,
                        last_cron_triggered: None,
                        last_exit_success: None,
                        retry: 0,
                        retry_count: 0,
                        ready_delay: None,
                        ready_output: None,
                        ready_http: None,
                        ready_port: None,
                        ready_cmd: None,
                        depends: vec![],
                    };
                    self.daemons.push(placeholder);
                }
            }
        }
    }

    /// Check if a daemon is from config only (not currently active)
    pub fn is_config_only(&self, daemon_id: &str) -> bool {
        self.config_daemon_ids.contains(daemon_id)
    }

    /// Toggle showing available daemons from config
    pub fn toggle_show_available(&mut self) {
        self.show_available = !self.show_available;
    }

    /// Get total daemon count (for selection bounds)
    fn total_daemon_count(&self) -> usize {
        self.filtered_daemons().len()
    }

    pub fn scroll_logs_down(&mut self) {
        if self.log_content.len() > 20 {
            let max_scroll = self.log_content.len().saturating_sub(20);
            self.log_scroll = (self.log_scroll + 1).min(max_scroll);
        }
    }

    pub fn scroll_logs_up(&mut self) {
        self.log_scroll = self.log_scroll.saturating_sub(1);
    }

    /// Scroll down by half page (Ctrl+D)
    pub fn scroll_logs_page_down(&mut self, visible_lines: usize) {
        let half_page = visible_lines / 2;
        if self.log_content.len() > visible_lines {
            let max_scroll = self.log_content.len().saturating_sub(visible_lines);
            self.log_scroll = (self.log_scroll + half_page).min(max_scroll);
        }
    }

    /// Scroll up by half page (Ctrl+U)
    pub fn scroll_logs_page_up(&mut self, visible_lines: usize) {
        let half_page = visible_lines / 2;
        self.log_scroll = self.log_scroll.saturating_sub(half_page);
    }

    // Log search
    pub fn start_log_search(&mut self) {
        self.log_search_active = true;
        self.log_search_query.clear();
        self.log_search_matches.clear();
        self.log_search_current = 0;
    }

    pub fn end_log_search(&mut self) {
        self.log_search_active = false;
    }

    pub fn clear_log_search(&mut self) {
        self.log_search_query.clear();
        self.log_search_active = false;
        self.log_search_matches.clear();
        self.log_search_current = 0;
    }

    pub fn log_search_push(&mut self, c: char) {
        self.log_search_query.push(c);
        self.update_log_search_matches();
    }

    pub fn log_search_pop(&mut self) {
        self.log_search_query.pop();
        self.update_log_search_matches();
    }

    fn update_log_search_matches(&mut self) {
        self.log_search_matches.clear();
        if !self.log_search_query.is_empty() {
            let query = self.log_search_query.to_lowercase();
            for (i, line) in self.log_content.iter().enumerate() {
                if line.to_lowercase().contains(&query) {
                    self.log_search_matches.push(i);
                }
            }
            // Jump to first match if any
            if !self.log_search_matches.is_empty() {
                self.log_search_current = 0;
                self.jump_to_log_match();
            }
        }
    }

    pub fn log_search_next(&mut self) {
        if !self.log_search_matches.is_empty() {
            self.log_search_current = (self.log_search_current + 1) % self.log_search_matches.len();
            self.jump_to_log_match();
        }
    }

    pub fn log_search_prev(&mut self) {
        if !self.log_search_matches.is_empty() {
            self.log_search_current = self
                .log_search_current
                .checked_sub(1)
                .unwrap_or(self.log_search_matches.len() - 1);
            self.jump_to_log_match();
        }
    }

    fn jump_to_log_match(&mut self) {
        if let Some(&line_idx) = self.log_search_matches.get(self.log_search_current) {
            // Scroll so the match is visible (center it if possible)
            let half_page = 10; // Assume ~20 visible lines
            self.log_scroll = line_idx.saturating_sub(half_page);
            self.log_follow = false;
        }
    }

    // Details view
    pub fn show_details(&mut self, daemon_id: &str) {
        self.details_daemon_id = Some(daemon_id.to_string());
        self.prev_view = self.view;
        self.view = View::Details;
    }

    pub fn hide_details(&mut self) {
        self.details_daemon_id = None;
        self.view = View::Dashboard;
    }

    /// View daemon details (charts + logs)
    pub fn view_daemon_details(&mut self, daemon_id: &str) {
        self.log_daemon_id = Some(daemon_id.to_string());
        self.logs_expanded = false; // Start with charts visible
        self.load_logs(daemon_id);
        self.view = View::Logs; // Logs view is now the full daemon details view
    }

    fn load_logs(&mut self, daemon_id: &str) {
        let log_path = Self::log_path(daemon_id);
        let prev_len = self.log_content.len();

        self.log_content = if log_path.exists() {
            fs::read_to_string(&log_path)
                .unwrap_or_default()
                .lines()
                .map(String::from)
                .collect()
        } else {
            vec!["No logs available".to_string()]
        };

        // Auto-scroll to bottom when in follow mode
        if self.log_follow {
            if self.log_content.len() > 20 {
                self.log_scroll = self.log_content.len().saturating_sub(20);
            } else {
                self.log_scroll = 0;
            }
        } else if prev_len == 0 {
            // First load - start at bottom
            if self.log_content.len() > 20 {
                self.log_scroll = self.log_content.len().saturating_sub(20);
            }
        }
        // If not following and not first load, keep scroll position
    }

    fn log_path(daemon_id: &str) -> PathBuf {
        PITCHFORK_LOGS_DIR
            .join(daemon_id)
            .join(format!("{daemon_id}.log"))
    }

    pub fn show_help(&mut self) {
        self.view = View::Help;
    }

    pub fn back_to_dashboard(&mut self) {
        self.view = View::Dashboard;
        self.log_daemon_id = None;
        self.log_content.clear();
        self.log_scroll = 0;
    }

    /// Returns (total, running, stopped, errored, available)
    pub fn stats(&self) -> (usize, usize, usize, usize, usize) {
        let available = self.config_daemon_ids.len();
        let total = self.daemons.len();
        let running = self
            .daemons
            .iter()
            .filter(|d| d.status.is_running())
            .count();
        // Don't count config-only daemons as stopped
        let stopped = self
            .daemons
            .iter()
            .filter(|d| d.status.is_stopped() && !self.config_daemon_ids.contains(&d.id))
            .count();
        let errored = self
            .daemons
            .iter()
            .filter(|d| d.status.is_errored() || d.status.is_failed())
            .count();
        (total, running, stopped, errored, available)
    }

    pub fn is_disabled(&self, daemon_id: &str) -> bool {
        self.disabled.contains(&daemon_id.to_string())
    }

    // Config editor methods

    /// Get list of available config file paths
    pub fn get_config_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = PitchforkToml::list_paths()
            .into_iter()
            .filter(|p| p.exists())
            .collect();

        // Add option to create in current directory if not present
        let cwd_config = crate::env::CWD.join("pitchfork.toml");
        if !files.contains(&cwd_config) {
            files.push(cwd_config);
        }

        files
    }

    /// Open file selector for creating a new daemon
    pub fn open_file_selector(&mut self) {
        let files = self.get_config_files();
        self.file_selector = Some(ConfigFileSelector { files, selected: 0 });
        self.view = View::ConfigFileSelect;
    }

    /// Open editor for a new daemon with the selected config file
    pub fn open_editor_create(&mut self, config_path: PathBuf) {
        self.editor_state = Some(EditorState::new_create(config_path));
        self.file_selector = None;
        self.view = View::ConfigEditor;
    }

    /// Open editor for an existing daemon
    pub fn open_editor_edit(&mut self, daemon_id: &str) {
        let config = PitchforkToml::all_merged();
        if let Some(daemon_config) = config.daemons.get(daemon_id) {
            let config_path = daemon_config
                .path
                .clone()
                .unwrap_or_else(|| crate::env::CWD.join("pitchfork.toml"));
            self.editor_state = Some(EditorState::new_edit(
                daemon_id.to_string(),
                daemon_config,
                config_path,
            ));
            self.view = View::ConfigEditor;
        } else {
            self.set_message(format!("Daemon '{daemon_id}' not found in config"));
        }
    }

    /// Close the editor and return to dashboard
    pub fn close_editor(&mut self) {
        self.editor_state = None;
        self.file_selector = None;
        self.view = View::Dashboard;
    }

    /// Save the current editor state to config file.
    /// Returns Ok(true) if saved successfully, Ok(false) if validation/duplicate error (don't close editor).
    pub fn save_editor_config(&mut self) -> Result<bool> {
        let editor = self
            .editor_state
            .as_mut()
            .ok_or_else(|| miette::miette!("No editor state"))?;

        // Validate
        if !editor.validate() {
            self.set_message("Please fix validation errors before saving");
            return Ok(false);
        }

        // Build daemon config
        let daemon_config = editor.to_daemon_config();

        // Read existing config (or create new)
        let mut config = PitchforkToml::read(&editor.config_path)?;

        // Check for duplicate daemon ID
        let is_duplicate = match &editor.mode {
            EditMode::Create => config.daemons.contains_key(&editor.daemon_id),
            EditMode::Edit { original_id } => {
                // Only a duplicate if ID changed AND new ID already exists
                original_id != &editor.daemon_id && config.daemons.contains_key(&editor.daemon_id)
            }
        };

        if is_duplicate {
            let daemon_id = editor.daemon_id.clone();
            self.set_message(format!("A daemon named '{daemon_id}' already exists"));
            return Ok(false);
        }

        // Handle rename case
        if let EditMode::Edit { original_id } = &editor.mode
            && original_id != &editor.daemon_id
        {
            config.daemons.shift_remove(original_id);
        }

        // Insert/update daemon
        config
            .daemons
            .insert(editor.daemon_id.clone(), daemon_config);

        // Write back
        config.write()?;

        editor.unsaved_changes = false;
        let daemon_id = editor.daemon_id.clone();
        self.set_message(format!("Saved daemon '{daemon_id}'"));

        Ok(true)
    }

    /// Delete a daemon from the config file. Returns Ok(true) if deleted, Ok(false) if not found.
    pub fn delete_daemon_from_config(
        &mut self,
        id: &str,
        config_path: &std::path::Path,
    ) -> Result<bool> {
        let mut config = PitchforkToml::read(config_path)?;

        if config.daemons.shift_remove(id).is_some() {
            config.write()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
