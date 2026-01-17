use crate::state_file::StateFile;
use crate::ui::table::print_table;
use crate::Result;
use comfy_table::{Cell, ContentArrangement, Table};

/// List all daemons
#[derive(Debug, clap::Args)]
#[clap(
    visible_alias = "ls",
    verbatim_doc_comment,
    long_about = "\
List all daemons

Displays a table of all tracked daemons with their PIDs, status, and
whether they are disabled.

Example:
  pitchfork list
  pitchfork ls                    Alias for 'list'
  pitchfork list --hide-header    Output without column headers

Output:
  Name    PID    Status
  api     12345  running
  worker  12346  running
  db      -      stopped  disabled"
)]
pub struct List {
    /// Hide the table header row
    #[clap(long)]
    hide_header: bool,
}

impl List {
    pub async fn run(&self) -> Result<()> {
        let mut table = Table::new();
        table
            .load_preset(comfy_table::presets::NOTHING)
            .set_content_arrangement(ContentArrangement::Dynamic);
        if !self.hide_header && console::user_attended() {
            table.set_header(vec!["Name", "PID", "Status", ""]);
        }

        let sf = StateFile::get();
        for (id, daemon) in sf.daemons.iter() {
            table.add_row(vec![
                Cell::new(id),
                Cell::new(
                    daemon
                        .pid
                        .as_ref()
                        .map(|p| p.to_string())
                        .unwrap_or_default(),
                ),
                Cell::new(daemon.status.style()),
                Cell::new(if sf.disabled.contains(id) {
                    "disabled"
                } else {
                    Default::default()
                }),
            ]);
        }

        print_table(table)
    }
}
