use crate::Result;
use crate::daemon::Daemon;
use crate::daemon_status::DaemonStatus;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::PitchforkToml;
use std::collections::HashSet;

/// Represents a daemon entry that can be either tracked (from state file) or available (from config only)
#[derive(Debug, Clone)]
pub struct DaemonListEntry {
    pub id: String,
    pub daemon: Daemon,
    pub is_disabled: bool,
    pub is_available: bool, // true if daemon is only in config, not in state
}

/// Get a unified list of all daemons from IPC client and config
///
/// This function merges daemons from the state file (including failed daemons) with daemons
/// defined in config files. Daemons that are only in config (not in state file) are marked
/// as "available".
///
/// This logic is shared across:
/// - `pitchfork list` command
/// - TUI daemon list
///
/// # Arguments
/// * `client` - IPC client to communicate with supervisor (used only for disabled list)
///
/// # Returns
/// A vector of daemon entries with their current status
pub async fn get_all_daemons(client: &IpcClient) -> Result<Vec<DaemonListEntry>> {
    let config = PitchforkToml::all_merged();

    // Read state file to get all daemons (including failed ones)
    let state_file = crate::state_file::StateFile::read(&*crate::env::PITCHFORK_STATE_FILE)?;
    let state_daemons: Vec<Daemon> = state_file.daemons.values().cloned().collect();

    let disabled_daemons = client.get_disabled_daemons().await?;
    let disabled_set: HashSet<String> = disabled_daemons.into_iter().collect();

    build_daemon_list(state_daemons, disabled_set, config)
}

/// Get a unified list of all daemons from supervisor directly (for Web UI)
///
/// This function is used by the Web UI which runs inside the supervisor process
/// and can access the supervisor directly without IPC.
///
/// # Arguments
/// * `supervisor` - Reference to the supervisor instance
///
/// # Returns
/// A vector of daemon entries with their current status
pub async fn get_all_daemons_direct(
    supervisor: &crate::supervisor::Supervisor,
) -> Result<Vec<DaemonListEntry>> {
    let config = PitchforkToml::all_merged();

    // Read all daemons from state file (including failed/stopped ones)
    // Note: Don't use supervisor.active_daemons() as it only returns daemons with PIDs
    let state_file = supervisor.state_file.lock().await;
    let state_daemons: Vec<Daemon> = state_file.daemons.values().cloned().collect();
    let disabled_set: HashSet<String> = state_file.disabled.clone().into_iter().collect();
    drop(state_file); // Release lock early

    build_daemon_list(state_daemons, disabled_set, config)
}

/// Internal helper to build the daemon list from state daemons and config
fn build_daemon_list(
    state_daemons: Vec<Daemon>,
    disabled_set: HashSet<String>,
    config: PitchforkToml,
) -> Result<Vec<DaemonListEntry>> {
    let mut entries = Vec::new();
    let mut seen_ids = HashSet::new();

    // First, add all daemons from state file
    for daemon in state_daemons {
        if daemon.id == "pitchfork" {
            continue; // Skip supervisor itself
        }

        seen_ids.insert(daemon.id.clone());
        entries.push(DaemonListEntry {
            id: daemon.id.clone(),
            is_disabled: disabled_set.contains(&daemon.id),
            is_available: false,
            daemon,
        });
    }

    // Then, add daemons from config that aren't in state file (available daemons)
    for daemon_id in config.daemons.keys() {
        if daemon_id == "pitchfork" || seen_ids.contains(daemon_id) {
            continue;
        }

        // Create a placeholder daemon for config-only entries
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
            original_port: Vec::new(),
            port: Vec::new(),
            auto_bump_port: false,
            port_bump_attempts: 10,
            depends: vec![],
            env: None,
            watch: vec![],
            watch_base_dir: None,
        };

        entries.push(DaemonListEntry {
            id: daemon_id.clone(),
            daemon: placeholder,
            is_disabled: disabled_set.contains(daemon_id),
            is_available: true,
        });
    }

    Ok(entries)
}
