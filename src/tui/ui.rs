use crate::daemon_status::DaemonStatus;
use crate::pitchfork_toml::PitchforkToml;
use crate::tui::app::{App, PendingAction, SortColumn, StatsHistory, View};
use ratatui::{
    prelude::*,
    symbols,
    widgets::{
        Axis, Block, Borders, Cell, Chart, Clear, Dataset, GraphType, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, Wrap,
    },
};

// Theme colors matching the web UI's "devilish" theme
const RED: Color = Color::Rgb(220, 38, 38); // #dc2626
const ORANGE: Color = Color::Rgb(255, 107, 0); // #ff6b00
const GREEN: Color = Color::Rgb(34, 197, 94);
const YELLOW: Color = Color::Rgb(234, 179, 8);
const GRAY: Color = Color::Rgb(107, 114, 128);
const DARK_GRAY: Color = Color::Rgb(55, 55, 55);

// Unicode block characters for bar rendering
const BAR_FULL: char = '█';
const BAR_EMPTY: char = '░';

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
        View::Details => draw_details_overlay(f, app),
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
        View::Dashboard | View::Confirm | View::Loading | View::Details => {
            draw_daemon_table(f, area, app)
        }
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

    // Build header with sort indicator
    let header_columns = [
        ("Name", Some(SortColumn::Name)),
        ("PID", None),
        ("Status", Some(SortColumn::Status)),
        ("CPU", Some(SortColumn::Cpu)),
        ("Mem", Some(SortColumn::Memory)),
        ("Uptime", Some(SortColumn::Uptime)),
        ("Error", None),
    ];
    let header_cells = header_columns.iter().map(|(name, sort_col)| {
        let text = if *sort_col == Some(app.sort_column) {
            format!("{} {}", name, app.sort_order.indicator())
        } else {
            (*name).to_string()
        };
        Cell::from(text).style(Style::default().fg(ORANGE).bold())
    });
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

        // CPU bar (5 chars wide)
        let cpu_cell = stats
            .map(|s| Cell::from(render_bar(s.cpu_percent, 5)))
            .unwrap_or_else(|| Cell::from("-").style(Style::default().fg(GRAY)));

        // Memory bar (5 chars wide)
        let mem_cell = stats
            .map(|s| Cell::from(render_memory_bar(s.memory_bytes, 5)))
            .unwrap_or_else(|| Cell::from("-").style(Style::default().fg(GRAY)));

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
            cpu_cell,
            mem_cell,
            Cell::from(uptime).style(Style::default().fg(GRAY)),
            Cell::from(error).style(Style::default().fg(RED)),
        ])
        .style(row_style)
        .height(1)
    });

    let widths = [
        Constraint::Percentage(18), // Name
        Constraint::Length(8),      // PID
        Constraint::Length(10),     // Status
        Constraint::Length(11),     // CPU bar
        Constraint::Length(12),     // Mem bar
        Constraint::Length(10),     // Uptime
        Constraint::Percentage(20), // Error
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

    // Render scrollbar if there are more items than visible
    let visible_rows = table_area.height.saturating_sub(3) as usize; // -3 for borders and header
    if filtered.len() > visible_rows {
        let mut scrollbar_state = ScrollbarState::new(filtered.len()).position(app.selected);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .style(Style::default().fg(GRAY));
        f.render_stateful_widget(
            scrollbar,
            table_area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
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

/// Render a usage bar with percentage and visual indicator
fn render_bar(percent: f32, width: usize) -> Line<'static> {
    let clamped = percent.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);

    // Color based on usage level
    let bar_color = if clamped >= 90.0 {
        RED
    } else if clamped >= 70.0 {
        ORANGE
    } else if clamped >= 50.0 {
        YELLOW
    } else {
        GREEN
    };

    let filled_str: String = std::iter::repeat_n(BAR_FULL, filled).collect();
    let empty_str: String = std::iter::repeat_n(BAR_EMPTY, empty).collect();
    let pct_str = format!("{:>3.0}%", clamped);

    Line::from(vec![
        Span::styled(filled_str, Style::default().fg(bar_color)),
        Span::styled(empty_str, Style::default().fg(DARK_GRAY)),
        Span::raw(" "),
        Span::styled(pct_str, Style::default().fg(GRAY)),
    ])
}

/// Render memory bar with size display
fn render_memory_bar(bytes: u64, width: usize) -> Line<'static> {
    // Estimate percentage - assume 8GB max for coloring purposes
    let max_bytes: u64 = 8 * 1024 * 1024 * 1024; // 8GB
    let percent = ((bytes as f64 / max_bytes as f64) * 100.0) as f32;
    let clamped = percent.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);

    // Color based on usage level
    let bar_color = if bytes > 2 * 1024 * 1024 * 1024 {
        RED // > 2GB
    } else if bytes > 1024 * 1024 * 1024 {
        ORANGE // > 1GB
    } else if bytes > 512 * 1024 * 1024 {
        YELLOW // > 512MB
    } else {
        GREEN
    };

    let filled_str: String = std::iter::repeat_n(BAR_FULL, filled).collect();
    let empty_str: String = std::iter::repeat_n(BAR_EMPTY, empty).collect();

    // Format memory size
    let size_str = if bytes < 1024 * 1024 {
        format!("{:.0}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.0}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    };

    Line::from(vec![
        Span::styled(filled_str, Style::default().fg(bar_color)),
        Span::styled(empty_str, Style::default().fg(DARK_GRAY)),
        Span::raw(" "),
        Span::styled(format!("{:>5}", size_str), Style::default().fg(GRAY)),
    ])
}

/// Draw the daemon details view (charts + logs)
fn draw_logs(f: &mut Frame, area: Rect, app: &App) {
    let daemon_id = app.log_daemon_id.as_deref().unwrap_or("unknown");

    // Layout: daemon info (3) | charts (8) | current stats (3) | search bar (optional 3) | logs (rest)
    let search_height = if app.log_search_active || !app.log_search_query.is_empty() {
        3
    } else {
        0
    };

    if app.logs_expanded {
        // Expanded mode: just header + search + logs
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),             // Daemon header
                Constraint::Length(search_height), // Search bar (if active)
                Constraint::Min(5),                // Logs (fills remaining space)
            ])
            .split(area);

        draw_daemon_header_compact(f, chunks[0], app, daemon_id);

        let logs_area = if search_height > 0 {
            draw_log_search_bar(f, chunks[1], app);
            chunks[2]
        } else {
            chunks[2]
        };

        draw_log_panel(f, logs_area, app, daemon_id);
    } else {
        // Normal mode: header + charts + stats + search + logs
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),             // Daemon header
                Constraint::Length(8),             // Charts
                Constraint::Length(3),             // Current stats
                Constraint::Length(search_height), // Search bar (if active)
                Constraint::Min(5),                // Logs
            ])
            .split(area);

        draw_daemon_header(f, chunks[0], app, daemon_id);
        draw_charts(f, chunks[1], app, daemon_id);
        draw_current_stats(f, chunks[2], app, daemon_id);

        let logs_area = if search_height > 0 {
            draw_log_search_bar(f, chunks[3], app);
            chunks[4]
        } else {
            chunks[4]
        };

        draw_log_panel(f, logs_area, app, daemon_id);
    }
}

