use crate::state_file::StateFile;
use crate::{env, Result};
use cli_table::format::Separator;
use cli_table::{
    format::{Border, Justify},
    print_stdout, Cell, Table,
};

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
        let sf = StateFile::read(&*env::PITCHFORK_STATE_FILE)?;
        let mut table = vec![];
        for (name, daemon) in sf.daemons.iter() {
            table.push(vec![
                name.cell(),
                daemon.pid.cell(),
                daemon.status.style().cell().justify(Justify::Right),
            ]);
        }
        let mut table = table
            .table()
            .separator(Separator::builder().build())
            .border(Border::builder().build());
        if !self.hide_header || !console::user_attended() {
            table = table.title(vec!["Name".cell(), "PID".cell(), "Status".cell()])
        }
        assert!(print_stdout(table).is_ok());
        Ok(())
    }
}
