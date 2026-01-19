use crate::daemon::{Daemon, RunOptions};
use crate::env::PITCHFORK_LOGS_DIR;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::{ProcessStats, PROCS};
use crate::Result;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use miette::bail;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

/// Maximum number of stat samples to keep for each daemon (e.g., 60 samples at 2s intervals = 2 minutes)
const MAX_STAT_HISTORY: usize = 60;

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
        }
    }

    pub fn confirm_action(&mut self, action: PendingAction) {
        self.pending_action = Some(action);
        self.view = View::Confirm;
    }

    pub fn cancel_confirm(&mut self) {
        self.pending_action = None;
        self.view = View::Dashboard;
    }

    pub fn take_pending_action(&mut self) -> Option<PendingAction> {
        self.view = View::Dashboard;
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
                        crate::daemon_status::DaemonStatus::Errored(_) => 4,
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
        if let Some(time) = self.message_time {
            if time.elapsed().as_secs() >= 3 {
                self.message = None;
                self.message_time = None;
            }
        }
    }

    pub fn get_stats(&self, pid: u32) -> Option<&ProcessStats> {
        self.process_stats.get(&pid)
    }

    fn refresh_process_stats(&mut self) {
        PROCS.refresh_processes();
        self.process_stats.clear();
        for daemon in &self.daemons {
            if let Some(pid) = daemon.pid {
                if let Some(stats) = PROCS.get_stats(pid) {
                    self.process_stats.insert(pid, stats);
                    // Record history for this daemon
                    let history = self.stats_history.entry(daemon.id.clone()).or_default();
                    history.push(StatsSnapshot::from(&stats));
                }
            }
        }
    }

    /// Get stats history for a daemon
    pub fn get_stats_history(&self, daemon_id: &str) -> Option<&StatsHistory> {
        self.stats_history.get(daemon_id)
    }

    pub async fn refresh(&mut self, client: &IpcClient) -> Result<()> {
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
        if self.view == View::Logs {
            if let Some(id) = self.log_daemon_id.clone() {
                self.load_logs(&id);
            }
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
                        autostop: false,
                        cron_schedule: None,
                        cron_retrigger: None,
                        last_exit_success: None,
                        retry: 0,
                        retry_count: 0,
                        ready_delay: None,
                        ready_output: None,
                        ready_http: None,
                        ready_port: None,
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

    pub async fn start_daemon(&mut self, client: &IpcClient, daemon_id: &str) -> Result<()> {
        // Find daemon config from pitchfork.toml files
        let config = PitchforkToml::all_merged();
        let daemon_config = config
            .daemons
            .get(daemon_id)
            .ok_or_else(|| miette::miette!("Daemon '{}' not found in config", daemon_id))?;

        let cmd = shell_words::split(&daemon_config.run)
            .map_err(|e| miette::miette!("Failed to parse command: {}", e))?;

        if cmd.is_empty() {
            bail!("Daemon '{}' has empty run command", daemon_id);
        }

        let (cron_schedule, cron_retrigger) = daemon_config
            .cron
            .as_ref()
            .map(|c| (Some(c.schedule.clone()), Some(c.retrigger)))
            .unwrap_or((None, None));

        let opts = RunOptions {
            id: daemon_id.to_string(),
            cmd,
            force: false,
            shell_pid: None,
            dir: std::env::current_dir().unwrap_or_default(),
            autostop: false,
            cron_schedule,
            cron_retrigger,
            retry: daemon_config.retry,
            retry_count: 0,
            ready_delay: daemon_config.ready_delay,
            ready_output: daemon_config.ready_output.clone(),
            ready_http: daemon_config.ready_http.clone(),
            ready_port: daemon_config.ready_port,
            wait_ready: false,
        };

        client.run(opts).await?;
        self.set_message(format!("Started {}", daemon_id));
        Ok(())
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
