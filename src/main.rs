mod cli;
mod env;
mod pid_file;
mod procs;
mod logger;
mod ui;
mod pitchfork_toml;
mod daemon;

pub use eyre::Result;

fn main() -> Result<()> {
    logger::init();
    cli::run()
}
