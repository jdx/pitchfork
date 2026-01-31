mod app;
mod event;
mod ui;

use crate::Result;
use crate::ipc::batch::StartOptions;
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
use std::sync::Arc;
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
    let client = Arc::new(IpcClient::connect(true).await?);

    // Create app state
    let mut app = App::new();
    app.refresh(&client).await?;

    // Run main loop
    run_app(terminal, &mut app, &client).await
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    client: &Arc<IpcClient>,
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
                    app.start_loading(format!("Starting {id}..."));
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;

                    let result = client
                        .start_daemons(std::slice::from_ref(&id), StartOptions::default())
                        .await;

                    app.stop_loading();
                    match result {
                        Ok(r) if r.any_failed => {
                            app.set_message(format!("Failed to start {id}"));
                        }
                        Ok(r) if !r.started.is_empty() => {
                            app.set_message(format!("Started {id}"));
                        }
                        Ok(_) => {
                            app.set_message(format!("No daemons were started for {id}"));
                        }
                        Err(e) => {
                            app.set_message(format!("Failed to start {id}: {e}"));
                        }
                    }
                    app.refresh(client).await?;
                }
                event::Action::Enable(id) => {
                    app.start_loading(format!("Enabling {id}..."));
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                    client.enable(id.clone()).await?;
                    app.stop_loading();
                    app.set_message(format!("Enabled {id}"));
                    app.refresh(client).await?;
                }
                event::Action::BatchStart(ids) => {
                    let count = ids.len();
                    app.start_loading(format!("Starting {count} daemons..."));
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;

                    let result = client.start_daemons(&ids, StartOptions::default()).await;

                    app.stop_loading();
                    app.clear_selection();
                    match result {
                        Ok(r) => {
                            let started = r.started.len();
                            if r.any_failed {
                                app.set_message(format!(
                                    "Started {started}/{count} daemons (some failed)"
                                ));
                            } else {
                                app.set_message(format!("Started {started} daemons"));
                            }
                        }
                        Err(e) => {
                            app.set_message(format!("Failed to start daemons: {e}"));
                        }
                    }
                    app.refresh(client).await?;
                }
                event::Action::BatchEnable(ids) => {
                    let count = ids.len();
                    app.start_loading(format!("Enabling {count} daemons..."));
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                    for id in &ids {
                        let _ = client.enable(id.clone()).await;
                    }
                    app.stop_loading();
                    app.clear_selection();
                    app.set_message(format!("Enabled {count} daemons"));
                    app.refresh(client).await?;
                }
                event::Action::Refresh => {
                    app.start_loading("Refreshing...");
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                    app.refresh(client).await?;
                    app.stop_loading();
                }
                event::Action::OpenEditorNew => {
                    app.open_file_selector();
                }
                event::Action::OpenEditorEdit(id) => {
                    app.open_editor_edit(&id);
                }
                event::Action::SaveConfig => {
                    app.start_loading("Saving...");
                    terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                    match app.save_editor_config() {
                        Ok(true) => {
                            // Successfully saved
                            app.stop_loading();
                            app.close_editor();
                            app.refresh(client).await?;
                        }
                        Ok(false) => {
                            // Validation or duplicate error - don't close editor
                            app.stop_loading();
                        }
                        Err(e) => {
                            app.stop_loading();
                            app.set_message(format!("Save failed: {e}"));
                        }
                    }
                }
                event::Action::DeleteDaemon { id, config_path } => {
                    app.confirm_action(app::PendingAction::DeleteDaemon { id, config_path });
                }
                event::Action::ConfirmPending => {
                    if let Some(pending) = app.take_pending_action() {
                        match pending {
                            app::PendingAction::Stop(id) => {
                                app.start_loading(format!("Stopping {id}..."));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                let result = client.stop(id.clone()).await;
                                app.stop_loading();
                                match result {
                                    Ok(true) => app.set_message(format!("Stopped {id}")),
                                    Ok(false) => {
                                        app.set_message(format!("Daemon {id} was not running"))
                                    }
                                    Err(e) => app.set_message(format!("Failed to stop {id}: {e}")),
                                }
                            }
                            app::PendingAction::Restart(id) => {
                                app.start_loading(format!("Restarting {id}..."));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;

                                // Restart is just start --force
                                let opts = StartOptions {
                                    force: true,
                                    ..Default::default()
                                };
                                let result =
                                    client.start_daemons(std::slice::from_ref(&id), opts).await;

                                app.stop_loading();
                                match result {
                                    Ok(r) if r.any_failed => {
                                        app.set_message(format!("Failed to restart {id}"));
                                    }
                                    Ok(_) => {
                                        app.set_message(format!("Restarted {id}"));
                                    }
                                    Err(e) => {
                                        app.set_message(format!("Failed to restart {id}: {e}"));
                                    }
                                }
                            }
                            app::PendingAction::Disable(id) => {
                                app.start_loading(format!("Disabling {id}..."));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                client.disable(id.clone()).await?;
                                app.stop_loading();
                                app.set_message(format!("Disabled {id}"));
                            }
                            app::PendingAction::BatchStop(ids) => {
                                let count = ids.len();
                                app.start_loading(format!("Stopping {count} daemons..."));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                let result = client.stop_daemons(&ids).await;
                                app.stop_loading();
                                app.clear_selection();
                                match result {
                                    Ok(r) if r.any_failed => {
                                        app.set_message(format!(
                                            "Stopped {count} daemons (some failed)"
                                        ));
                                    }
                                    Ok(_) => {
                                        app.set_message(format!("Stopped {count} daemons"));
                                    }
                                    Err(e) => {
                                        app.set_message(format!("Failed to stop daemons: {e}"));
                                    }
                                }
                            }
                            app::PendingAction::BatchRestart(ids) => {
                                let count = ids.len();
                                app.start_loading(format!("Restarting {count} daemons..."));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;

                                // Restart is just start --force
                                let opts = StartOptions {
                                    force: true,
                                    ..Default::default()
                                };
                                let result = client.start_daemons(&ids, opts).await;

                                app.stop_loading();
                                app.clear_selection();
                                match result {
                                    Ok(r) => {
                                        let restarted = r.started.len();
                                        if r.any_failed {
                                            app.set_message(format!("Restarted {restarted}/{count} daemons (some failed)"));
                                        } else {
                                            app.set_message(format!(
                                                "Restarted {restarted} daemons"
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        app.set_message(format!("Failed to restart daemons: {e}"));
                                    }
                                }
                            }
                            app::PendingAction::BatchDisable(ids) => {
                                let count = ids.len();
                                app.start_loading(format!("Disabling {count} daemons..."));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                for id in &ids {
                                    let _ = client.disable(id.clone()).await;
                                }
                                app.stop_loading();
                                app.clear_selection();
                                app.set_message(format!("Disabled {count} daemons"));
                            }
                            app::PendingAction::DeleteDaemon { id, config_path } => {
                                app.start_loading(format!("Deleting {id}..."));
                                terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;
                                match app.delete_daemon_from_config(&id, &config_path) {
                                    Ok(true) => {
                                        app.stop_loading();
                                        app.close_editor();
                                        app.set_message(format!("Deleted {id}"));
                                    }
                                    Ok(false) => {
                                        app.stop_loading();
                                        app.set_message(format!(
                                            "Daemon '{id}' not found in config"
                                        ));
                                    }
                                    Err(e) => {
                                        app.stop_loading();
                                        app.set_message(format!("Delete failed: {e}"));
                                    }
                                }
                            }
                            app::PendingAction::DiscardEditorChanges => {
                                app.close_editor();
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
