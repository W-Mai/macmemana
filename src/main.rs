mod app;
mod footprint;
mod scanner;
mod top;
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
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use rayon::prelude::*;
use scanner::{ProcessMemory, format_size, get_process_memory};
use sysinfo::System;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Run in CLI mode (dump output once and exit)
    #[arg(long)]
    cli: bool,

    /// Sort by column (swap, phys)
    #[arg(long, default_value = "swap")]
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

    let mut processes = if let Ok(procs) = scanner::scan_all_processes_optimized() {
        if let Err(e) = writeln!(
            out,
            "Used optimized scan (footprint). Found {} processes.",
            procs.len()
        ) {
            if e.kind() == io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(e.into());
        }
        procs
    } else {
        // Fallback
        let mut sys = System::new_all();
        sys.refresh_all();
        let pids: Vec<(i32, String)> = sys
            .processes()
            .iter()
            .map(|(pid, process)| (pid.as_u32() as i32, process.name().to_string()))
            .collect();

        if let Err(e) = writeln!(
            out,
            "Optimized scan failed (need root?). Falling back to vmmap. Found {} processes.",
            pids.len()
        ) {
            if e.kind() == io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(e.into());
        }

        pids.par_iter()
            .map(|(pid, name)| {
                get_process_memory(*pid, name).unwrap_or_else(|_| ProcessMemory {
                    pid: *pid,
                    name: name.clone(),
                    physical_footprint: 0,
                    compressed: 0,
                    swapped_total: 0,
                    swap_disk_est: 0,
                    swap_disk: 0,
                })
            })
            .collect()
    };

    // Filter empty
    processes.retain(|p| p.total() > 0);

    let system_swap_str = get_system_swap().unwrap_or_else(|| String::from("0B"));
    let system_swap_bytes = scanner::parse_size(&system_swap_str);

    // Only normalize if we fell back?
    // Actually, optimized scan (footprint) also produces swap_disk_est = swapped - compressed.
    // Does it match system swap? Not necessarily.
    // If we trust footprint, maybe we skip normalization or keep it?
    // Normalization ensures sum(process swap) == system swap.
    // If footprint is accurate, maybe it matches?
    // Let's keep normalization for consistency unless we are sure.
    // But if footprint gives Swapped (Total) and Top gives Compressed, we derive Swap Disk.
    // This derived Swap Disk might still not sum up perfectly to system swap due to shared pages etc.
    // Normalization forces it to match system view. It's safer to keep it for "Disk Swap" column accuracy relative to system total.
    normalize_process_swaps(&mut processes, system_swap_bytes);

    let sum_swap_bytes: u64 = processes.iter().map(|p| p.swap_disk).sum();

    // Sort
    match args.sort.as_str() {
        "phys" => processes.sort_by(|a, b| b.physical_footprint.cmp(&a.physical_footprint)),
        _ => processes.sort_by(|a, b| b.swap_disk.cmp(&a.swap_disk)),
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

    for p in &processes {
        if p.swap_disk > 0 || p.physical_footprint > 100 * 1024 * 1024 {
            // Show if swapped > 0 or phys > 100MB
            if let Err(e) = writeln!(
                out,
                "{:<8} {:<30} {:<15} {:<15} {:<15}",
                p.pid,
                truncate(&p.name, 30),
                format_size(p.physical_footprint),
                format_size(p.swap_disk),
                format_size(p.total())
            ) {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(e.into());
            }
        }
    }
    if let Err(e) = writeln!(
        out,
        "System Swap Used: {} | Swap Sum: {} | Delta: {}",
        system_swap_str,
        format_size(sum_swap_bytes),
        format_size(sum_swap_bytes.abs_diff(system_swap_bytes))
    ) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(e.into());
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
        app.system_swap_bytes = scanner::parse_size(&app.system_swap);
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
                        app.total_swap = 0;
                        app.total_phys = 0;
                        if let Some(s) = get_system_swap() {
                            app.system_swap = s;
                            app.system_swap_bytes = scanner::parse_size(&app.system_swap);
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
                    app.normalize_swap_to_system();
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
    // Try optimized scan (footprint + top)
    if let Ok(processes) = scanner::scan_all_processes_optimized() {
        let total = processes.len();
        let _ = tx.send(EventWrapper::Start(total));
        for (i, p) in processes.into_iter().enumerate() {
            let _ = tx.send(EventWrapper::Progress(i + 1, p.name.clone()));
            if p.total() > 0 {
                let _ = tx.send(EventWrapper::Result(p));
            }
        }
        let _ = tx.send(EventWrapper::Complete);
        return;
    }

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

fn normalize_process_swaps(processes: &mut [ProcessMemory], system_swap_bytes: u64) {
    if system_swap_bytes == 0 || processes.is_empty() {
        return;
    }

    let mut windowserver_idx = None;
    for (i, p) in processes.iter().enumerate() {
        if p.name.contains("WindowServer") {
            windowserver_idx = Some(i);
            break;
        }
    }

    let ws_display = if let Some(i) = windowserver_idx {
        let ws_est = processes[i].swap_disk_est;
        let ws_display = ws_est.min(system_swap_bytes);
        processes[i].swap_disk = ws_display;
        ws_display
    } else {
        0
    };

    let remaining = system_swap_bytes.saturating_sub(ws_display);
    let mut sum_other_est: u64 = 0;
    for (i, p) in processes.iter().enumerate() {
        if Some(i) == windowserver_idx {
            continue;
        }
        sum_other_est = sum_other_est.saturating_add(p.swap_disk_est);
    }

    if sum_other_est == 0 {
        for (i, p) in processes.iter_mut().enumerate() {
            if Some(i) == windowserver_idx {
                continue;
            }
            p.swap_disk = 0;
        }
        return;
    }

    let factor = remaining as f64 / sum_other_est as f64;
    let mut sum_scaled: u64 = 0;
    let mut max_other_idx = None;
    let mut max_other_est = 0;
    for (i, p) in processes.iter_mut().enumerate() {
        if Some(i) == windowserver_idx {
            continue;
        }
        if p.swap_disk_est > max_other_est {
            max_other_est = p.swap_disk_est;
            max_other_idx = Some(i);
        }
        let scaled = ((p.swap_disk_est as f64) * factor).round() as u64;
        p.swap_disk = scaled;
        sum_scaled = sum_scaled.saturating_add(scaled);
    }

    let current_sum = ws_display.saturating_add(sum_scaled);
    if current_sum != system_swap_bytes {
        let diff = system_swap_bytes as i128 - current_sum as i128;
        if let Some(i) = max_other_idx.or(windowserver_idx) {
            if diff.is_negative() {
                let sub = (-diff) as u64;
                processes[i].swap_disk = processes[i].swap_disk.saturating_sub(sub);
            } else {
                processes[i].swap_disk = processes[i].swap_disk.saturating_add(diff as u64);
            }
        }
    }
}
