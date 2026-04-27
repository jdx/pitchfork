mod app;
mod event;
mod ui;

use crate::Result;
use crate::daemon_id::DaemonId;
use crate::daemon_list::DaemonListEntry;
use crate::ipc::batch::{StartOptions, StartResult, StopResult};
use crate::ipc::client::IpcClient;
use crate::settings::settings;
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

pub use app::App;

/// Results sent back from background IPC tasks to the main loop.
enum TaskResult {
    Start {
        id: DaemonId,
        result: crate::Result<StartResult>,
    },
    Stop {
        id: DaemonId,
        result: crate::Result<bool>,
    },
    Restart {
        id: DaemonId,
        result: crate::Result<StartResult>,
    },
    Enable {
        id: DaemonId,
        result: crate::Result<bool>,
    },
    Disable {
        id: DaemonId,
        result: crate::Result<bool>,
    },
    BatchStart {
        count: usize,
        result: crate::Result<StartResult>,
    },
    BatchStop {
        count: usize,
        result: crate::Result<StopResult>,
    },
    BatchRestart {
        count: usize,
        result: crate::Result<StartResult>,
    },
    BatchEnable {
        count: usize,
    },
    BatchDisable {
        count: usize,
    },
    Refresh(crate::Result<Vec<DaemonListEntry>>),
    RefreshNetwork(Vec<listeners::Listener>),
}

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

