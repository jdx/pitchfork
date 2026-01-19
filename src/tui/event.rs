use crate::tui::app::{App, PendingAction, View};
use crate::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use miette::IntoDiagnostic;

pub enum Action {
    Quit,
    Start(String),
    Enable(String),
    Refresh,
    ConfirmPending,
}

pub fn handle_event(app: &mut App) -> Result<Option<Action>> {
    let event = event::read().into_diagnostic()?;

    match event {
        Event::Key(key) => {
            // Ctrl+C always quits
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                return Ok(Some(Action::Quit));
            }

            match app.view {
                View::Dashboard => handle_dashboard_event(app, key.code, key.modifiers),
                View::Logs => handle_logs_event(app, key.code, key.modifiers),
                View::Help => handle_help_event(app, key.code),
                View::Confirm => handle_confirm_event(app, key.code),
                View::Details => handle_details_event(app, key.code),
                View::Loading => Ok(None), // Ignore input during loading
            }
        }
        Event::Mouse(mouse) => {
            match app.view {
                View::Dashboard => handle_dashboard_mouse(app, mouse.kind, mouse.row),
                View::Logs => handle_logs_mouse(app, mouse.kind),
                View::Help | View::Details => {
                    // Click anywhere to close overlays
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        app.back_to_dashboard();
                    }
                    Ok(None)
                }
                View::Confirm => {
                    // Click anywhere to cancel (Esc behavior)
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        app.cancel_confirm();
                    }
                    Ok(None)
                }
                View::Loading => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

fn handle_dashboard_event(
    app: &mut App,
    key: KeyCode,
    _modifiers: KeyModifiers,
) -> Result<Option<Action>> {
    // Handle search input mode first
    if app.search_active {
        return handle_search_input(app, key);
    }

    match key {
        KeyCode::Char('q') => {
            // If search has a query, clear it first
            if !app.search_query.is_empty() {
                app.clear_search();
                Ok(None)
            } else {
                Ok(Some(Action::Quit))
            }
        }
        KeyCode::Char('/') => {
            app.start_search();
            Ok(None)
        }
        KeyCode::Esc => {
            if !app.search_query.is_empty() {
                app.clear_search();
                Ok(None)
            } else {
                Ok(Some(Action::Quit))
            }
        }
        KeyCode::Char('?') => {
            app.show_help();
            Ok(None)
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.select_next();
            Ok(None)
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.select_prev();
            Ok(None)
        }
        // Sort: 'S' cycles columns, 'o' toggles order
        KeyCode::Char('S') => {
            app.cycle_sort();
            Ok(None)
        }
        KeyCode::Char('o') => {
            app.toggle_sort_order();
            Ok(None)
        }
        // Details view
        KeyCode::Char('i') => {
            if let Some(daemon) = app.selected_daemon() {
                let id = daemon.id.clone();
                app.show_details(&id);
            }
            Ok(None)
        }
        KeyCode::Char('s') => {
            if let Some(daemon) = app.selected_daemon() {
                if daemon.status.is_stopped()
                    || daemon.status.is_errored()
                    || daemon.status.is_failed()
                {
                    return Ok(Some(Action::Start(daemon.id.clone())));
                }
            }
            Ok(None)
        }
        KeyCode::Char('x') => {
            // Stop requires confirmation
            if let Some(daemon) = app.selected_daemon() {
                if daemon.status.is_running() || daemon.status.is_waiting() {
                    app.confirm_action(PendingAction::Stop(daemon.id.clone()));
                }
            }
            Ok(None)
        }
        KeyCode::Char('r') => {
            // Restart requires confirmation
            if let Some(daemon) = app.selected_daemon() {
                if daemon.status.is_running() || daemon.status.is_waiting() {
                    app.confirm_action(PendingAction::Restart(daemon.id.clone()));
                } else {
                    // If not running, just start it (no confirmation needed)
                    return Ok(Some(Action::Start(daemon.id.clone())));
                }
            }
            Ok(None)
        }
        KeyCode::Char('e') => {
            if let Some(daemon) = app.selected_daemon() {
                if app.is_disabled(&daemon.id) {
                    return Ok(Some(Action::Enable(daemon.id.clone())));
                }
            }
            Ok(None)
        }
        KeyCode::Char('d') => {
            // Disable requires confirmation
            if let Some(daemon) = app.selected_daemon() {
                if !app.is_disabled(&daemon.id) {
                    app.confirm_action(PendingAction::Disable(daemon.id.clone()));
                }
            }
            Ok(None)
        }
        KeyCode::Char('l') | KeyCode::Enter => {
            if let Some(daemon) = app.selected_daemon() {
                let id = daemon.id.clone();
                app.view_daemon_details(&id);
            }
            Ok(None)
        }
        KeyCode::Char('R') => Ok(Some(Action::Refresh)),
        _ => Ok(None),
    }
}

fn handle_search_input(app: &mut App, key: KeyCode) -> Result<Option<Action>> {
    match key {
        KeyCode::Esc => {
            app.clear_search();
            Ok(None)
        }
        KeyCode::Enter => {
            app.end_search();
            Ok(None)
        }
        KeyCode::Backspace => {
            if app.search_query.is_empty() {
                app.end_search();
            } else {
                app.search_pop();
            }
            Ok(None)
        }
        KeyCode::Char(c) => {
            app.search_push(c);
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn handle_logs_event(
    app: &mut App,
    key: KeyCode,
    modifiers: KeyModifiers,
) -> Result<Option<Action>> {
    // Handle log search input mode first
    if app.log_search_active {
        return handle_log_search_input(app, key);
    }

    // Handle Ctrl+D/U for page scrolling
    if modifiers.contains(KeyModifiers::CONTROL) {
        match key {
            KeyCode::Char('d') => {
                app.log_follow = false;
                app.scroll_logs_page_down(20);
                return Ok(None);
            }
            KeyCode::Char('u') => {
                app.log_follow = false;
                app.scroll_logs_page_up(20);
                return Ok(None);
            }
            _ => {}
        }
    }

    match key {
        KeyCode::Char('q') => {
            if !app.log_search_query.is_empty() {
                app.clear_log_search();
            } else {
                app.back_to_dashboard();
            }
            Ok(None)
        }
        KeyCode::Esc => {
            if !app.log_search_query.is_empty() {
                app.clear_log_search();
            } else {
                app.back_to_dashboard();
            }
            Ok(None)
        }
        KeyCode::Char('/') => {
            app.start_log_search();
            Ok(None)
        }
        KeyCode::Char('n') => {
            app.log_search_next();
            Ok(None)
        }
        KeyCode::Char('N') => {
            app.log_search_prev();
            Ok(None)
        }
        KeyCode::Char('f') => {
            app.toggle_log_follow();
            Ok(None)
        }
        KeyCode::Char('e') => {
            app.toggle_logs_expanded();
            Ok(None)
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.log_follow = false;
            app.scroll_logs_down();
            Ok(None)
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.log_follow = false;
            app.scroll_logs_up();
            Ok(None)
        }
        KeyCode::PageDown => {
            app.log_follow = false;
            app.scroll_logs_page_down(20);
            Ok(None)
        }
        KeyCode::PageUp => {
            app.log_follow = false;
            app.scroll_logs_page_up(20);
            Ok(None)
        }
        KeyCode::Char('g') => {
            app.log_follow = false;
            app.log_scroll = 0;
            Ok(None)
        }
        KeyCode::Char('G') => {
            app.log_follow = true;
            if app.log_content.len() > 20 {
                app.log_scroll = app.log_content.len().saturating_sub(20);
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn handle_log_search_input(app: &mut App, key: KeyCode) -> Result<Option<Action>> {
    match key {
        KeyCode::Esc => {
            app.clear_log_search();
            Ok(None)
        }
        KeyCode::Enter => {
            app.end_log_search();
            Ok(None)
        }
        KeyCode::Backspace => {
            if app.log_search_query.is_empty() {
                app.end_log_search();
            } else {
                app.log_search_pop();
            }
            Ok(None)
        }
        KeyCode::Char(c) => {
            app.log_search_push(c);
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn handle_details_event(app: &mut App, key: KeyCode) -> Result<Option<Action>> {
    match key {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('i') => {
            app.hide_details();
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn handle_help_event(app: &mut App, key: KeyCode) -> Result<Option<Action>> {
    match key {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('?') => {
            app.back_to_dashboard();
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn handle_confirm_event(app: &mut App, key: KeyCode) -> Result<Option<Action>> {
    match key {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            // User confirmed - execute the pending action
            Ok(Some(Action::ConfirmPending))
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            // User cancelled
            app.cancel_confirm();
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn handle_dashboard_mouse(app: &mut App, kind: MouseEventKind, row: u16) -> Result<Option<Action>> {
    // Skip if search is active (don't interfere with typing)
    if app.search_active {
        return Ok(None);
    }

    match kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Calculate which daemon was clicked
            // Table starts at row 6 (header=3, stats=3) + 2 for table header/border
            let table_start = 8_u16;
            if row >= table_start {
                let clicked_index = (row - table_start) as usize;
                let filtered_count = app.filtered_daemons().len();
                if clicked_index < filtered_count {
                    app.selected = clicked_index;
                }
            }
            Ok(None)
        }
        MouseEventKind::ScrollDown => {
            app.select_next();
            Ok(None)
        }
        MouseEventKind::ScrollUp => {
            app.select_prev();
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn handle_logs_mouse(app: &mut App, kind: MouseEventKind) -> Result<Option<Action>> {
    // Skip if search is active
    if app.log_search_active {
        return Ok(None);
    }

    match kind {
        MouseEventKind::ScrollDown => {
            app.log_follow = false;
            app.scroll_logs_down();
            app.scroll_logs_down();
            app.scroll_logs_down();
            Ok(None)
        }
        MouseEventKind::ScrollUp => {
            app.log_follow = false;
            app.scroll_logs_up();
            app.scroll_logs_up();
            app.scroll_logs_up();
            Ok(None)
        }
        _ => Ok(None),
    }
}
