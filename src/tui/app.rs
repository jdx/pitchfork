use crate::daemon::{Daemon, RunOptions};
use crate::env::PITCHFORK_LOGS_DIR;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use crate::procs::{ProcessStats, PROCS};
use crate::Result;
use miette::bail;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Dashboard,
    Logs,
    Help,
    Confirm,
    Loading,
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    Stop(String),
    Restart(String),
    Disable(String),
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
    pub pending_action: Option<PendingAction>,
    pub loading_text: Option<String>,
    pub search_query: String,
    pub search_active: bool,
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
            pending_action: None,
            loading_text: None,
            search_query: String::new(),
            search_active: false,
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
        if self.search_query.is_empty() {
            self.daemons.iter().collect()
        } else {
            let query = self.search_query.to_lowercase();
            self.daemons
                .iter()
                .filter(|d| d.id.to_lowercase().contains(&query))
                .collect()
        }
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
                }
            }
        }
    }

    pub async fn refresh(&mut self, client: &IpcClient) -> Result<()> {
        self.daemons = client.active_daemons().await?;
        // Filter out the pitchfork supervisor from the list (like web UI does)
        self.daemons.retain(|d| d.id != "pitchfork");
        self.disabled = client.get_disabled_daemons().await?;

        // Refresh process stats (CPU, memory, uptime)
        self.refresh_process_stats();

        // Clear stale messages
        self.clear_stale_message();

        // Keep selection in bounds
        if !self.daemons.is_empty() && self.selected >= self.daemons.len() {
            self.selected = self.daemons.len() - 1;
        }

        // Refresh logs if viewing
        if self.view == View::Logs {
            if let Some(id) = self.log_daemon_id.clone() {
                self.load_logs(&id);
            }
        }

        Ok(())
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

    pub fn view_logs(&mut self, daemon_id: &str) {
        self.log_daemon_id = Some(daemon_id.to_string());
        self.load_logs(daemon_id);
        self.view = View::Logs;
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

    pub fn stats(&self) -> (usize, usize, usize, usize) {
        let total = self.daemons.len();
        let running = self
            .daemons
            .iter()
            .filter(|d| d.status.is_running())
            .count();
        let stopped = self
            .daemons
            .iter()
            .filter(|d| d.status.is_stopped())
            .count();
        let errored = self
            .daemons
            .iter()
            .filter(|d| d.status.is_errored() || d.status.is_failed())
            .count();
        (total, running, stopped, errored)
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
            wait_ready: false,
        };

        let _ = client.run(opts).await;
        self.set_message(format!("Started {}", daemon_id));
        Ok(())
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
