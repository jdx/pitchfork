use crate::Result;
use miette::IntoDiagnostic as _;
use std::ffi::OsString;
use tokio::io::AsyncWriteExt as _;
use usage_rs::Cli;

mod activate;
mod api_schema;
mod boot;
mod cd;
mod clean;
mod command_effects;
mod completion;
mod daemons;
mod disable;
mod enable;
mod interactive;
pub mod json_output;
mod list;
pub mod log_sink;
pub mod logs;
mod mcp;
mod project;
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

#[derive(Debug, usage_rs::Cli)]
#[usage(
    name = "pitchfork",
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    completion,
    unknown_flags = "error",
    arg_required_else_help
)]
struct Cli {
    #[usage(subcommand)]
    command: Commands,
}

#[derive(Debug, usage_rs::Subcommands)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    Activate(activate::Activate),
    #[usage(hide)]
    ApiSchema(api_schema::ApiSchema),
    Boot(boot::Boot),
    #[usage(hide)]
    Cd(cd::Cd),
    #[usage(alias = "c")]
    Clean(clean::Clean),
    #[usage(alias = "daemon")]
    Daemons(daemons::Daemons),
    Completion(completion::Completion),
    #[usage(alias = "d")]
    Disable(disable::Disable),
    #[usage(alias = "e")]
    Enable(enable::Enable),
    #[usage(alias = "ls")]
    List(list::List),
    #[usage(hide)]
    LogSink(log_sink::LogSink),
    #[usage(alias = "l")]
    Logs(logs::Logs),
    Mcp(mcp::Mcp),
    Proxy(proxy::Proxy),
    Project(project::Project),
    Restart(restart::Restart),
    #[usage(alias = "r")]
    Run(run::Run),
    #[usage(hide)]
    Schema(schema::Schema),
    #[usage(alias = "setting")]
    Settings(settings::Settings),
    Sponsors(sponsors::Sponsors),
    #[usage(alias = "s")]
    Start(start::Start),
    #[usage(alias = "stat")]
    Status(status::Status),
    #[usage(alias = "kill")]
    Stop(stop::Stop),
    #[usage(alias = "sup")]
    Supervisor(supervisor::Supervisor),
    Tui(tui::Tui),
    #[usage(hide)]
    Usage(usage::Usage),
    #[usage(alias = "w")]
    Wait(wait::Wait),
    #[usage(external_subcommand)]
    Fallback(Vec<OsString>),
}

/// Parses tokens captured by the implicit subcommand fallback as a
/// `pitchfork start` invocation, so usage/help/error output reflects that.
#[derive(Debug, usage_rs::Cli)]
#[usage(
    name = "pitchfork",
    bin = "pitchfork start",
    version = env!("CARGO_PKG_VERSION"),
    long_about = start::LONG_ABOUT,
    unknown_flags = "error"
)]
struct StartFallback {
    #[usage(flatten)]
    start: start::Start,
}

pub async fn run() -> Result<()> {
    let args = Cli::parse();
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
        Commands::LogSink(log_sink) => log_sink.run().await,
        Commands::Logs(logs) => logs.run().await,
        Commands::Mcp(mcp) => mcp.run().await,
        Commands::Proxy(proxy) => proxy.run().await,
        Commands::Project(project) => project.run().await,
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
            let argv: Vec<_> = tokens.iter().map(OsString::as_os_str).collect();
            match StartFallback::parse_from(&argv) {
                Ok(fallback) => fallback.start.run().await,
                Err(usage_rs::Error::Help { cmd, long }) => {
                    let rendered = usage_rs::help::render(StartFallback::spec(), cmd, long)
                        .expect("derived help metadata should render");
                    let mut stdout = tokio::io::stdout();
                    stdout
                        .write_all(rendered.as_bytes())
                        .await
                        .into_diagnostic()?;
                    stdout.flush().await.into_diagnostic()?;
                    Ok(())
                }
                Err(usage_rs::Error::Version { .. }) => {
                    let rendered = format!("pitchfork {}\n", env!("CARGO_PKG_VERSION"));
                    let mut stdout = tokio::io::stdout();
                    stdout
                        .write_all(rendered.as_bytes())
                        .await
                        .into_diagnostic()?;
                    stdout.flush().await.into_diagnostic()?;
                    Ok(())
                }
                Err(error) => Err(miette::miette!(usage_rs::render_failure(
                    StartFallback::spec(),
                    &argv,
                    &error,
                ))),
            }
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
    use std::ffi::OsStr;

    #[test]
    fn bare_invocation_requests_help_instead_of_entering_the_start_fallback() {
        assert!(matches!(
            Cli::parse_from(&[]),
            Err(usage_rs::Error::Help { .. })
        ));
    }

    #[test]
    fn help_topics_are_claimed_before_the_external_start_fallback() {
        assert!(matches!(
            Cli::parse_from(&[OsStr::new("help")]),
            Err(usage_rs::Error::Help { .. })
        ));
        assert!(matches!(
            Cli::parse_from(&[OsStr::new("help"), OsStr::new("start")]),
            Err(usage_rs::Error::Help { .. })
        ));
    }

    #[test]
    fn unknown_subcommand_captured_as_fallback() {
        let argv = [OsStr::new("mydaemon"), OsStr::new("--force")];
        let args = Cli::parse_from(&argv).unwrap();
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
        let argv = [
            OsStr::new("api"),
            OsStr::new("worker"),
            OsStr::new("--force"),
        ];
        let args = Cli::parse_from(&argv).unwrap();
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
        let argv = [OsStr::new("start"), OsStr::new("mydaemon")];
        let args = Cli::parse_from(&argv).unwrap();
        match args.command {
            Commands::Start(_) => {}
            _ => panic!("expected Start variant, got {:?}", args.command),
        }
    }

    #[test]
    fn start_alias_still_works() {
        let argv = [OsStr::new("s"), OsStr::new("mydaemon")];
        let args = Cli::parse_from(&argv).unwrap();
        match args.command {
            Commands::Start(_) => {}
            _ => panic!("expected Start variant, got {:?}", args.command),
        }
    }

    #[test]
    fn fallback_reparse_as_start() {
        let argv = [OsStr::new("mydaemon"), OsStr::new("--force")];
        StartFallback::parse_from(&argv).expect("should re-parse captured tokens as Start");
    }

    #[test]
    fn fallback_reparse_rejects_invalid_start_flag() {
        let argv = [OsStr::new("mydaemon"), OsStr::new("--not-a-start-flag")];
        let result = StartFallback::parse_from(&argv);
        assert!(
            result.is_err(),
            "expected re-parse to fail for invalid Start flag"
        );
    }

    #[test]
    fn fallback_invalid_start_usage_renders_pitchfork_start() {
        let argv = [OsStr::new("mydaemon"), OsStr::new("--not-a-start-flag")];
        let err = StartFallback::parse_from(&argv).unwrap_err();
        let rendered = usage_rs::render_failure(StartFallback::spec(), &argv, &err);
        assert!(
            rendered.contains("Usage: pitchfork start"),
            "expected usage to contain 'pitchfork start', got: {rendered}"
        );
    }

    #[test]
    fn fallback_help_shows_start_long_about() {
        let argv = [OsStr::new("mydaemon"), OsStr::new("--help")];
        let usage_rs::Error::Help { cmd, long } = StartFallback::parse_from(&argv).unwrap_err()
        else {
            panic!("expected help")
        };
        let rendered = usage_rs::help::render(StartFallback::spec(), cmd, long).unwrap();
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
