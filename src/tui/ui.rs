use crate::daemon_status::DaemonStatus;
use crate::tui::app::{App, PendingAction, View};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};

// Theme colors matching the web UI's "devilish" theme
const RED: Color = Color::Rgb(220, 38, 38); // #dc2626
const ORANGE: Color = Color::Rgb(255, 107, 0); // #ff6b00
const GREEN: Color = Color::Rgb(34, 197, 94);
const YELLOW: Color = Color::Rgb(234, 179, 8);
const GRAY: Color = Color::Rgb(107, 114, 128);

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(3), // Stats
            Constraint::Min(0),    // Main content
            Constraint::Length(1), // Message bar
            Constraint::Length(1), // Footer
        ])
        .split(f.area());

    draw_header(f, chunks[0]);
    draw_stats(f, chunks[1], app);
    draw_main(f, chunks[2], app);
    draw_message_bar(f, chunks[3], app);
    draw_footer(f, chunks[4], app);

    // Draw overlays
    match app.view {
        View::Help => draw_help_overlay(f),
        View::Confirm => draw_confirm_overlay(f, app),
        View::Loading => draw_loading_overlay(f, app),
        _ => {}
    }
}

fn draw_header(f: &mut Frame, area: Rect) {
    // Gradient from orange to red: p i t c h f o r k
    let title = vec![
        Span::styled("p", Style::default().fg(Color::Rgb(255, 140, 0)).bold()), // dark orange
        Span::styled("i", Style::default().fg(Color::Rgb(255, 120, 0)).bold()),
        Span::styled("t", Style::default().fg(Color::Rgb(255, 100, 0)).bold()),
        Span::styled("c", Style::default().fg(Color::Rgb(240, 80, 20)).bold()),
        Span::styled("h", Style::default().fg(Color::Rgb(230, 60, 30)).bold()),
        Span::styled("f", Style::default().fg(Color::Rgb(220, 50, 38)).bold()), // red
        Span::styled("o", Style::default().fg(Color::Rgb(210, 45, 40)).bold()),
        Span::styled("r", Style::default().fg(Color::Rgb(200, 40, 45)).bold()),
        Span::styled("k", Style::default().fg(Color::Rgb(190, 38, 50)).bold()), // darker red
    ];
    let header = Paragraph::new(Line::from(title))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(RED)),
        );
    f.render_widget(header, area);
}

fn draw_stats(f: &mut Frame, area: Rect, app: &App) {
    let (total, running, stopped, errored) = app.stats();

    let stats = Line::from(vec![
        Span::styled("Total: ", Style::default().fg(Color::White)),
        Span::styled(total.to_string(), Style::default().fg(Color::White).bold()),
        Span::raw("  "),
        Span::styled("Running: ", Style::default().fg(GREEN)),
        Span::styled(running.to_string(), Style::default().fg(GREEN).bold()),
        Span::raw("  "),
        Span::styled("Stopped: ", Style::default().fg(GRAY)),
        Span::styled(stopped.to_string(), Style::default().fg(GRAY).bold()),
        Span::raw("  "),
        Span::styled("Errored: ", Style::default().fg(RED)),
        Span::styled(errored.to_string(), Style::default().fg(RED).bold()),
    ]);

    let stats_widget = Paragraph::new(stats).alignment(Alignment::Center);
    f.render_widget(stats_widget, area);
}

fn draw_main(f: &mut Frame, area: Rect, app: &App) {
    match app.view {
        View::Dashboard | View::Confirm | View::Loading => draw_daemon_table(f, area, app),
        View::Logs => draw_logs(f, area, app),
        View::Help => draw_daemon_table(f, area, app), // Help is an overlay
    }
}

