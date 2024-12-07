#[macro_use]
extern crate log;

mod cli;
mod daemon;
mod env;
mod logger;
mod state_file;
mod pitchfork_toml;
mod procs;
mod supervisor;
mod ui;
mod ipc;

pub use eyre::Result;

fn main() -> Result<()> {
    logger::init();
    color_eyre::install()?;
    cli::run()
}