/// Draw compact daemon header (for expanded logs mode)
fn draw_daemon_header_compact(f: &mut Frame, area: Rect, app: &App, daemon_id: &str) {
    let daemon = app.daemons.iter().find(|d| d.id == daemon_id);

    let mut spans = vec![Span::styled(daemon_id, Style::default().fg(ORANGE).bold())];

    if let Some(d) = daemon {
        let (status_text, status_color) = status_display(&d.status);
        spans.push(Span::raw("  "));
        spans.push(Span::styled(status_text, Style::default().fg(status_color)));

        if let Some(pid) = d.pid {
            if let Some(stats) = app.get_stats(pid) {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!(
                        "CPU: {:.1}%  Mem: {}",
                        stats.cpu_percent,
                        stats.memory_display()
                    ),
                    Style::default().fg(GRAY),
                ));
            }
        }
    }

    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        "[expanded]",
        Style::default().fg(DARK_GRAY).italic(),
    ));

    let header = Paragraph::new(Line::from(spans))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(DARK_GRAY)),
        );
    f.render_widget(header, area);
}

/// Draw daemon header with name and status
fn draw_daemon_header(f: &mut Frame, area: Rect, app: &App, daemon_id: &str) {
    let daemon = app.daemons.iter().find(|d| d.id == daemon_id);

    let mut spans = vec![Span::styled(daemon_id, Style::default().fg(ORANGE).bold())];

    if let Some(d) = daemon {
        let (status_text, status_color) = status_display(&d.status);
        spans.push(Span::raw("  "));
        spans.push(Span::styled(status_text, Style::default().fg(status_color)));

        if let Some(pid) = d.pid {
            spans.push(Span::raw("  "));
            spans.push(Span::styled("PID: ", Style::default().fg(GRAY)));
            spans.push(Span::styled(
                pid.to_string(),
                Style::default().fg(Color::White),
            ));
        }

        if let Some(pid) = d.pid {
            if let Some(stats) = app.get_stats(pid) {
                spans.push(Span::raw("  "));
                spans.push(Span::styled("Uptime: ", Style::default().fg(GRAY)));
                spans.push(Span::styled(
                    stats.uptime_display(),
                    Style::default().fg(Color::White),
                ));
            }
        }
    }

    if app.is_disabled(daemon_id) {
        spans.push(Span::raw("  "));
        spans.push(Span::styled("[DISABLED]", Style::default().fg(RED).bold()));
    }

    let header = Paragraph::new(Line::from(spans))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(DARK_GRAY)),
        );
    f.render_widget(header, area);
}

