use crate::Result;
use crate::daemon_list::get_all_daemons;
use crate::daemon_status::DaemonStatus;
use crate::ipc::client::IpcClient;
use crate::ui::table::print_table;
use comfy_table::{Cell, Color, ContentArrangement, Table};

/// List all daemons
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "ls",
    verbatim_doc_comment,
    long_about = "\
List all daemons

Displays a table of all tracked daemons with their PIDs, status,
whether they are disabled, and any error messages.

This command shows both:
- Active daemons (currently running or stopped)
- Available daemons (defined in config but not yet started)

Example:
  pitchfork list
  pitchfork ls                    Alias for 'list'
  pitchfork list --hide-header    Output without column headers

Output:
  Name    PID    Status     Error
  api     12345  running
  worker         available
  db             errored    exit code 127"
)]
pub struct List {
    /// Hide the table header row
    #[clap(long)]
    hide_header: bool,
}

impl List {
    pub async fn run(&self) -> Result<()> {
        let client = IpcClient::connect(true).await?;

        let mut table = Table::new();
        table
            .load_preset(comfy_table::presets::NOTHING)
            .set_content_arrangement(ContentArrangement::Dynamic);
        if !self.hide_header && console::user_attended() {
            table.set_header(vec!["Name", "PID", "Status", "", "Error"]);
        }

        let entries = get_all_daemons(&client).await?;

        for entry in entries {
            let status_text = if entry.is_available {
                "available".to_string()
            } else {
                entry.daemon.status.to_string()
            };

            let status_color = if entry.is_available {
                Color::Cyan
            } else {
                match entry.daemon.status {
                    DaemonStatus::Failed(_) => Color::Red,
                    DaemonStatus::Waiting => Color::Yellow,
                    DaemonStatus::Running => Color::Green,
                    DaemonStatus::Stopping => Color::Yellow,
                    DaemonStatus::Stopped => Color::DarkGrey,
                    DaemonStatus::Errored(_) => Color::Red,
                }
            };

            let disabled_marker = if entry.is_disabled { "disabled" } else { "" };

            let error_msg = entry.daemon.status.error_message().unwrap_or_default();

            let error_cell = if error_msg.is_empty() {
                Cell::new("")
            } else {
                Cell::new(&error_msg).fg(Color::Red)
            };

            table.add_row(vec![
                Cell::new(&entry.id),
                Cell::new(
                    entry
                        .daemon
                        .pid
                        .as_ref()
                        .map(|p| p.to_string())
                        .unwrap_or_default(),
                ),
                Cell::new(&status_text).fg(status_color),
                Cell::new(disabled_marker),
                error_cell,
            ]);
        }

        print_table(table)
    }
}
