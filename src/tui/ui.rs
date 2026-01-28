use crate::daemon_status::DaemonStatus;
use crate::pitchfork_toml::{CronRetrigger, PitchforkToml, PitchforkTomlAuto};
use crate::tui::app::{
    App, EditMode, FormFieldValue, PendingAction, SortColumn, StatsHistory, View,
};
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
const CYAN: Color = Color::Rgb(34, 211, 238); // #22d3ee - for available/config-only daemons

// Unicode block characters for bar rendering
const BAR_FULL: char = '█';
const BAR_EMPTY: char = '░';

/// UTF-8 safe string truncation from the end, returning "...{suffix}" if too long.
/// Uses character count instead of byte length to avoid panics on non-ASCII.
fn truncate_path_end(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let suffix_len = max_chars.saturating_sub(3); // Account for "..."
        let suffix: String = s.chars().skip(char_count - suffix_len).collect();
        format!("...{suffix}")
    }
}

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
        View::ConfigEditor => draw_config_editor_overlay(f, app),
        View::ConfigFileSelect => draw_file_select_overlay(f, app),
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
    let (total, running, stopped, errored, available) = app.stats();

    let mut spans = vec![
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
    ];

    // Show available count if there are config-only daemons
    if available > 0 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled("Available: ", Style::default().fg(CYAN)));
        spans.push(Span::styled(
            available.to_string(),
            Style::default().fg(CYAN).bold(),
        ));
    }

    let stats = Line::from(spans);
    let stats_widget = Paragraph::new(stats).alignment(Alignment::Center);
    f.render_widget(stats_widget, area);
}

