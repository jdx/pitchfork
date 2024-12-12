#[macro_use]
extern crate log;

mod cli;
mod daemon;
mod daemon_status;
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
use tokio::signal;
use tokio::signal::unix::SignalKind;

#[tokio::main]
async fn main() -> Result<()> {
    logger::init();
    handle_epipe();
    cli::run().await
}

fn handle_epipe() {
    let mut pipe_stream = signal::unix::signal(SignalKind::pipe()).unwrap();
    tokio::spawn(async move {
        pipe_stream.recv().await;
        debug!("received SIGPIPE");
    });
}