fn draw_daemon_table(f: &mut Frame, area: Rect, app: &App) {
    // Split area for search bar (if active or has query) and table
    let (search_area, table_area) = if app.search_active || !app.search_query.is_empty() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, area)
    };

    // Draw search bar if present
    if let Some(search_area) = search_area {
        draw_search_bar(f, search_area, app);
    }

    let filtered = app.filtered_daemons();

    if filtered.is_empty() {
        let msg = if app.daemons.is_empty() {
            "No daemons running. Start one with: pitchfork start <name>"
        } else {
            "No daemons match the search query"
        };
        let paragraph = Paragraph::new(msg)
            .alignment(Alignment::Center)
            .style(Style::default().fg(GRAY))
            .block(
                Block::default()
                    .title(" Daemons ")
                    .title_style(Style::default().fg(RED).bold())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(RED)),
            );
        f.render_widget(paragraph, table_area);
        return;
    }

    let header_cells = ["Name", "PID", "Status", "CPU", "Mem", "Uptime", "Error"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(ORANGE).bold()));
    let header = Row::new(header_cells).height(1);

    let rows = filtered.iter().enumerate().map(|(i, daemon)| {
        let selected = i == app.selected;
        let disabled = app.is_disabled(&daemon.id);

        let name_style = if disabled {
            Style::default().fg(GRAY).italic()
        } else if selected {
            Style::default().fg(Color::White).bold()
        } else {
            Style::default().fg(Color::White)
        };

        let name = if disabled {
            format!("{} (disabled)", daemon.id)
        } else {
            daemon.id.clone()
        };

        let pid = daemon
            .pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());

        let (status_text, status_color) = status_display(&daemon.status);

        let stats = daemon.pid.and_then(|pid| app.get_stats(pid));
        let cpu = stats
            .map(|s| s.cpu_display())
            .unwrap_or_else(|| "-".to_string());
        let mem = stats
            .map(|s| s.memory_display())
            .unwrap_or_else(|| "-".to_string());
        let uptime = stats
            .map(|s| s.uptime_display())
            .unwrap_or_else(|| "-".to_string());

        let error = daemon.status.error_message().unwrap_or_default();

        let row_style = if selected {
            Style::default().bg(Color::Rgb(50, 20, 20))
        } else {
            Style::default()
        };

        Row::new(vec![
            Cell::from(name).style(name_style),
            Cell::from(pid),
            Cell::from(status_text).style(Style::default().fg(status_color)),
            Cell::from(cpu).style(Style::default().fg(GRAY)),
            Cell::from(mem).style(Style::default().fg(GRAY)),
            Cell::from(uptime).style(Style::default().fg(GRAY)),
            Cell::from(error).style(Style::default().fg(RED)),
        ])
        .style(row_style)
        .height(1)
    });

    let widths = [
        Constraint::Percentage(20),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Percentage(25),
    ];

    let title = if !app.search_query.is_empty() {
        format!(" Daemons ({} of {}) ", filtered.len(), app.daemons.len())
    } else {
        " Daemons ".to_string()
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(title)
                .title_style(Style::default().fg(RED).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RED)),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(50, 20, 20)));

    f.render_widget(table, table_area);
}

fn draw_search_bar(f: &mut Frame, area: Rect, app: &App) {
    let search_text = if app.search_active {
        format!("/{}_", app.search_query)
    } else {
        format!("/{}", app.search_query)
    };

    let search_bar = Paragraph::new(search_text)
        .style(if app.search_active {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(GRAY)
        })
        .block(
            Block::default()
                .title(" Search ")
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(if app.search_active {
                    Style::default().fg(ORANGE)
                } else {
                    Style::default().fg(GRAY)
                }),
        );
    f.render_widget(search_bar, area);
}

fn status_display(status: &DaemonStatus) -> (String, Color) {
    match status {
        DaemonStatus::Running => ("running".to_string(), GREEN),
        DaemonStatus::Stopped => ("stopped".to_string(), GRAY),
        DaemonStatus::Waiting => ("waiting".to_string(), YELLOW),
        DaemonStatus::Stopping => ("stopping".to_string(), YELLOW),
        DaemonStatus::Failed(_) => ("failed".to_string(), RED),
        DaemonStatus::Errored(code) => {
            let text = code
                .map(|c| format!("errored ({})", c))
                .unwrap_or_else(|| "errored".to_string());
            (text, RED)
        }
    }
}

fn draw_logs(f: &mut Frame, area: Rect, app: &App) {
    let daemon_name = app.log_daemon_id.as_deref().unwrap_or("unknown");
    let follow_indicator = if app.log_follow { " [follow]" } else { "" };
    let title = format!(" Logs: {}{} ", daemon_name, follow_indicator);

    let visible_lines: Vec<Line> = app
        .log_content
        .iter()
        .skip(app.log_scroll)
        .take(area.height.saturating_sub(2) as usize)
        .map(|line| Line::from(line.as_str()))
        .collect();

    let logs = Paragraph::new(visible_lines)
        .block(
            Block::default()
                .title(title)
                .title_style(Style::default().fg(RED).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RED)),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(logs, area);
}

