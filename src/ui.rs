use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table},
    Frame,
};
use crate::app::App;
use bytesize::ByteSize;

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.size());

    render_table(f, app, chunks[0]);
    render_status(f, app, chunks[1]);
}

fn render_table(f: &mut Frame, app: &mut App, area: Rect) {
    let header_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let selected_style = Style::default().add_modifier(Modifier::REVERSED);

    let header_cells = ["PID", "Name", "Physical", "Compressed", "Swap", "Total"]
        .iter()
        .map(|h| Cell::from(*h).style(header_style));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows = app.processes.iter().map(|item| {
        let cells = vec![
            Cell::from(item.pid.to_string()),
            Cell::from(item.name.clone()),
            Cell::from(ByteSize(item.physical_footprint).to_string()),
            Cell::from(ByteSize(item.compressed).to_string()),
            Cell::from(ByteSize(item.swap_used).to_string()),
            Cell::from(ByteSize(item.total()).to_string()),
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
        .constraints([
            Constraint::Percentage(70),
            Constraint::Percentage(30),
        ])
        .split(area);

    // Render Gauge
    if let Some((current, total)) = app.scan_progress {
        let percent = if total > 0 {
            ((current as f64 / total as f64) * 100.0) as u16
        } else {
            0
        };
        
        let label = if let Some(name) = &app.current_scanning {
            format!("Scanning {}/{} - {}", current, total, name)
        } else {
            format!("Scanning {}/{}", current, total)
        };

        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Progress"))
            .gauge_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::ITALIC))
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
    let summary = format!(
        "Total Swap: {} | Total Comp: {} | Total Phys: {}",
        ByteSize(app.total_swap),
        ByteSize(app.total_compressed),
        ByteSize(app.total_phys)
    );
    
    // Note: Total Swap is the sum of logical swap per process, which can be larger than disk swap usage due to sharing.
    let status = format!("{} | 'r': Refresh | 'q': Quit | 's': Swap | 't': Total | 'x': Kill", summary);

    let p = Paragraph::new(status)
        .style(Style::default().fg(Color::White))
        .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(p, area);
}
