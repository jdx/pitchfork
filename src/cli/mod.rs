use crate::Result;
use clap::Parser;
use std::ffi::OsString;

mod activate;
mod api_schema;
mod boot;
mod cd;
mod clean;
mod completion;
mod daemons;
mod disable;
mod enable;
mod json_output;
mod list;
pub mod logs;
mod mcp;
mod proxy;
mod restart;
mod run;
mod schema;
mod settings;
mod sponsors;
mod start;
mod status;
mod stop;
mod supervisor;
mod tui;
mod usage;
mod wait;

#[derive(Debug, clap::Parser)]
#[clap(name = "pitchfork", version = env!("CARGO_PKG_VERSION"), about = env!("CARGO_PKG_DESCRIPTION"))]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    Activate(activate::Activate),
    ApiSchema(api_schema::ApiSchema),
    Boot(boot::Boot),
    Cd(cd::Cd),
    Clean(clean::Clean),
    Daemons(daemons::Daemons),
    Completion(completion::Completion),
    Disable(disable::Disable),
    Enable(enable::Enable),
    List(list::List),
    Logs(logs::Logs),
    Mcp(mcp::Mcp),
    Proxy(proxy::Proxy),
    Restart(restart::Restart),
    Run(run::Run),
    Schema(schema::Schema),
    Settings(settings::Settings),
    Sponsors(sponsors::Sponsors),
    Start(start::Start),
    Status(status::Status),
    Stop(stop::Stop),
    Supervisor(supervisor::Supervisor),
    Tui(tui::Tui),
    Usage(usage::Usage),
    Wait(wait::Wait),
    #[clap(external_subcommand)]
    Fallback(Vec<OsString>),
}

/// Parses tokens captured by the implicit subcommand fallback as a
/// `pitchfork start` invocation, so usage/help/error output reflects that.
#[derive(Debug, clap::Parser)]
#[clap(
    name = "pitchfork",
    bin_name = "pitchfork start",
    version = env!("CARGO_PKG_VERSION"),
    long_about = start::LONG_ABOUT
)]
struct StartFallback {
    #[clap(flatten)]
    start: start::Start,
}

pub async fn run() -> Result<()> {
    let args = Cli::parse();
    let program = std::env::args_os()
        .next()
        .unwrap_or_else(|| "pitchfork".into());
    match args.command {
        Commands::Activate(activate) => activate.run().await,
        Commands::Boot(boot) => boot.run().await,
        Commands::Cd(cd) => cd.run().await,
        Commands::Clean(clean) => clean.run().await,
        Commands::Daemons(daemons) => daemons.run().await,
        Commands::Completion(completion) => completion.run().await,
        Commands::Disable(disable) => disable.run().await,
        Commands::Enable(enable) => enable.run().await,
        Commands::List(list) => list.run().await,
        Commands::Logs(logs) => logs.run().await,
        Commands::Mcp(mcp) => mcp.run().await,
        Commands::Proxy(proxy) => proxy.run().await,
        Commands::Restart(restart) => restart.run().await,
        Commands::Run(run) => run.run().await,
        Commands::ApiSchema(api_schema) => api_schema.run().await,
        Commands::Schema(schema) => schema.run().await,
        Commands::Settings(settings) => settings.run().await,
        Commands::Sponsors(_) => sponsors::Sponsors::run().await,
        Commands::Start(start) => start.run().await,
        Commands::Status(status) => status.run().await,
        Commands::Stop(stop) => stop.run().await,
        Commands::Supervisor(supervisor) => supervisor.run().await,
        Commands::Tui(tui) => tui.run().await,
        Commands::Usage(usage) => usage.run().await,
        Commands::Wait(wait) => wait.run().await,
        Commands::Fallback(tokens) => {
            let mut argv = vec![program];
            argv.extend(tokens);
            StartFallback::parse_from(argv).start.run().await
        }
    }
}

/// Drain and display any pending notifications from the supervisor.
///
/// Notifications are queued by the supervisor for events that happen
/// asynchronously (e.g. proxy bind failure) and would otherwise be invisible
/// to CLI users.  Call this at the end of user-facing commands that connect
/// to the supervisor via IPC.
pub(crate) async fn drain_notifications(ipc: &crate::ipc::client::IpcClient) {
    use log::LevelFilter;
    if let Ok(notifications) = ipc.get_notifications().await {
        for (level, msg) in notifications {
            match level {
                LevelFilter::Trace => trace!("{msg}"),
                LevelFilter::Debug => debug!("{msg}"),
                LevelFilter::Info => info!("{msg}"),
                LevelFilter::Warn => warn!("{msg}"),
                LevelFilter::Error => error!("{msg}"),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn unknown_subcommand_captured_as_fallback() {
        let args = Cli::parse_from(["pitchfork", "mydaemon", "--force"]);
        match args.command {
            Commands::Fallback(tokens) => {
                assert_eq!(
                    tokens,
                    vec![OsString::from("mydaemon"), OsString::from("--force")]
                );
            }
            _ => panic!("expected Fallback variant, got {:?}", args.command),
        }
    }

    #[test]
    fn unknown_subcommand_captures_multiple_args() {
        let args = Cli::parse_from(["pitchfork", "api", "worker", "--force"]);
        match args.command {
            Commands::Fallback(tokens) => {
                assert_eq!(
                    tokens,
                    vec![
                        OsString::from("api"),
                        OsString::from("worker"),
                        OsString::from("--force")
                    ]
                );
            }
            _ => panic!("expected Fallback variant, got {:?}", args.command),
        }
    }

    #[test]
    fn known_start_parses_as_start() {
        let args = Cli::parse_from(["pitchfork", "start", "mydaemon"]);
        match args.command {
            Commands::Start(_) => {}
            _ => panic!("expected Start variant, got {:?}", args.command),
        }
    }

    #[test]
    fn start_alias_still_works() {
        let args = Cli::parse_from(["pitchfork", "s", "mydaemon"]);
        match args.command {
            Commands::Start(_) => {}
            _ => panic!("expected Start variant, got {:?}", args.command),
        }
    }

    #[test]
    fn fallback_reparse_as_start() {
        StartFallback::try_parse_from(["pitchfork", "mydaemon", "--force"])
            .expect("should re-parse captured tokens as Start");
    }

    #[test]
    fn fallback_reparse_rejects_invalid_start_flag() {
        let result = StartFallback::try_parse_from(["pitchfork", "mydaemon", "--not-a-start-flag"]);
        assert!(
            result.is_err(),
            "expected re-parse to fail for invalid Start flag"
        );
    }

    #[test]
    fn fallback_invalid_start_usage_renders_pitchfork_start() {
        let err = StartFallback::try_parse_from(["pitchfork", "mydaemon", "--not-a-start-flag"])
            .unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("Usage: pitchfork start"),
            "expected usage to contain 'pitchfork start', got: {rendered}"
        );
    }

    #[test]
    fn fallback_help_shows_start_long_about() {
        let err = StartFallback::try_parse_from(["pitchfork", "mydaemon", "--help"]).unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("Examples:"),
            "expected help to include Start long_about examples, got: {rendered}"
        );
        assert!(
            rendered.contains("pitchfork start api"),
            "expected help to reference `pitchfork start api`, got: {rendered}"
        );
    }
}
