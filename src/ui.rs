use crate::app::App;
use crate::scanner::format_size;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table},
};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(5)])
        .split(f.size());

    render_table(f, app, chunks[0]);
    render_status(f, app, chunks[1]);
}

fn render_table(f: &mut Frame, app: &mut App, area: Rect) {
    let header_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let selected_style = Style::default().add_modifier(Modifier::REVERSED);

    let header_cells = ["PID", "Name", "Physical", "Compressed", "Swap", "Total"]
        .iter()
        .map(|h| Cell::from(*h).style(header_style));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows = app.processes.iter().map(|item| {
        let cells = vec![
            Cell::from(item.pid.to_string()),
            Cell::from(item.name.clone()),
            Cell::from(format_size(item.physical_footprint)),
            Cell::from(format_size(item.compressed)),
            Cell::from(format_size(item.swap_disk)),
            Cell::from(format_size(item.total())),
        ];
        Row::new(cells).height(1)
    });

    // Mark sorted column
    let widths = [
        Constraint::Length(8),
        Constraint::Percentage(30),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
    ];

    let t = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Processes"))
        .highlight_style(selected_style)
        .highlight_symbol(">> ");

    f.render_stateful_widget(t, area, &mut app.state);
}

fn render_status(f: &mut Frame, app: &mut App, area: Rect) {
    if app.is_loading {
        render_loading_status(f, app, area);
    } else {
        render_idle_status(f, app, area);
    }
}

fn render_loading_status(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(area);

    // Render Gauge
    if let Some((current, total)) = app.scan_progress {
        let percent = if total > 0 {
            ((current as f64 / total as f64) * 100.0) as u16
        } else {
            0
        };

        let label = if let Some(msg) = &app.status_message {
            msg.clone()
        } else {
             format!("Scanning {}/{}", current, total)
        };

        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Quick Scan Progress"))
            .gauge_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::ITALIC),
            )
            .percent(percent)
            .label(label);

        f.render_widget(gauge, chunks[0]);
    } else {
        let p = Paragraph::new("Initializing scan...")
            .block(Block::default().borders(Borders::ALL).title("Progress"));
        f.render_widget(p, chunks[0]);
    }

    // Render Spinner
    let spinner_char = SPINNER[app.spinner_idx % SPINNER.len()];
    let p = Paragraph::new(format!("{} Scanning...", spinner_char))
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(p, chunks[1]);
}

fn render_idle_status(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(35),
            Constraint::Percentage(45),
            Constraint::Percentage(20),
        ])
        .split(area);

    // 1. System Status
    let sys_swap_color = if app.system_swap_bytes > 0 {
        Color::Red
    } else {
        Color::Green
    };

    let info_text = vec![
        Line::from(vec![
            Span::raw("System Swap: "),
            Span::styled(
                app.system_swap.clone(),
                Style::default()
                    .fg(sys_swap_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("Total Swap:  "),
            Span::styled(
                format_size(app.total_swap),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::raw("Total Phys:  "),
            Span::styled(
                format_size(app.total_phys),
                Style::default().fg(Color::Cyan),
            ),
        ]),
    ];

    let info_block = Paragraph::new(info_text)
        .block(Block::default().borders(Borders::ALL).title("System Status"));
    f.render_widget(info_block, chunks[0]);

    // 2. Controls
    let keys_style = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::Gray);

    let controls_text = vec![
        Line::from(vec![
            Span::styled("R", keys_style),
            Span::styled("efresh ", desc_style),
            Span::styled("Q", keys_style),
            Span::styled("uit ", desc_style),
            Span::styled("X", keys_style),
            Span::styled("Kill Selected", desc_style),
        ]),
        Line::from(vec![
            Span::raw("Sort: "),
            Span::styled("S", keys_style),
            Span::styled("wap ", desc_style),
            Span::styled("P", keys_style),
            Span::styled("hys ", desc_style),
            Span::styled("C", keys_style),
            Span::styled("omp ", desc_style),
        ]),
        Line::from(vec![
            Span::raw("      "), // indentation
            Span::styled("T", keys_style),
            Span::styled("otal ", desc_style),
            Span::styled("N", keys_style),
            Span::styled("ame ", desc_style),
            Span::styled("I", keys_style),
            Span::styled("PID ", desc_style),
        ]),
    ];

    let controls_block = Paragraph::new(controls_text)
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(controls_block, chunks[1]);

    // 3. Deep scan progress or Idle indicator
    if let Some((current, total)) = app.deep_scan_progress {
        let percent = if total > 0 {
            ((current as f64 / total as f64) * 100.0) as u16
        } else {
            0
        };
        let spinner_char = SPINNER[app.spinner_idx % SPINNER.len()];
        let label = format!("{} Deep Analysis: {}%", spinner_char, percent);

        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Deep Analysis"))
            .gauge_style(
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::ITALIC),
            )
            .percent(percent)
            .label(label);
        f.render_widget(gauge, chunks[2]);
    } else {
        let p = Paragraph::new("Idle")
            .style(Style::default().fg(Color::Green))
            .block(Block::default().borders(Borders::ALL).title("Analysis"));
        f.render_widget(p, chunks[2]);
    }
}
