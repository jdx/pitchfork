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
mod async_watcher;

pub use eyre::Result;

fn main() -> Result<()> {
    logger::init();
    cli::run()
}