/// Draw resource usage charts (CPU, Memory, Disk I/O)
fn draw_charts(f: &mut Frame, area: Rect, app: &App, daemon_id: &str) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(area);

    let history = app.get_stats_history(daemon_id);

    draw_cpu_chart(f, chunks[0], history);
    draw_memory_chart(f, chunks[1], history);
    draw_disk_chart(f, chunks[2], history);
}

/// Draw CPU usage chart
fn draw_cpu_chart(f: &mut Frame, area: Rect, history: Option<&StatsHistory>) {
    let values = history.map(|h| h.cpu_values()).unwrap_or_default();
    let current = values.last().copied().unwrap_or(0.0);
    let color = cpu_color(current);

    // Convert to (x, y) data points for the chart
    let data: Vec<(f64, f64)> = values
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64, v as f64))
        .collect();

    let datasets = vec![Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color))
        .data(&data)];

    let x_max = data.len().max(1) as f64;

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .title(format!(" CPU {:.1}% ", current))
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DARK_GRAY)),
        )
        .x_axis(
            Axis::default()
                .bounds([0.0, x_max])
                .style(Style::default().fg(DARK_GRAY)),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, 100.0])
                .labels(vec![Line::from("0"), Line::from("50"), Line::from("100")])
                .style(Style::default().fg(DARK_GRAY)),
        );

    f.render_widget(chart, area);
}