async fn run_with_cleanup(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    // Connect to supervisor (auto-start if needed)
    let client = Arc::new(IpcClient::connect(true).await?);

    // Create app state
    let mut app = App::new();
    app.refresh(&client).await?;

    // Run main loop
    run_app(terminal, &mut app, &client).await
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    client: &Arc<IpcClient>,
) -> Result<()> {
    let s = settings();
    let tick_rate = s.tui_tick_rate();
    let refresh_rate = s.tui_refresh_rate();
    let mut last_refresh = std::time::Instant::now();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TaskResult>();
    // True while an IPC operation (start/stop/etc.) is in flight.
    // Used to prevent overlapping operations. Navigation and other local
    // actions are always allowed.
    let mut in_flight = false;

    loop {
        // Draw UI
        terminal.draw(|f| ui::draw(f, app)).into_diagnostic()?;

        // Drain completed background task results
        while let Ok(result) = rx.try_recv() {
            match result {
                TaskResult::Start { id, result } => {
                    app.stop_loading();
                    in_flight = false;
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
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                TaskResult::Stop { id, result } => {
                    app.stop_loading();
                    in_flight = false;
                    match result {
                        Ok(true) => app.set_message(format!("Stopped {id}")),
                        Ok(false) => app.set_message(format!("Daemon {id} was not running")),
                        Err(e) => app.set_message(format!("Failed to stop {id}: {e}")),
                    }
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                TaskResult::Restart { id, result } => {
                    app.stop_loading();
                    in_flight = false;
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
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                TaskResult::Enable { id, result } => {
                    app.stop_loading();
                    in_flight = false;
                    match result {
                        Ok(_) => app.set_message(format!("Enabled {id}")),
                        Err(e) => app.set_message(format!("Failed to enable {id}: {e}")),
                    }
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                TaskResult::Disable { id, result } => {
                    app.stop_loading();
                    in_flight = false;
                    match result {
                        Ok(_) => app.set_message(format!("Disabled {id}")),
                        Err(e) => app.set_message(format!("Failed to disable {id}: {e}")),
                    }
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                TaskResult::BatchStart { count, result } => {
                    app.stop_loading();
                    in_flight = false;
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
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                TaskResult::BatchStop { count, result } => {
                    app.stop_loading();
                    in_flight = false;
                    app.clear_selection();
                    match result {
                        Ok(r) if r.any_failed => {
                            app.set_message(format!("Stopped {count} daemons (some failed)"));
                        }
                        Ok(_) => {
                            app.set_message(format!("Stopped {count} daemons"));
                        }
                        Err(e) => {
                            app.set_message(format!("Failed to stop daemons: {e}"));
                        }
                    }
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                TaskResult::BatchRestart { count, result } => {
                    app.stop_loading();
                    in_flight = false;
                    app.clear_selection();
                    match result {
                        Ok(r) => {
                            let restarted = r.started.len();
                            if r.any_failed {
                                app.set_message(format!(
                                    "Restarted {restarted}/{count} daemons (some failed)"
                                ));
                            } else {
                                app.set_message(format!("Restarted {restarted} daemons"));
                            }
                        }
                        Err(e) => {
                            app.set_message(format!("Failed to restart daemons: {e}"));
                        }
                    }
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                TaskResult::BatchEnable { count } => {
                    app.stop_loading();
                    in_flight = false;
                    app.clear_selection();
                    app.set_message(format!("Enabled {count} daemons"));
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                TaskResult::BatchDisable { count } => {
                    app.stop_loading();
                    in_flight = false;
                    app.clear_selection();
                    app.set_message(format!("Disabled {count} daemons"));
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                TaskResult::Refresh(result) => {
                    match result {
                        Ok(entries) => app.apply_refresh(entries),
                        Err(e) => app.set_message(format!("Refresh failed: {e}")),
                    }
                    last_refresh = std::time::Instant::now();
                }
                TaskResult::RefreshNetwork(listeners) => {
                    app.apply_network_refresh(listeners);
                }
            }
        }

        // Handle events with timeout
        if crossterm::event::poll(tick_rate).into_diagnostic()?
            && let Some(action) = event::handle_event(app)?
        {
            match action {
                event::Action::Quit => break,
                event::Action::Start(id) if !in_flight => {
                    in_flight = true;
                    app.start_loading(format!("Starting {id}..."));
                    let client = Arc::clone(client);
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let result = client
                            .start_daemons(std::slice::from_ref(&id), StartOptions::default())
                            .await;
                        let _ = tx.send(TaskResult::Start { id, result });
                    });
                }
                event::Action::Enable(id) if !in_flight => {
                    in_flight = true;
                    app.start_loading(format!("Enabling {id}..."));
                    let client = Arc::clone(client);
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let result = client.enable(id.clone()).await;
                        let _ = tx.send(TaskResult::Enable { id, result });
                    });
                }
                event::Action::BatchStart(ids) if !in_flight => {
                    let count = ids.len();
                    in_flight = true;
                    app.start_loading(format!("Starting {count} daemons..."));
                    let client = Arc::clone(client);
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let result = client.start_daemons(&ids, StartOptions::default()).await;
                        let _ = tx.send(TaskResult::BatchStart { count, result });
                    });
                }
                event::Action::BatchEnable(ids) if !in_flight => {
                    let count = ids.len();
                    in_flight = true;
                    app.start_loading(format!("Enabling {count} daemons..."));
                    let client = Arc::clone(client);
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        for id in &ids {
                            let _ = client.enable(id.clone()).await;
                        }
                        let _ = tx.send(TaskResult::BatchEnable { count });
                    });
                }
                event::Action::Refresh => {
                    app.start_loading("Refreshing...");
                    spawn_refresh(Arc::clone(client), tx.clone());
                }
                event::Action::OpenEditorNew => {
                    app.open_file_selector();
                }
                event::Action::OpenEditorEdit(id) => {
                    app.open_editor_edit(&id);
                }
                event::Action::SaveConfig => {
                    app.start_loading("Saving...");
                    match app.save_editor_config() {
                        Ok(true) => {
                            app.stop_loading();
                            app.close_editor();
                            spawn_refresh(Arc::clone(client), tx.clone());
                        }
                        Ok(false) => {
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
                event::Action::ConfirmPending if !in_flight => {
                    if let Some(pending) = app.take_pending_action() {
                        match pending {
                            app::PendingAction::Stop(id) => {
                                in_flight = true;
                                app.start_loading(format!("Stopping {id}..."));
                                let client = Arc::clone(client);
                                let tx = tx.clone();
                                tokio::spawn(async move {
                                    let result = client.stop(id.clone()).await;
                                    let _ = tx.send(TaskResult::Stop { id, result });
                                });
                            }
                            app::PendingAction::Restart(id) => {
                                in_flight = true;
                                app.start_loading(format!("Restarting {id}..."));
                                let client = Arc::clone(client);
                                let tx = tx.clone();
                                tokio::spawn(async move {
                                    let opts = StartOptions {
                                        force: true,
                                        ..Default::default()
                                    };
                                    let result =
                                        client.start_daemons(std::slice::from_ref(&id), opts).await;
                                    let _ = tx.send(TaskResult::Restart { id, result });
                                });
                            }
                            app::PendingAction::Disable(id) => {
                                in_flight = true;
                                app.start_loading(format!("Disabling {id}..."));
                                let client = Arc::clone(client);
                                let tx = tx.clone();
                                tokio::spawn(async move {
                                    let result = client.disable(id.clone()).await;
                                    let _ = tx.send(TaskResult::Disable { id, result });
                                });
                            }
                            app::PendingAction::BatchStop(ids) => {
                                let count = ids.len();
                                in_flight = true;
                                app.start_loading(format!("Stopping {count} daemons..."));
                                let client = Arc::clone(client);
                                let tx = tx.clone();
                                tokio::spawn(async move {
                                    let result = client.stop_daemons(&ids).await;
                                    let _ = tx.send(TaskResult::BatchStop { count, result });
                                });
                            }
                            app::PendingAction::BatchRestart(ids) => {
                                let count = ids.len();
                                in_flight = true;
                                app.start_loading(format!("Restarting {count} daemons..."));
                                let client = Arc::clone(client);
                                let tx = tx.clone();
                                tokio::spawn(async move {
                                    let opts = StartOptions {
                                        force: true,
                                        ..Default::default()
                                    };
                                    let result = client.start_daemons(&ids, opts).await;
                                    let _ = tx.send(TaskResult::BatchRestart { count, result });
                                });
                            }
                            app::PendingAction::BatchDisable(ids) => {
                                let count = ids.len();
                                in_flight = true;
                                app.start_loading(format!("Disabling {count} daemons..."));
                                let client = Arc::clone(client);
                                let tx = tx.clone();
                                tokio::spawn(async move {
                                    for id in &ids {
                                        let _ = client.disable(id.clone()).await;
                                    }
                                    let _ = tx.send(TaskResult::BatchDisable { count });
                                });
                            }
                            app::PendingAction::DeleteDaemon { id, config_path } => {
                                app.start_loading(format!("Deleting {id}..."));
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
                                spawn_refresh(Arc::clone(client), tx.clone());
                            }
                            app::PendingAction::DiscardEditorChanges => {
                                app.close_editor();
                            }
                        }
                    }
                }
                // Ignore IPC actions when in_flight (navigation/local actions fall through)
                _ => {}
            }
        }

        // Auto-refresh daemon list (skip if IPC operation in flight)
        if last_refresh.elapsed() >= refresh_rate && !in_flight {
            let is_network = app.view == app::View::Network;
            let client_ref = Arc::clone(client);
            let tx_ref = tx.clone();
            tokio::spawn(async move {
                let entries = App::fetch_daemon_data(&client_ref).await;
                let _ = tx_ref.send(TaskResult::Refresh(entries));
                if is_network {
                    let listeners = tokio::task::spawn_blocking(|| {
                        listeners::get_all()
                            .map(|set| set.into_iter().collect::<Vec<_>>())
                            .unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default();
                    let _ = tx_ref.send(TaskResult::RefreshNetwork(listeners));
                }
            });
            // Optimistically advance the timer so we don't spam refreshes
            last_refresh = std::time::Instant::now();
        }
    }

    Ok(())
}

fn spawn_refresh(client: Arc<IpcClient>, tx: tokio::sync::mpsc::UnboundedSender<TaskResult>) {
    tokio::spawn(async move {
        let entries = App::fetch_daemon_data(&client).await;
        let _ = tx.send(TaskResult::Refresh(entries));
    });
}
