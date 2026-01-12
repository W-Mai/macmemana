mod app;
mod scanner;
mod ui;

use std::{
    io,
    io::Write,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use app::{App, SortColumn};
use bytesize::ByteSize;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use rayon::prelude::*;
use scanner::{ProcessMemory, get_process_memory};
use sysinfo::System;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Run in CLI mode (dump output once and exit)
    #[arg(long)]
    cli: bool,

    /// Sort by column (swapped, phys)
    #[arg(long, default_value = "swapped")]
    sort: String,
}

enum EventWrapper {
    Start(usize),            // Total processes
    Progress(usize, String), // Current count, current scanning name
    Result(ProcessMemory),   // Incremental result
    Complete,                // Done
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
    let mut out = io::stdout().lock();
    if let Err(e) = writeln!(out, "Scanning processes... This may take a while.") {
        if e.kind() == io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(e.into());
    }

    // For CLI, we need a simple blocking scan, but maybe re-use logic?
    // Let's just inline a simple scan here since perform_scan is async/channel based.
    let mut sys = System::new_all();
    sys.refresh_all();
    let pids: Vec<(i32, String)> = sys
        .processes()
        .iter()
        .map(|(pid, process)| (pid.as_u32() as i32, process.name().to_string()))
        .collect();

    if let Err(e) = writeln!(out, "Found {} processes. Starting scan...", pids.len()) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(e.into());
    }

    let mut processes: Vec<ProcessMemory> = pids
        .par_iter()
        .map(|(pid, name)| {
            get_process_memory(*pid, name).unwrap_or_else(|_| ProcessMemory {
                pid: *pid,
                name: name.clone(),
                physical_footprint: 0,
                swapped: 0,
            })
        })
        .filter(|p| p.total() > 0)
        .collect();

    // Sort
    match args.sort.as_str() {
        "phys" => processes.sort_by(|a, b| b.physical_footprint.cmp(&a.physical_footprint)),
        _ => processes.sort_by(|a, b| b.swapped.cmp(&a.swapped)),
    }

    if let Err(e) = writeln!(
        out,
        "{:<8} {:<30} {:<15} {:<15} {:<15}",
        "PID", "Name", "Physical", "Swap", "Total"
    ) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(e.into());
    }

    if let Err(e) = writeln!(out, "{}", "-".repeat(100)) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(e.into());
    }

    for p in processes {
        if p.swapped > 0 || p.physical_footprint > 100 * 1024 * 1024 {
            // Show if swapped > 0 or phys > 100MB
            if let Err(e) = writeln!(
                out,
                "{:<8} {:<30} {:<15} {:<15} {:<15}",
                p.pid,
                truncate(&p.name, 30),
                ByteSize(p.physical_footprint),
                ByteSize(p.swapped),
                ByteSize(p.total())
            ) {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(e.into());
            }
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

use std::process::Command;

fn get_system_swap() -> Option<String> {
    // sysctl vm.swapusage: vm.swapusage: total = 3072.00M  used = 1398.75M  free = 1673.25M  (encrypted)
    let output = Command::new("sysctl").arg("vm.swapusage").output().ok()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse "used = X"
        // Regex or simple split
        if let Some(used_part) = stdout.split("used = ").nth(1)
            && let Some(val) = used_part.split_whitespace().next()
        {
            return Some(val.to_string());
        }
    }
    None
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
    if let Some(s) = get_system_swap() {
        app.system_swap = s;
    }

    let tick_rate = Duration::from_millis(100); // Faster tick for smooth animation
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| ui::ui(f, &mut app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if crossterm::event::poll(timeout)?
            && let Event::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Char('q') => {
                    app.quit();
                }
                KeyCode::Char('r') => {
                    if !app.is_loading {
                        app.is_loading = true;
                        app.processes.clear(); // Clear existing list on refresh
                        app.total_swapped = 0;
                        app.total_phys = 0;
                        if let Some(s) = get_system_swap() {
                            app.system_swap = s;
                        }
                        let tx_scan = tx.clone();
                        thread::spawn(move || {
                            perform_scan(tx_scan);
                        });
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => app.next(),
                KeyCode::Up | KeyCode::Char('k') => app.previous(),
                KeyCode::Char('s') => {
                    app.sort_column = SortColumn::Swapped;
                    app.sort_desc = true;
                    app.sort();
                }
                KeyCode::Char('p') => {
                    app.sort_column = SortColumn::Physical;
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

        if last_tick.elapsed() >= tick_rate {
            app.on_tick();
            last_tick = Instant::now();
        }

        // Check for scan results
        if let Ok(results) = rx.try_recv() {
            match results {
                EventWrapper::Start(total) => {
                    app.scan_progress = Some((0, total));
                    app.current_scanning = None;
                }
                EventWrapper::Progress(current, name) => {
                    if let Some((_, total)) = app.scan_progress {
                        app.scan_progress = Some((current, total));
                    }
                    app.current_scanning = Some(name);
                }
                EventWrapper::Result(process) => {
                    app.add_process(process);
                }
                EventWrapper::Complete => {
                    app.is_loading = false;
                    app.scan_progress = None;
                    app.current_scanning = None;
                    // Final sort to be sure
                    app.sort();
                    if app.state.selected().is_none() && !app.processes.is_empty() {
                        app.state.select(Some(0));
                    }
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

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn perform_scan(tx: mpsc::Sender<EventWrapper>) {
    let mut sys = System::new_all();
    sys.refresh_all();

    // Get all PIDs
    let pids: Vec<(i32, String)> = sys
        .processes()
        .iter()
        .map(|(pid, process)| (pid.as_u32() as i32, process.name().to_string()))
        .collect();

    let total = pids.len();
    let _ = tx.send(EventWrapper::Start(total));

    let counter = Arc::new(AtomicUsize::new(0));

    // Use rayon to parallelize vmmap calls
    pids.par_iter().for_each(|(pid, name)| {
        let current = counter.fetch_add(1, Ordering::Relaxed) + 1;
        // Send progress update periodically or every time?
        // Every time might overwhelm the channel/UI thread, let's try every 1 or 5.
        // For smoother UI, every 1 is fine if main loop drains quickly.
        let _ = tx.send(EventWrapper::Progress(current, name.clone()));

        if let Ok(mem) = get_process_memory(*pid, name)
            && mem.total() > 0
        {
            let _ = tx.send(EventWrapper::Result(mem));
        }
    });

    let _ = tx.send(EventWrapper::Complete);
}