fn draw_message_bar(f: &mut Frame, area: Rect, app: &App) {
    if let Some(msg) = &app.message {
        let message = Paragraph::new(msg.as_str())
            .style(Style::default().fg(GREEN))
            .alignment(Alignment::Center);
        f.render_widget(message, area);
    }
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let help_text = match app.view {
        View::Dashboard if app.search_active => "Type to search  Enter:finish  Esc:clear",
        View::Dashboard if !app.search_query.is_empty() => {
            "/:search  q/Esc:clear  j/k:navigate  s:start  x:stop  r:restart  l:logs  ?:help"
        }
        View::Dashboard => {
            "/:search  q:quit  j/k:navigate  s:start  x:stop  r:restart  l:logs  ?:help"
        }
        View::Logs => "q/Esc:back  j/k:scroll  f:follow  g:top  G:bottom",
        View::Help => "q/Esc/?:close",
        View::Confirm => "y/Enter:confirm  n/Esc:cancel",
        View::Loading => "Please wait...",
    };

    let footer = Paragraph::new(help_text)
        .style(Style::default().fg(GRAY))
        .alignment(Alignment::Center);
    f.render_widget(footer, area);
}

fn draw_help_overlay(f: &mut Frame) {
    let area = centered_rect(60, 70, f.area());

    // Clear the background
    f.render_widget(Clear, area);

    let help_text = vec![
        Line::from(vec![Span::styled(
            "Keyboard Shortcuts",
            Style::default().fg(ORANGE).bold(),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Navigation",
            Style::default().fg(RED).bold(),
        )]),
        Line::from("  j / Down    Move selection down"),
        Line::from("  k / Up      Move selection up"),
        Line::from("  l / Enter   View logs for selected daemon"),
        Line::from("  /           Search/filter daemons"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Actions",
            Style::default().fg(RED).bold(),
        )]),
        Line::from("  s           Start stopped daemon"),
        Line::from("  x           Stop running daemon"),
        Line::from("  r           Restart daemon"),
        Line::from("  e           Enable disabled daemon"),
        Line::from("  d           Disable daemon"),
        Line::from("  R           Force refresh"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "General",
            Style::default().fg(RED).bold(),
        )]),
        Line::from("  ?           Toggle this help"),
        Line::from("  q           Quit / Go back"),
        Line::from("  Ctrl+C      Force quit"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Log View",
            Style::default().fg(RED).bold(),
        )]),
        Line::from("  j / k       Scroll up/down"),
        Line::from("  f           Toggle follow mode"),
        Line::from("  g           Go to top"),
        Line::from("  G           Go to bottom (enables follow)"),
        Line::from("  q / Esc     Return to dashboard"),
    ];

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .title(" Help ")
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RED)),
        )
        .style(Style::default().bg(Color::Rgb(20, 20, 20)));

    f.render_widget(help, area);
}

fn draw_loading_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(40, 20, f.area());

    // Clear the background
    f.render_widget(Clear, area);

    let text = app.loading_text.as_deref().unwrap_or("Loading...");

    let content = vec![
        Line::from(""),
        Line::from(vec![Span::styled(text, Style::default().fg(ORANGE).bold())]),
        Line::from(""),
    ];

    let loading = Paragraph::new(content)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RED)),
        )
        .style(Style::default().bg(Color::Rgb(30, 20, 20)));

    f.render_widget(loading, area);
}

fn draw_confirm_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(50, 30, f.area());

    // Clear the background
    f.render_widget(Clear, area);

    let (action_text, daemon_id) = match &app.pending_action {
        Some(PendingAction::Stop(id)) => ("Stop", id.as_str()),
        Some(PendingAction::Restart(id)) => ("Restart", id.as_str()),
        Some(PendingAction::Disable(id)) => ("Disable", id.as_str()),
        None => ("Unknown", "unknown"),
    };

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(action_text, Style::default().fg(ORANGE).bold()),
            Span::raw(" daemon "),
            Span::styled(daemon_id, Style::default().fg(Color::White).bold()),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled("y", Style::default().fg(GREEN).bold()),
            Span::raw(" / "),
            Span::styled("Enter", Style::default().fg(GREEN).bold()),
            Span::raw(" to confirm, "),
            Span::styled("n", Style::default().fg(RED).bold()),
            Span::raw(" / "),
            Span::styled("Esc", Style::default().fg(RED).bold()),
            Span::raw(" to cancel"),
        ]),
    ];

    let confirm = Paragraph::new(text)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .title(" Confirm ")
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RED)),
        )
        .style(Style::default().bg(Color::Rgb(30, 20, 20)));

    f.render_widget(confirm, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
