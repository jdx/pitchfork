#[macro_use]
extern crate log;

mod boot_manager;
mod cli;
mod config_types;
#[cfg(windows)]
mod console_ctrl;
mod daemon;
mod daemon_id;
mod daemon_list;
mod daemon_status;
mod deprecated;
mod deps;
mod env;
mod error;
mod extra_configs;
mod inherited_fds;
mod ipc;
mod log_jq;
mod log_parse;
mod log_store;
mod logger;
mod pitchfork_toml;
mod procs;
mod program_status;
mod proxy;
mod settings;
mod shell;
mod state_file;
mod supervisor;
mod template;
mod tui;
mod ui;
mod watch_files;
mod web;
#[cfg(windows)]
mod win_job;

pub use miette::Result;
#[cfg(unix)]
use tokio::signal;
#[cfg(unix)]
use tokio::signal::unix::SignalKind;

fn main() -> Result<()> {
    // The shell bridge inherits daemon output; loading settings would leak
    // CLI warnings into readiness checks and hooks.
    let shell_bridge =
        std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("shell-exec"));
    if !shell_bridge {
        initialize()?;
    }
    let invocation = cli::Invocation::parse();
    let runtime = invocation.runtime()?;
    runtime.block_on(async {
        #[cfg(unix)]
        if !shell_bridge {
            handle_epipe();
        }
        invocation.run().await
    })
}

/// Initialize synchronous CLI state before parsing, so help and parse errors
/// keep the configured logging without constructing a worker pool.
fn initialize() -> Result<()> {
    // Install ring as the default rustls crypto provider.
    // Required because reqwest is built with `rustls-tls-*-no-provider`, which
    // avoids pulling in aws-lc-sys but requires the caller to install a provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Reject an unusable `supervisor run --invoking-user` before logging and
    // settings resolve paths: they would fall back to root's home and touch
    // its configuration and state.
    #[cfg(unix)]
    if let Err(e) = &*env::INVOKING_USER {
        return Err(miette::miette!("{e}"));
    }
    logger::init();
    // Re-apply log levels now that settings (env + config files) are loaded.
    // logger::init() only sees env vars; this picks up pitchfork.toml values.
    logger::apply_settings();
    init_progress_output();
    Ok(())
}

/// Switches progress output to plain lines when stderr is not a terminal.
///
/// clx defaults to its animated UI, which redraws in place several times a
/// second. Without a terminal to interpret the cursor movement, every redraw
/// lands in the output as new lines. Text mode instead prints one line per
/// status change.
fn init_progress_output() {
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() {
        clx::progress::set_output(clx::progress::ProgressOutput::Text);
    }
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