/// Draw memory usage chart
fn draw_memory_chart(f: &mut Frame, area: Rect, history: Option<&StatsHistory>) {
    let values = history.map(|h| h.memory_values()).unwrap_or_default();
    let current = values.last().copied().unwrap_or(0);
    let max_val = values.iter().copied().max().unwrap_or(1).max(1) as f64;
    let color = memory_color(current);

    // Convert to (x, y) data points for the chart (in MB for readability)
    let data: Vec<(f64, f64)> = values
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64, v as f64 / (1024.0 * 1024.0)))
        .collect();

    let datasets = vec![Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color))
        .data(&data)];

    let x_max = data.len().max(1) as f64;
    let y_max = (max_val / (1024.0 * 1024.0)).max(1.0);

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .title(format!(" Mem {} ", format_memory(current)))
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DARK_GRAY)),
        )
        .x_axis(
            Axis::default()
                .bounds([0.0, x_max])
                .style(Style::default().fg(DARK_GRAY)),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, y_max])
                .labels(vec![
                    Line::from("0"),
                    Line::from(format!("{}M", (y_max / 2.0) as u64)),
                    Line::from(format!("{}M", y_max as u64)),
                ])
                .style(Style::default().fg(DARK_GRAY)),
        );

    f.render_widget(chart, area);
}

/// Draw disk I/O chart (read and write as separate lines)
fn draw_disk_chart(f: &mut Frame, area: Rect, history: Option<&StatsHistory>) {
    let read_values = history.map(|h| h.disk_read_values()).unwrap_or_default();
    let write_values = history.map(|h| h.disk_write_values()).unwrap_or_default();

    let current_read = read_values.last().copied().unwrap_or(0);
    let current_write = write_values.last().copied().unwrap_or(0);

    // Convert to (x, y) data points for the chart (in KB/s)
    let read_data: Vec<(f64, f64)> = read_values
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64, v as f64 / 1024.0))
        .collect();

    let write_data: Vec<(f64, f64)> = write_values
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64, v as f64 / 1024.0))
        .collect();

    let max_read = read_values.iter().copied().max().unwrap_or(1) as f64 / 1024.0;
    let max_write = write_values.iter().copied().max().unwrap_or(1) as f64 / 1024.0;
    let y_max = max_read.max(max_write).max(1.0);

    let datasets = vec![
        Dataset::default()
            .name("R")
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(GREEN))
            .data(&read_data),
        Dataset::default()
            .name("W")
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(YELLOW))
            .data(&write_data),
    ];

    let x_max = read_data.len().max(write_data.len()).max(1) as f64;

    // Build title with current rates
    let title = format!(
        " Disk R:{} W:{} ",
        format_rate(current_read),
        format_rate(current_write)
    );

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .title(title)
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DARK_GRAY)),
        )
        .x_axis(
            Axis::default()
                .bounds([0.0, x_max])
                .style(Style::default().fg(DARK_GRAY)),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, y_max])
                .labels(vec![
                    Line::from("0"),
                    Line::from(format!("{}K", (y_max / 2.0) as u64)),
                    Line::from(format!("{}K", y_max as u64)),
                ])
                .style(Style::default().fg(DARK_GRAY)),
        );

    f.render_widget(chart, area);
}

/// Draw current stats summary
fn draw_current_stats(f: &mut Frame, area: Rect, app: &App, daemon_id: &str) {
    let daemon = app.daemons.iter().find(|d| d.id == daemon_id);
    let stats = daemon
        .and_then(|d| d.pid)
        .and_then(|pid| app.get_stats(pid));

    let content = if let Some(stats) = stats {
        Line::from(vec![
            Span::styled("CPU: ", Style::default().fg(GRAY)),
            Span::styled(
                format!("{:.1}%", stats.cpu_percent),
                Style::default().fg(cpu_color(stats.cpu_percent)),
            ),
            Span::raw("   "),
            Span::styled("Memory: ", Style::default().fg(GRAY)),
            Span::styled(
                stats.memory_display(),
                Style::default().fg(memory_color(stats.memory_bytes)),
            ),
            Span::raw("   "),
            Span::styled("Disk Read: ", Style::default().fg(GRAY)),
            Span::styled(stats.disk_read_display(), Style::default().fg(GREEN)),
            Span::raw("   "),
            Span::styled("Disk Write: ", Style::default().fg(GRAY)),
            Span::styled(stats.disk_write_display(), Style::default().fg(YELLOW)),
        ])
    } else {
        Line::from(vec![Span::styled(
            "No process stats available",
            Style::default().fg(GRAY).italic(),
        )])
    };

    let paragraph = Paragraph::new(content).alignment(Alignment::Center);
    f.render_widget(paragraph, area);
}

