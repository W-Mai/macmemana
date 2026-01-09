mod app;
mod scanner;
mod ui;

use std::{
    io,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use app::{App, SortColumn};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use scanner::{get_process_memory, ProcessMemory};
use sysinfo::System;
use rayon::prelude::*;
use bytesize::ByteSize;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Run in CLI mode (dump output once and exit)
    #[arg(long)]
    cli: bool,

    /// Sort by column (swap, phys, comp)
    #[arg(long, default_value = "swap")]
    sort: String,
}

enum EventWrapper {
    ScanStart(usize), // Total processes
    ScanProgress(usize, String), // Current count, current scanning name
    ScanComplete(Vec<ProcessMemory>),
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.cli {
        run_cli(args)?;
    } else {
        run_tui()?;
    }

    Ok(())
}

fn run_cli(args: Args) -> Result<()> {
    println!("Scanning processes... This may take a while.");
    
    // For CLI, we need a simple blocking scan, but maybe re-use logic?
    // Let's just inline a simple scan here since perform_scan is async/channel based.
    let mut sys = System::new_all();
    sys.refresh_all();
    let pids: Vec<(i32, String)> = sys.processes()
        .iter()
        .map(|(pid, process)| (pid.as_u32() as i32, process.name().to_string()))
        .collect();

    println!("Found {} processes. Starting scan...", pids.len());

    let mut processes: Vec<ProcessMemory> = pids
        .par_iter()
        .map(|(pid, name)| {
            get_process_memory(*pid, name).unwrap_or_else(|_| ProcessMemory {
                pid: *pid,
                name: name.clone(),
                physical_footprint: 0,
                compressed: 0,
                swap_used: 0,
            })
        })
        .filter(|p| p.total() > 0)
        .collect();

    // Sort
    match args.sort.as_str() {
        "phys" => processes.sort_by(|a, b| b.physical_footprint.cmp(&a.physical_footprint)),
        "comp" => processes.sort_by(|a, b| b.compressed.cmp(&a.compressed)),
        _ => processes.sort_by(|a, b| b.swap_used.cmp(&a.swap_used)),
    }

    println!("{:<8} {:<30} {:<15} {:<15} {:<15} {:<15}", "PID", "Name", "Physical", "Compressed", "Swap", "Total");
    println!("{}", "-".repeat(100));

    for p in processes {
        if p.swap_used > 0 || p.physical_footprint > 100 * 1024 * 1024 { // Show if swap > 0 or phys > 100MB
            println!(
                "{:<8} {:<30} {:<15} {:<15} {:<15} {:<15}",
                p.pid,
                truncate(&p.name, 30),
                ByteSize(p.physical_footprint),
                ByteSize(p.compressed),
                ByteSize(p.swap_used),
                ByteSize(p.total())
            );
        }
    }

    Ok(())
}

fn truncate(s: &str, max_width: usize) -> String {
    if s.len() > max_width {
        format!("{}...", &s[0..max_width - 3])
    } else {
        s.to_string()
    }
}

fn run_tui() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = App::new();

    // Setup communication channel
    let (tx, rx) = mpsc::channel();
    let tx_scan = tx.clone();

    // Start initial scan in background
    thread::spawn(move || {
        perform_scan(tx_scan);
    });
    app.is_loading = true;

    let tick_rate = Duration::from_millis(100); // Faster tick for smooth animation
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| ui::ui(f, &mut app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => {
                        app.quit();
                    }
                    KeyCode::Char('r') => {
                        if !app.is_loading {
                            app.is_loading = true;
                            let tx_scan = tx.clone();
                            thread::spawn(move || {
                                perform_scan(tx_scan);
                            });
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => app.next(),
                    KeyCode::Up | KeyCode::Char('k') => app.previous(),
                    KeyCode::Char('s') => {
                        app.sort_column = SortColumn::Swap;
                        app.sort_desc = true;
                        app.sort();
                    }
                    KeyCode::Char('p') => {
                        app.sort_column = SortColumn::Physical;
                        app.sort_desc = true;
                        app.sort();
                    }
                    KeyCode::Char('c') => {
                        app.sort_column = SortColumn::Compressed;
                        app.sort_desc = true;
                        app.sort();
                    }
                    KeyCode::Char('t') => {
                        app.sort_column = SortColumn::Total;
                        app.sort_desc = true;
                        app.sort();
                    }
                    KeyCode::Char('n') => {
                        app.sort_column = SortColumn::Name;
                        app.sort_desc = false; // Name usually asc
                        app.sort();
                    }
                    KeyCode::Char('i') => {
                        app.sort_column = SortColumn::Pid;
                        app.sort_desc = false; // PID usually asc
                        app.sort();
                    }
                    KeyCode::Char('x') => {
                        app.kill_selected();
                    }
                    _ => {}
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.on_tick();
            last_tick = Instant::now();
        }

        // Check for scan results
        if let Ok(results) = rx.try_recv() {
            match results {
                EventWrapper::ScanStart(total) => {
                    app.scan_progress = Some((0, total));
                    app.current_scanning = None;
                }
                EventWrapper::ScanProgress(current, name) => {
                    if let Some((_, total)) = app.scan_progress {
                        app.scan_progress = Some((current, total));
                    }
                    app.current_scanning = Some(name);
                }
                EventWrapper::ScanComplete(data) => {
                    app.set_processes(data);
                    app.is_loading = false;
                    app.scan_progress = None;
                    app.current_scanning = None;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn perform_scan(tx: mpsc::Sender<EventWrapper>) {
    let mut sys = System::new_all();
    sys.refresh_all();

    // Get all PIDs
    let pids: Vec<(i32, String)> = sys.processes()
        .iter()
        .map(|(pid, process)| (pid.as_u32() as i32, process.name().to_string()))
        .collect();

    let total = pids.len();
    let _ = tx.send(EventWrapper::ScanStart(total));

    let counter = Arc::new(AtomicUsize::new(0));

    // Use rayon to parallelize vmmap calls
    let results: Vec<ProcessMemory> = pids
        .par_iter()
        .map(|(pid, name)| {
            let current = counter.fetch_add(1, Ordering::Relaxed) + 1;
            // Send progress update periodically or every time?
            // Every time might overwhelm the channel/UI thread, let's try every 1 or 5.
            // For smoother UI, every 1 is fine if main loop drains quickly.
            let _ = tx.send(EventWrapper::ScanProgress(current, name.clone()));

            get_process_memory(*pid, name).unwrap_or_else(|_| ProcessMemory {
                pid: *pid,
                name: name.clone(),
                physical_footprint: 0,
                compressed: 0,
                swap_used: 0,
            })
        })
        .filter(|p| p.total() > 0) // Filter out empty/failed ones
        .collect();

    let _ = tx.send(EventWrapper::ScanComplete(results));
}
