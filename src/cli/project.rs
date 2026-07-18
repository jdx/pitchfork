use crate::daemon_id::DaemonId;
use crate::ipc::client::IpcClient;
use crate::pitchfork_toml::{PitchforkToml, PitchforkTomlAuto};
use crate::ui::table::print_table;
use crate::{Result, env};
use miette::{IntoDiagnostic, ensure};
use std::collections::HashSet;
use std::path::PathBuf;

use comfy_table::{Cell, Color, ContentArrangement, Table};

/// Project session management for IDE and workspace integrations.
#[derive(Debug, clap::Args)]
#[clap(verbatim_doc_comment)]
pub struct Project {
    #[clap(subcommand)]
    command: ProjectCommands,
}

#[derive(Debug, clap::Subcommand)]
enum ProjectCommands {
    Enter(Enter),
    Leave(Leave),
    List(List),
}

/// Enter (or replace) a project session tied to a host process.
///
/// The host PID is required and is used by the supervisor to revoke the
/// session automatically when the process dies (with a title match to guard
/// against PID reuse).
///
/// On Windows, automatic revocation when the host process exits is not
/// available (Git Bash PIDs are invisible to the process table); sessions
/// must be ended with an explicit `project leave`.
#[derive(Debug, clap::Args)]
pub struct Enter {
    /// Host process PID that owns the session. Required.
    #[clap(long)]
    pid: u32,
    /// Project directory to associate with the session. Defaults to the
    /// current working directory and is canonicalized before tracking.
    #[clap(long)]
    directory: Option<PathBuf>,
}

/// Leave a project session and evaluate its directory for autostop.
#[derive(Debug, clap::Args)]
pub struct Leave {
    /// Host process PID that owns the session.
    #[clap(long)]
    pid: u32,
    /// Project directory the session was entered with. Defaults to the
    /// current working directory and is canonicalized before lookup. Must
    /// match the directory used at enter time.
    #[clap(long)]
    directory: Option<PathBuf>,
}

/// List tracked project sessions.
#[derive(Debug, clap::Args)]
#[clap(
    verbatim_doc_comment,
    long_about = "\
List tracked project sessions

Displays a table of active project sessions with their host PID, directory,
liveness status, and recorded process title.

Example:
  pitchfork project list
  pitchfork project list --json"
)]
pub struct List {
    /// Output in JSON format
    #[clap(long)]
    json: bool,
}

impl Project {
    pub async fn run(&self) -> Result<()> {
        match &self.command {
            ProjectCommands::Enter(enter) => enter.run().await,
            ProjectCommands::Leave(leave) => leave.run().await,
            ProjectCommands::List(list) => list.run().await,
        }
    }
}

/// Resolve a directory argument (or the CWD) to a canonical absolute path.
fn resolve_directory(directory: &Option<PathBuf>) -> Result<PathBuf> {
    match directory {
        Some(d) => d.canonicalize().into_diagnostic(),
        None => env::CWD.canonicalize().into_diagnostic(),
    }
}

impl Enter {
    pub async fn run(&self) -> Result<()> {
        let target_dir = resolve_directory(&self.directory)?;

        // Validate configuration before creating a session so invalid configs
        // do not leave a half-entered session behind.
        let pt = PitchforkToml::all_merged_from(&target_dir)?;

        let ipc = IpcClient::connect(true).await?;
        ipc.project_enter(self.pid, target_dir.clone()).await?;

        let to_start: Vec<DaemonId> = pt
            .daemons
            .into_iter()
            .filter(|(_, d)| d.auto.contains(&PitchforkTomlAuto::Start))
            .map(|(id, _)| id)
            .collect();

        if !to_start.is_empty() {
            let active_daemons: HashSet<DaemonId> = ipc
                .active_daemons()
                .await?
                .into_iter()
                .map(|d| d.id)
                .collect();
            let mut args = vec!["start".to_string()];
            for id in &to_start {
                if active_daemons.contains(id) {
                    continue;
                }
                args.push(id.qualified());
            }
            if args.len() > 1 {
                let status = tokio::process::Command::new(&*env::PITCHFORK_BIN)
                    .args(&args)
                    .current_dir(&target_dir)
                    .status()
                    .await
                    .into_diagnostic()?;
                ensure!(
                    status.success(),
                    "pitchfork start {} exited with {status}",
                    args[1..].join(" ")
                );
            }
        }

        super::drain_notifications(&ipc).await;
        Ok(())
    }
}

impl Leave {
    pub async fn run(&self) -> Result<()> {
        let target_dir = resolve_directory(&self.directory)?;
        let ipc = IpcClient::connect(true).await?;
        ipc.project_leave(self.pid, target_dir).await?;
        super::drain_notifications(&ipc).await;
        Ok(())
    }
}

impl List {
    pub async fn run(&self) -> Result<()> {
        let ipc = IpcClient::connect(true).await?;
        let mut sessions = ipc.get_project_sessions().await?;
        // Stable ordering: by PID, then directory.
        sessions.sort_by(|a, b| a.pid.cmp(&b.pid).then(a.directory.cmp(&b.directory)));

        if self.json {
            return crate::cli::json_output::print_json(&sessions);
        }

        let mut table = Table::new();
        table
            .load_preset(comfy_table::presets::NOTHING)
            .set_content_arrangement(ContentArrangement::Disabled);
        if console::user_attended() {
            table.set_header(vec!["PID", "DIRECTORY", "STATUS", "TITLE"]);
        }

        for s in &sessions {
            let pid = s.pid.to_string();
            let directory = s.directory.display().to_string();
            let (status_text, status_color) = if s.alive {
                ("alive".to_string(), Color::Green)
            } else {
                ("dead".to_string(), Color::Red)
            };
            // Prefer the recorded title for display; fall back to the current
            // title if no snapshot was taken (shouldn't normally happen since
            // enter always records one for a live PID).
            let title = s
                .liveness_title
                .clone()
                .or_else(|| s.current_title.clone())
                .unwrap_or_default();

            table.add_row(vec![
                Cell::new(&pid),
                Cell::new(&directory),
                Cell::new(&status_text).fg(status_color),
                Cell::new(&title),
            ]);
        }

        print_table(table)
    }
}