fn draw_main(f: &mut Frame, area: Rect, app: &App) {
    match app.view {
        View::Dashboard
        | View::Confirm
        | View::Loading
        | View::Details
        | View::ConfigEditor
        | View::ConfigFileSelect => draw_daemon_table(f, area, app),
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

    // Build header with sort indicator (include checkbox column if multi-select is active)
    let show_checkbox = app.has_selection();
    let header_columns = [
        ("Name", Some(SortColumn::Name)),
        ("PID", None),
        ("Status", Some(SortColumn::Status)),
        ("CPU", Some(SortColumn::Cpu)),
        ("Mem", Some(SortColumn::Memory)),
        ("Uptime", Some(SortColumn::Uptime)),
        ("Error", None),
    ];
    let mut header_cells: Vec<Cell> = if show_checkbox {
        vec![Cell::from("☐").style(Style::default().fg(ORANGE).bold())]
    } else {
        vec![]
    };
    header_cells.extend(header_columns.iter().map(|(name, sort_col)| {
        let text = if *sort_col == Some(app.sort_column) {
            format!("{} {}", name, app.sort_order.indicator())
        } else {
            (*name).to_string()
        };
        Cell::from(text).style(Style::default().fg(ORANGE).bold())
    }));
    let header = Row::new(header_cells).height(1);

    let rows = filtered.iter().enumerate().map(|(i, daemon)| {
        let cursor_here = i == app.selected;
        let is_multi_selected = app.is_selected(&daemon.id);
        let disabled = app.is_disabled(&daemon.id);
        let is_config_only = app.is_config_only(&daemon.id);

        let name_style = if is_config_only {
            Style::default().fg(CYAN).italic() // Cyan for available/config-only
        } else if disabled {
            Style::default().fg(GRAY).italic()
        } else if cursor_here {
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

        // Show "available" for config-only daemons instead of "stopped"
        let (status_text, status_color) = if is_config_only {
            ("available".to_string(), CYAN)
        } else {
            status_display(&daemon.status)
        };

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

        let row_style = if is_multi_selected {
            Style::default().bg(Color::Rgb(40, 40, 20)) // Yellow-ish for multi-select
        } else if cursor_here {
            Style::default().bg(Color::Rgb(50, 20, 20))
        } else {
            Style::default()
        };

        // Build row cells
        let mut cells = vec![];
        if show_checkbox {
            let checkbox = if is_multi_selected { "☑" } else { "☐" };
            let checkbox_style = if is_multi_selected {
                Style::default().fg(GREEN)
            } else {
                Style::default().fg(GRAY)
            };
            cells.push(Cell::from(checkbox).style(checkbox_style));
        }
        cells.extend(vec![
            Cell::from(name).style(name_style),
            Cell::from(pid),
            Cell::from(status_text).style(Style::default().fg(status_color)),
            cpu_cell,
            mem_cell,
            Cell::from(uptime).style(Style::default().fg(GRAY)),
            Cell::from(error).style(Style::default().fg(RED)),
        ]);

        Row::new(cells).style(row_style).height(1)
    });

    let widths: Vec<Constraint> = if show_checkbox {
        vec![
            Constraint::Length(2),      // Checkbox
            Constraint::Percentage(18), // Name
            Constraint::Length(8),      // PID
            Constraint::Length(10),     // Status
            Constraint::Length(11),     // CPU bar
            Constraint::Length(12),     // Mem bar
            Constraint::Length(10),     // Uptime
            Constraint::Percentage(18), // Error (slightly smaller to fit checkbox)
        ]
    } else {
        vec![
            Constraint::Percentage(18), // Name
            Constraint::Length(8),      // PID
            Constraint::Length(10),     // Status
            Constraint::Length(11),     // CPU bar
            Constraint::Length(12),     // Mem bar
            Constraint::Length(10),     // Uptime
            Constraint::Percentage(20), // Error
        ]
    };

    let selection_count = app.multi_select.len();
    let title = if selection_count > 0 {
        format!(" Daemons ({selection_count} selected) ")
    } else if !app.search_query.is_empty() {
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
        DaemonStatus::Errored(code) => (format!("errored ({code})"), RED),
        DaemonStatus::ErroredUnknown => ("errored".to_string(), RED),
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
    let pct_str = format!("{clamped:>3.0}%");

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
        Span::styled(format!("{size_str:>5}"), Style::default().fg(GRAY)),
    ])
}

/// Draw the daemon details view (charts + logs)
fn draw_logs(f: &mut Frame, area: Rect, app: &App) {
    let daemon_id = app.log_daemon_id.as_deref().unwrap_or("unknown");

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
        // Normal mode: stats panel on left, charts on right, logs below
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10),            // Stats + Charts row
                Constraint::Length(search_height), // Search bar (if active)
                Constraint::Min(5),                // Logs
            ])
            .split(area);

        // Split top row: stats panel (left) | charts (right)
        let top_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(24), // Stats panel (fixed width)
                Constraint::Min(30),    // Charts (fills remaining)
            ])
            .split(chunks[0]);

        draw_stats_panel(f, top_chunks[0], app, daemon_id);
        draw_charts(f, top_chunks[1], app, daemon_id);

        let logs_area = if search_height > 0 {
            draw_log_search_bar(f, chunks[1], app);
            chunks[2]
        } else {
            chunks[2]
        };

        draw_log_panel(f, logs_area, app, daemon_id);
    }
}

