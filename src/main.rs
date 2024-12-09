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
mod watch_files;

pub use miette::Result;

fn main() -> Result<()> {
    logger::init();
    cli::run()
}