/// Draw the logs panel
fn draw_log_panel(f: &mut Frame, area: Rect, app: &App, daemon_id: &str) {
    let follow_indicator = if app.log_follow { " [follow]" } else { "" };
    let search_indicator = if !app.log_search_matches.is_empty() {
        format!(
            " [{}/{}]",
            app.log_search_current + 1,
            app.log_search_matches.len()
        )
    } else {
        String::new()
    };
    let title = format!(
        " Logs: {}{}{} ",
        daemon_id, follow_indicator, search_indicator
    );

    let visible_height = area.height.saturating_sub(2) as usize;
    let visible_lines: Vec<Line> = app
        .log_content
        .iter()
        .enumerate()
        .skip(app.log_scroll)
        .take(visible_height)
        .map(|(line_idx, line)| highlight_log_line(line, line_idx, app))
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

    // Render scrollbar if there are more lines than visible
    let total_lines = app.log_content.len();
    if total_lines > visible_height {
        let mut scrollbar_state = ScrollbarState::new(total_lines.saturating_sub(visible_height))
            .position(app.log_scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .style(Style::default().fg(GRAY));
        f.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

/// Get color for CPU usage
fn cpu_color(percent: f32) -> Color {
    if percent >= 90.0 {
        RED
    } else if percent >= 70.0 {
        ORANGE
    } else if percent >= 50.0 {
        YELLOW
    } else {
        GREEN
    }
}

/// Get color for memory usage
fn memory_color(bytes: u64) -> Color {
    if bytes > 2 * 1024 * 1024 * 1024 {
        RED // > 2GB
    } else if bytes > 1024 * 1024 * 1024 {
        ORANGE // > 1GB
    } else if bytes > 512 * 1024 * 1024 {
        YELLOW // > 512MB
    } else {
        GREEN
    }
}

/// Format memory in human-readable form
fn format_memory(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Format bytes per second rate
fn format_rate(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B/s", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K/s", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M/s", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G/s", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn draw_log_search_bar(f: &mut Frame, area: Rect, app: &App) {
    let search_text = if app.log_search_active {
        format!("/{}_", app.log_search_query)
    } else {
        format!("/{}", app.log_search_query)
    };

    let match_info = if !app.log_search_matches.is_empty() {
        format!(" ({} matches)", app.log_search_matches.len())
    } else if !app.log_search_query.is_empty() {
        " (no matches)".to_string()
    } else {
        String::new()
    };

    let search_bar = Paragraph::new(format!("{}{}", search_text, match_info))
        .style(if app.log_search_active {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(GRAY)
        })
        .block(
            Block::default()
                .title(" Search Logs ")
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(if app.log_search_active {
                    Style::default().fg(ORANGE)
                } else {
                    Style::default().fg(GRAY)
                }),
        );
    f.render_widget(search_bar, area);
}

/// Highlight a log line with syntax coloring and search match highlighting
fn highlight_log_line(line: &str, line_idx: usize, app: &App) -> Line<'static> {
    let is_match = app.log_search_matches.contains(&line_idx);
    let is_current_match = app
        .log_search_matches
        .get(app.log_search_current)
        .map(|&idx| idx == line_idx)
        .unwrap_or(false);

    // Determine base style based on log level
    let line_lower = line.to_lowercase();
    let base_style = if line_lower.contains("error")
        || line_lower.contains("fatal")
        || line_lower.contains("panic")
    {
        Style::default().fg(RED)
    } else if line_lower.contains("warn") {
        Style::default().fg(YELLOW)
    } else if line_lower.contains("debug") || line_lower.contains("trace") {
        Style::default().fg(DARK_GRAY)
    } else {
        Style::default().fg(Color::White)
    };

    // Apply search highlight
    let style = if is_current_match {
        base_style.bg(Color::Rgb(100, 60, 0)) // Orange-ish background for current match
    } else if is_match {
        base_style.bg(Color::Rgb(50, 40, 0)) // Dim yellow background for other matches
    } else {
        base_style
    };

    // Highlight timestamps (common patterns like 2024-01-15 or HH:MM:SS)
    let mut spans = Vec::new();

    // Simple timestamp detection at start of line (use char-based iteration for UTF-8 safety)
    let chars: Vec<char> = line.chars().collect();
    if chars.len() >= 10 {
        let potential_date: String = chars[..10].iter().collect();
        if potential_date.chars().filter(|c| *c == '-').count() == 2
            && potential_date
                .chars()
                .filter(|c| c.is_ascii_digit())
                .count()
                == 8
        {
            spans.push(Span::styled(potential_date, Style::default().fg(GRAY)));
            let remaining: String = chars[10..].iter().collect();
            if !remaining.is_empty() {
                spans.push(Span::styled(remaining, style));
            }
        } else {
            spans.push(Span::styled(line.to_string(), style));
        }
    } else {
        spans.push(Span::styled(line.to_string(), style));
    }

    Line::from(spans)
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
            "/:search  q/Esc:clear  j/k:nav  S:sort  i:info  s:start  x:stop  l:details  ?:help"
        }
        View::Dashboard => {
            "/:search  q:quit  j/k:nav  S:sort  o:order  i:info  s:start  x:stop  l:details  ?:help"
        }
        View::Logs if app.log_search_active => "Type to search  Enter:finish  Esc:clear",
        View::Logs if !app.log_search_query.is_empty() => {
            "/:search  n/N:next/prev  q/Esc:back  Ctrl+D/U:page  f:follow  e:expand"
        }
        View::Logs if app.logs_expanded => {
            "/:search  q/Esc:back  j/k:scroll  Ctrl+D/U:page  f:follow  e:collapse  g/G:top/btm"
        }
        View::Logs => {
            "/:search  q/Esc:back  j/k:scroll  Ctrl+D/U:page  f:follow  e:expand  g/G:top/btm"
        }
        View::Help => "q/Esc/?:close",
        View::Confirm => "y/Enter:confirm  n/Esc:cancel",
        View::Loading => "Please wait...",
        View::Details => "q/Esc/i:close",
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
        Line::from("  l / Enter   View daemon details (charts + logs)"),
        Line::from("  i           Quick daemon info popup"),
        Line::from("  /           Search/filter daemons"),
        Line::from("  S           Cycle sort column"),
        Line::from("  o           Toggle sort order"),
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
            "Details View",
            Style::default().fg(RED).bold(),
        )]),
        Line::from("  j / k       Scroll logs up/down"),
        Line::from("  Ctrl+D/U    Page down/up"),
        Line::from("  /           Search in logs"),
        Line::from("  n / N       Next/prev match"),
        Line::from("  f           Toggle follow mode"),
        Line::from("  e           Expand/collapse logs"),
        Line::from("  g / G       Go to top/bottom"),
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

fn draw_details_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(70, 80, f.area());

    // Clear the background
    f.render_widget(Clear, area);

    let daemon_id = app.details_daemon_id.as_deref().unwrap_or("unknown");

    // Get daemon info
    let daemon = app.daemons.iter().find(|d| d.id == daemon_id);
    let config = PitchforkToml::all_merged();
    let daemon_config = config.daemons.get(daemon_id);

    let mut lines = vec![
        Line::from(vec![Span::styled(
            daemon_id,
            Style::default().fg(ORANGE).bold(),
        )]),
        Line::from(""),
    ];

    // Status info
    if let Some(d) = daemon {
        lines.push(Line::from(vec![
            Span::styled("Status: ", Style::default().fg(GRAY)),
            Span::styled(
                format!("{:?}", d.status),
                Style::default().fg(match &d.status {
                    crate::daemon_status::DaemonStatus::Running => GREEN,
                    crate::daemon_status::DaemonStatus::Stopped => GRAY,
                    crate::daemon_status::DaemonStatus::Waiting => YELLOW,
                    crate::daemon_status::DaemonStatus::Stopping => YELLOW,
                    _ => RED,
                }),
            ),
        ]));

        if let Some(pid) = d.pid {
            lines.push(Line::from(vec![
                Span::styled("PID: ", Style::default().fg(GRAY)),
                Span::styled(pid.to_string(), Style::default().fg(Color::White)),
            ]));

            if let Some(stats) = app.get_stats(pid) {
                lines.push(Line::from(vec![
                    Span::styled("CPU: ", Style::default().fg(GRAY)),
                    Span::styled(stats.cpu_display(), Style::default().fg(Color::White)),
                    Span::raw("  "),
                    Span::styled("Memory: ", Style::default().fg(GRAY)),
                    Span::styled(stats.memory_display(), Style::default().fg(Color::White)),
                    Span::raw("  "),
                    Span::styled("Uptime: ", Style::default().fg(GRAY)),
                    Span::styled(stats.uptime_display(), Style::default().fg(Color::White)),
                ]));
            }
        }

        if let Some(err) = d.status.error_message() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("Error: ", Style::default().fg(RED)),
                Span::styled(err, Style::default().fg(RED)),
            ]));
        }
    }

    // Config info
    if let Some(cfg) = daemon_config {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Configuration",
            Style::default().fg(RED).bold(),
        )]));

        lines.push(Line::from(vec![
            Span::styled("Command: ", Style::default().fg(GRAY)),
            Span::styled(cfg.run.clone(), Style::default().fg(Color::White)),
        ]));

        if let Some(cron) = &cfg.cron {
            lines.push(Line::from(vec![
                Span::styled("Cron: ", Style::default().fg(GRAY)),
                Span::styled(&cron.schedule, Style::default().fg(Color::White)),
                Span::raw(" (retrigger: "),
                Span::styled(
                    format!("{:?}", cron.retrigger),
                    Style::default().fg(Color::White),
                ),
                Span::raw(")"),
            ]));
        }

        if cfg.retry > 0 {
            lines.push(Line::from(vec![
                Span::styled("Retry: ", Style::default().fg(GRAY)),
                Span::styled(cfg.retry.to_string(), Style::default().fg(Color::White)),
                Span::raw(" attempts"),
            ]));
        }

        if let Some(delay) = cfg.ready_delay {
            lines.push(Line::from(vec![
                Span::styled("Ready delay: ", Style::default().fg(GRAY)),
                Span::styled(format!("{}s", delay), Style::default().fg(Color::White)),
            ]));
        }

        if let Some(output) = &cfg.ready_output {
            lines.push(Line::from(vec![
                Span::styled("Ready output: ", Style::default().fg(GRAY)),
                Span::styled(output.clone(), Style::default().fg(Color::White)),
            ]));
        }

        if let Some(http) = &cfg.ready_http {
            lines.push(Line::from(vec![
                Span::styled("Ready HTTP: ", Style::default().fg(GRAY)),
                Span::styled(http.clone(), Style::default().fg(Color::White)),
            ]));
        }

        if cfg.boot_start.unwrap_or(false) {
            lines.push(Line::from(vec![
                Span::styled("Boot start: ", Style::default().fg(GRAY)),
                Span::styled("enabled", Style::default().fg(GREEN)),
            ]));
        }
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "No configuration found in pitchfork.toml",
            Style::default().fg(GRAY).italic(),
        )]));
    }

    // Disabled status
    if app.is_disabled(daemon_id) {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "This daemon is DISABLED",
            Style::default().fg(RED).bold(),
        )]));
    }

    let details = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Daemon Details ")
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RED)),
        )
        .style(Style::default().bg(Color::Rgb(20, 20, 20)));

    f.render_widget(details, area);
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