/// Draw stats panel on the left side of details view
fn draw_stats_panel(f: &mut Frame, area: Rect, app: &App, daemon_id: &str) {
    let daemon = app.daemons.iter().find(|d| d.id == daemon_id);
    let stats = daemon
        .and_then(|d| d.pid)
        .and_then(|pid| app.get_stats(pid));

    let mut lines = vec![
        Line::from(vec![Span::styled(
            daemon_id,
            Style::default().fg(ORANGE).bold(),
        )]),
        Line::from(""),
    ];

    // Status
    if let Some(d) = daemon {
        let (status_text, status_color) = status_display(&d.status);
        lines.push(Line::from(vec![
            Span::styled("Status: ", Style::default().fg(GRAY)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ]));

        if let Some(pid) = d.pid {
            lines.push(Line::from(vec![
                Span::styled("PID:    ", Style::default().fg(GRAY)),
                Span::styled(pid.to_string(), Style::default().fg(Color::White)),
            ]));
        }
    }

    // Stats from process
    if let Some(stats) = stats {
        lines.push(Line::from(vec![
            Span::styled("Uptime: ", Style::default().fg(GRAY)),
            Span::styled(stats.uptime_display(), Style::default().fg(Color::White)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("CPU:    ", Style::default().fg(GRAY)),
            Span::styled(
                format!("{:.1}%", stats.cpu_percent),
                Style::default().fg(cpu_color(stats.cpu_percent)),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Memory: ", Style::default().fg(GRAY)),
            Span::styled(
                stats.memory_display(),
                Style::default().fg(memory_color(stats.memory_bytes)),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Disk R: ", Style::default().fg(GRAY)),
            Span::styled(stats.disk_read_display(), Style::default().fg(GREEN)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Disk W: ", Style::default().fg(GRAY)),
            Span::styled(stats.disk_write_display(), Style::default().fg(YELLOW)),
        ]));
    }

    // Disabled indicator
    if app.is_disabled(daemon_id) {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "DISABLED",
            Style::default().fg(RED).bold(),
        )]));
    }

    let panel = Paragraph::new(lines).block(
        Block::default()
            .title(" Info ")
            .title_style(Style::default().fg(ORANGE).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DARK_GRAY)),
    );
    f.render_widget(panel, area);
}

