#[macro_use]
extern crate log;

mod boot_manager;
mod cli;
mod daemon;
mod daemon_id;
mod daemon_list;
mod daemon_status;
mod deps;
mod env;
mod error;
mod ipc;
mod logger;
mod pitchfork_toml;
mod procs;
mod settings;
mod shell;
mod state_file;
mod supervisor;
mod tui;
mod ui;
mod watch_files;
mod web;

pub use miette::Result;
use tokio::signal;
#[cfg(unix)]
use tokio::signal::unix::SignalKind;

#[tokio::main]
async fn main() -> Result<()> {
    logger::init();
    #[cfg(unix)]
    handle_epipe();
    cli::run().await
}

#[cfg(unix)]
fn handle_epipe() {
    match signal::unix::signal(SignalKind::pipe()) {
        Ok(mut pipe_stream) => {
            tokio::spawn(async move {
                pipe_stream.recv().await;
                debug!("received SIGPIPE");
            });
        }
        Err(e) => {
            warn!("Could not set up SIGPIPE handler: {e}");
        }
    }
}
