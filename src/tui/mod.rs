mod app;
mod event;
mod ui;

use crate::Result;
use crate::ipc::client::IpcClient;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use log::LevelFilter;
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use std::io;
use std::time::Duration;

pub use app::App;

const REFRESH_RATE: Duration = Duration::from_secs(2);
const TICK_RATE: Duration = Duration::from_millis(100);

pub async fn run() -> Result<()> {
    // Suppress terminal logging while TUI is active (logs still go to file)
    let prev_log_level = log::max_level();
    log::set_max_level(LevelFilter::Off);

    // Setup terminal
    enable_raw_mode().into_diagnostic()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).into_diagnostic()?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).into_diagnostic()?;

    // Run with cleanup guaranteed
    let result = run_with_cleanup(&mut terminal).await;

    // Restore terminal (always runs)
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();

    // Restore log level
    log::set_max_level(prev_log_level);

    result
}

async fn run_with_cleanup<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    // Connect to supervisor (auto-start if needed)
    let client = IpcClient::connect(true).await?;

    // Create app state
    let mut app = App::new();
    app.refresh(&client).await?;

    // Run main loop
    run_app(terminal, &mut app, &client).await
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    client: &IpcClient,
) -> Result<()> {
    let mut last_refresh = std::time::Instant::now();

    loop {
        // Draw UI
        terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;

        // Handle events with timeout
        if crossterm::event::poll(TICK_RATE).into_diagnostic()?
            && let Some(action) = event::handle_event(app)?
        {
            match action {
                event::Action::Quit => break,
                event::Action::Start(id) => {
                    app.start_loading(format!("Starting {}...", id));
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                    // Handle start errors gracefully (don't crash TUI)
                    if let Err(e) = app.start_daemon(client, &id).await {
                        app.stop_loading();
                        app.set_message(format!("Failed to start {}: {}", id, e));
                    } else {
                        app.stop_loading();
                    }
                    app.refresh(client).await?;
                }
                event::Action::Enable(id) => {
                    app.start_loading(format!("Enabling {}...", id));
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                    client.enable(id.clone()).await?;
                    app.stop_loading();
                    app.set_message(format!("Enabled {}", id));
                    app.refresh(client).await?;
                }
                event::Action::BatchStart(ids) => {
                    let count = ids.len();
                    app.start_loading(format!("Starting {} daemons...", count));
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                    let mut started = 0;
                    for id in &ids {
                        if app.start_daemon(client, id).await.is_ok() {
                            started += 1;
                        }
                    }
                    app.stop_loading();
                    app.clear_selection();
                    app.set_message(format!("Started {}/{} daemons", started, count));
                    app.refresh(client).await?;
                }
                event::Action::BatchEnable(ids) => {
                    let count = ids.len();
                    app.start_loading(format!("Enabling {} daemons...", count));
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                    for id in &ids {
                        let _ = client.enable(id.clone()).await;
                    }
                    app.stop_loading();
                    app.clear_selection();
                    app.set_message(format!("Enabled {} daemons", count));
                    app.refresh(client).await?;
                }
                event::Action::Refresh => {
                    app.start_loading("Refreshing...");
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                    app.refresh(client).await?;
                    app.stop_loading();
                }
                event::Action::ConfirmPending => {
                    if let Some(pending) = app.take_pending_action() {
                        match pending {
                            app::PendingAction::Stop(id) => {
                                app.start_loading(format!("Stopping {}...", id));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                client.stop(id.clone()).await?;
                                app.stop_loading();
                                app.set_message(format!("Stopped {}", id));
                            }
                            app::PendingAction::Restart(id) => {
                                app.start_loading(format!("Restarting {}...", id));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                client.stop(id.clone()).await?;
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                // Handle start errors gracefully (don't crash TUI)
                                if let Err(e) = app.start_daemon(client, &id).await {
                                    app.stop_loading();
                                    app.set_message(format!(
                                        "Stopped {} but failed to restart: {}",
                                        id, e
                                    ));
                                } else {
                                    app.stop_loading();
                                    app.set_message(format!("Restarted {}", id));
                                }
                            }
                            app::PendingAction::Disable(id) => {
                                app.start_loading(format!("Disabling {}...", id));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                client.disable(id.clone()).await?;
                                app.stop_loading();
                                app.set_message(format!("Disabled {}", id));
                            }
                            app::PendingAction::BatchStop(ids) => {
                                let count = ids.len();
                                app.start_loading(format!("Stopping {} daemons...", count));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                for id in &ids {
                                    let _ = client.stop(id.clone()).await;
                                }
                                app.stop_loading();
                                app.clear_selection();
                                app.set_message(format!("Stopped {} daemons", count));
                            }
                            app::PendingAction::BatchRestart(ids) => {
                                let count = ids.len();
                                app.start_loading(format!("Restarting {} daemons...", count));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                // Stop all first
                                for id in &ids {
                                    let _ = client.stop(id.clone()).await;
                                }
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                // Start all
                                let mut started = 0;
                                for id in &ids {
                                    if app.start_daemon(client, id).await.is_ok() {
                                        started += 1;
                                    }
                                }
                                app.stop_loading();
                                app.clear_selection();
                                app.set_message(format!("Restarted {}/{} daemons", started, count));
                            }
                            app::PendingAction::BatchDisable(ids) => {
                                let count = ids.len();
                                app.start_loading(format!("Disabling {} daemons...", count));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                for id in &ids {
                                    let _ = client.disable(id.clone()).await;
                                }
                                app.stop_loading();
                                app.clear_selection();
                                app.set_message(format!("Disabled {} daemons", count));
                            }
                        }
                        app.refresh(client).await?;
                    }
                }
            }
        }

        // Auto-refresh daemon list
        if last_refresh.elapsed() >= REFRESH_RATE {
            app.refresh(client).await?;
            last_refresh = std::time::Instant::now();
        }
    }

    Ok(())
}