/// Draw compact daemon header (for expanded logs mode)
fn draw_daemon_header_compact(f: &mut Frame, area: Rect, app: &App, daemon_id: &str) {
    let daemon = app.daemons.iter().find(|d| d.id == daemon_id);

    let mut spans = vec![Span::styled(daemon_id, Style::default().fg(ORANGE).bold())];

    if let Some(d) = daemon {
        let (status_text, status_color) = status_display(&d.status);
        spans.push(Span::raw("  "));
        spans.push(Span::styled(status_text, Style::default().fg(status_color)));

        if let Some(pid) = d.pid
            && let Some(stats) = app.get_stats(pid)
        {
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

    let datasets = vec![
        Dataset::default()
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(color))
            .data(&data),
    ];

    let x_max = data.len().max(1) as f64;

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .title(format!(" CPU {current:.1}% "))
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

    let datasets = vec![
        Dataset::default()
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(color))
            .data(&data),
    ];

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
    let title = format!(" Logs: {daemon_id}{follow_indicator}{search_indicator} ");

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
        format!("{bytes}B/s")
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

    let search_bar = Paragraph::new(format!("{search_text}{match_info}"))
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
        View::Dashboard if app.has_selection() => {
            "Space:toggle  Ctrl+A:all  c:clear  s:start  x:stop  r:restart  d:disable  e:enable"
        }
        View::Dashboard if !app.search_query.is_empty() => {
            "/:search  q/Esc:clear  j/k:nav  Space:select  s:start  a:toggle-avail  ?:help"
        }
        View::Dashboard => {
            "/:search  q/Esc:quit  j/k:nav  Space:select  s:start  a:toggle-avail  ?:help"
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
        View::ConfigEditor => "Tab/j/k:nav  Enter:edit  Ctrl+S:save  Esc:cancel  D:delete",
        View::ConfigFileSelect => "j/k:nav  Enter:select  Esc:cancel",
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
        Line::from("  a           Toggle available daemons"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Multi-select",
            Style::default().fg(RED).bold(),
        )]),
        Line::from("  Space       Toggle selection"),
        Line::from("  Ctrl+A      Select all visible"),
        Line::from("  c           Clear selection"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Actions",
            Style::default().fg(RED).bold(),
        )]),
        Line::from("  s           Start stopped daemon(s)"),
        Line::from("  x           Stop running daemon(s)"),
        Line::from("  r           Restart daemon(s)"),
        Line::from("  e           Enable disabled daemon(s)"),
        Line::from("  d           Disable daemon(s)"),
        Line::from("  R           Force refresh"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Config Editor",
            Style::default().fg(RED).bold(),
        )]),
        Line::from("  n           New daemon"),
        Line::from("  E           Edit selected daemon config"),
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

    let (action_text, target_text) = match &app.pending_action {
        Some(PendingAction::Stop(id)) => ("Stop", format!("daemon '{id}'")),
        Some(PendingAction::Restart(id)) => ("Restart", format!("daemon '{id}'")),
        Some(PendingAction::Disable(id)) => ("Disable", format!("daemon '{id}'")),
        Some(PendingAction::BatchStop(ids)) => ("Stop", format!("{} daemons", ids.len())),
        Some(PendingAction::BatchRestart(ids)) => ("Restart", format!("{} daemons", ids.len())),
        Some(PendingAction::BatchDisable(ids)) => ("Disable", format!("{} daemons", ids.len())),
        Some(PendingAction::DeleteDaemon { id, .. }) => {
            ("Delete", format!("daemon '{id}' from config"))
        }
        Some(PendingAction::DiscardEditorChanges) => ("Discard", "unsaved changes".to_string()),
        None => ("Unknown", "unknown".to_string()),
    };

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(action_text, Style::default().fg(ORANGE).bold()),
            Span::raw(" "),
            Span::styled(target_text, Style::default().fg(Color::White).bold()),
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

        if cfg.retry.count() > 0 {
            lines.push(Line::from(vec![
                Span::styled("Retry: ", Style::default().fg(GRAY)),
                Span::styled(cfg.retry.to_string(), Style::default().fg(Color::White)),
                if cfg.retry.is_infinite() {
                    Span::raw("")
                } else {
                    Span::raw(" attempts")
                },
            ]));
        }

        if let Some(delay) = cfg.ready_delay {
            lines.push(Line::from(vec![
                Span::styled("Ready delay: ", Style::default().fg(GRAY)),
                Span::styled(format!("{delay}s"), Style::default().fg(Color::White)),
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

fn draw_config_editor_overlay(f: &mut Frame, app: &App) {
    let editor = match &app.editor_state {
        Some(e) => e,
        None => return,
    };

    let area = centered_rect(70, 85, f.area());
    f.render_widget(Clear, area);

    let title = match &editor.mode {
        EditMode::Create => " New Daemon ",
        EditMode::Edit { .. } => " Edit Daemon ",
    };

    // Split into header and body
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Daemon ID
            Constraint::Length(1), // Config path
            Constraint::Min(0),    // Form fields
            Constraint::Length(2), // Footer
        ])
        .split(area);

    // Daemon ID header
    let id_style = if editor.daemon_id_editing {
        Style::default().fg(ORANGE).bold()
    } else {
        Style::default().fg(Color::White)
    };

    let id_display = if editor.daemon_id_editing {
        format!("Name: {}█", editor.daemon_id)
    } else if editor.daemon_id.is_empty() {
        "Name: (press 'i' to edit name)".to_string()
    } else {
        format!("Name: {}", editor.daemon_id)
    };

    // Append error if present
    let id_display = if let Some(err) = &editor.daemon_id_error {
        format!("{id_display} [{err}]")
    } else {
        id_display
    };

    let id_style = if editor.daemon_id_error.is_some() {
        Style::default().fg(RED).bold()
    } else {
        id_style
    };

    let header = Paragraph::new(id_display)
        .style(id_style)
        .block(
            Block::default()
                .title(title)
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RED)),
        )
        .style(Style::default().bg(Color::Rgb(20, 20, 20)));
    f.render_widget(header, chunks[0]);

    // Config path
    let path_str = editor.config_path.display().to_string();
    let path_display = truncate_path_end(&path_str, 60);
    let path_line = Paragraph::new(format!("  Config: {path_display}"))
        .style(Style::default().fg(GRAY).bg(Color::Rgb(20, 20, 20)));
    f.render_widget(path_line, chunks[1]);

    // Form fields
    let mut lines: Vec<Line> = Vec::new();

    for (i, field) in editor.fields.iter().enumerate() {
        let is_focused = i == editor.focused_field && !editor.daemon_id_editing;

        // Field label
        let focus_indicator = if is_focused { "▶ " } else { "  " };
        let required_marker = if field.required { "*" } else { "" };
        let label_style = if is_focused {
            Style::default().fg(ORANGE).bold()
        } else {
            Style::default().fg(GRAY)
        };

        lines.push(Line::from(vec![
            Span::styled(focus_indicator, Style::default().fg(ORANGE)),
            Span::styled(field.label, label_style),
            Span::styled(required_marker, Style::default().fg(RED)),
        ]));

        // Field value
        let value_line = render_field_value(field, is_focused && field.editing);
        lines.push(value_line);

        // Error message if any
        if let Some(error) = &field.error {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(format!("⚠ {error}"), Style::default().fg(RED)),
            ]));
        }

        // Add spacing between fields
        lines.push(Line::from(""));
    }

    let form = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Configuration ")
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DARK_GRAY)),
        )
        .style(Style::default().bg(Color::Rgb(20, 20, 20)));
    f.render_widget(form, chunks[2]);

    // Footer with keybindings
    let footer_text = if editor.is_editing() {
        "Enter: Confirm | Esc: Cancel editing"
    } else {
        match &editor.mode {
            EditMode::Create => {
                "Tab/j/k: Navigate | Enter: Edit | Space: Toggle | Ctrl+S: Save | q: Cancel"
            }
            EditMode::Edit { .. } => {
                "Tab/j/k: Navigate | Enter: Edit | Space: Toggle | Ctrl+S: Save | D: Delete | q: Cancel"
            }
        }
    };

    let footer = Paragraph::new(footer_text)
        .alignment(Alignment::Center)
        .style(Style::default().fg(GRAY).bg(Color::Rgb(20, 20, 20)));
    f.render_widget(footer, chunks[3]);
}

