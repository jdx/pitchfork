#[macro_use]
extern crate log;

mod cli;
mod daemon;
mod env;
mod ipc;
mod logger;
mod pitchfork_toml;
mod procs;
mod state_file;
mod supervisor;
mod ui;

pub use eyre::Result;

fn main() -> Result<()> {
    logger::init();
    color_eyre::install()?;
    cli::run()
}
