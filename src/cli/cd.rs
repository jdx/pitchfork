use crate::Result;

#[derive(Debug, clap::Args)]
#[clap(hide = true, verbatim_doc_comment)]
pub struct Cd {
    #[clap(long)]
    shell_pid: u32,
}

impl Cd {
    pub async fn run(&self) -> Result<()> {
        dbg!(self);
        Ok(())
    }
}