fn render_field_value(field: &crate::tui::app::FormField, is_editing: bool) -> Line<'static> {
    let cursor = if is_editing { "█" } else { "" };

    match &field.value {
        FormFieldValue::Text(s) => {
            let display = if s.is_empty() && !is_editing {
                Span::styled("(empty)", Style::default().fg(DARK_GRAY).italic())
            } else {
                Span::styled(format!("{s}{cursor}"), Style::default().fg(Color::White))
            };
            Line::from(vec![Span::raw("    "), display])
        }
        FormFieldValue::OptionalText(opt) => {
            let display = match opt {
                Some(s) => Span::styled(format!("{s}{cursor}"), Style::default().fg(Color::White)),
                None if is_editing => {
                    Span::styled(cursor.to_string(), Style::default().fg(Color::White))
                }
                None => Span::styled("(not set)", Style::default().fg(DARK_GRAY).italic()),
            };
            Line::from(vec![Span::raw("    "), display])
        }
        FormFieldValue::Number(n) => {
            let display = if is_editing {
                Span::styled(format!("{n}{cursor}"), Style::default().fg(Color::White))
            } else {
                Span::styled(n.to_string(), Style::default().fg(Color::White))
            };
            Line::from(vec![Span::raw("    "), display])
        }
        FormFieldValue::OptionalNumber(opt) => {
            let display = match opt {
                Some(n) => Span::styled(format!("{n}{cursor}"), Style::default().fg(Color::White)),
                None if is_editing => {
                    Span::styled(cursor.to_string(), Style::default().fg(Color::White))
                }
                None => Span::styled("(not set)", Style::default().fg(DARK_GRAY).italic()),
            };
            Line::from(vec![Span::raw("    "), display])
        }
        FormFieldValue::OptionalPort(opt) => {
            let display = match opt {
                Some(p) => Span::styled(format!("{p}{cursor}"), Style::default().fg(Color::White)),
                None if is_editing => {
                    Span::styled(cursor.to_string(), Style::default().fg(Color::White))
                }
                None => Span::styled("(not set)", Style::default().fg(DARK_GRAY).italic()),
            };
            Line::from(vec![Span::raw("    "), display])
        }
        FormFieldValue::Boolean(b) => {
            let checkbox = if *b { "[x]" } else { "[ ]" };
            let color = if *b { GREEN } else { GRAY };
            Line::from(vec![
                Span::raw("    "),
                Span::styled(checkbox, Style::default().fg(color)),
            ])
        }
        FormFieldValue::OptionalBoolean(opt) => {
            let (checkbox, color) = match opt {
                Some(true) => ("[x] Yes", GREEN),
                Some(false) => ("[ ] No", GRAY),
                None => ("[-] (not set)", DARK_GRAY),
            };
            Line::from(vec![
                Span::raw("    "),
                Span::styled(checkbox, Style::default().fg(color)),
            ])
        }
        FormFieldValue::AutoBehavior(v) => {
            let has_start = v.contains(&PitchforkTomlAuto::Start);
            let has_stop = v.contains(&PitchforkTomlAuto::Stop);

            let start_box = if has_start { "[x]" } else { "[ ]" };
            let stop_box = if has_stop { "[x]" } else { "[ ]" };

            Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    start_box,
                    Style::default().fg(if has_start { GREEN } else { GRAY }),
                ),
                Span::raw(" Start  "),
                Span::styled(
                    stop_box,
                    Style::default().fg(if has_stop { GREEN } else { GRAY }),
                ),
                Span::raw(" Stop"),
            ])
        }
        FormFieldValue::Retrigger(r) => {
            let options = [
                ("Finish", CronRetrigger::Finish),
                ("Always", CronRetrigger::Always),
                ("Success", CronRetrigger::Success),
                ("Fail", CronRetrigger::Fail),
            ];

            let mut spans = vec![Span::raw("    ")];
            for (name, val) in &options {
                let style = if r == val {
                    Style::default().fg(GREEN).bold()
                } else {
                    Style::default().fg(GRAY)
                };
                spans.push(Span::styled(format!("{name} "), style));
            }
            Line::from(spans)
        }
        FormFieldValue::StringList(v) => {
            let display = if v.is_empty() && !is_editing {
                Span::styled("(none)", Style::default().fg(DARK_GRAY).italic())
            } else {
                let text = v.join(", ");
                Span::styled(format!("{text}{cursor}"), Style::default().fg(Color::White))
            };
            Line::from(vec![Span::raw("    "), display])
        }
    }
}

