use crate::state_file::StateFile;
use crate::ui::table::print_table;
use crate::Result;
use comfy_table::{Cell, ContentArrangement, Table};

/// List all daemons
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "ls", verbatim_doc_comment)]
pub struct List {
    /// Show header
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
                Cell::new(
                    sf.disabled
                        .contains(id)
                        .then_some("disabled")
                        .unwrap_or_default(),
                ),
            ]);
        }

        print_table(table)
    }
}