fn draw_file_select_overlay(f: &mut Frame, app: &App) {
    let selector = match &app.file_selector {
        Some(s) => s,
        None => return,
    };

    let area = centered_rect(60, 50, f.area());
    f.render_widget(Clear, area);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![Span::styled(
            "Select a config file for the new daemon:",
            Style::default().fg(ORANGE),
        )]),
        Line::from(""),
    ];

    for (i, path) in selector.files.iter().enumerate() {
        let is_selected = i == selector.selected;
        let indicator = if is_selected { "▶ " } else { "  " };

        let path_str = path.display().to_string();
        let display_path = truncate_path_end(&path_str, 50);

        let exists_marker = if path.exists() { "" } else { " (new)" };

        let style = if is_selected {
            Style::default().fg(ORANGE).bold()
        } else {
            Style::default().fg(Color::White)
        };

        lines.push(Line::from(vec![
            Span::styled(indicator, Style::default().fg(ORANGE)),
            Span::styled(display_path, style),
            Span::styled(exists_marker, Style::default().fg(CYAN)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "j/k: Navigate | Enter: Select | q: Cancel",
        Style::default().fg(GRAY),
    )]));

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Select Config File ")
                .title_style(Style::default().fg(ORANGE).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RED)),
        )
        .style(Style::default().bg(Color::Rgb(20, 20, 20)));

    f.render_widget(popup, area);
}
