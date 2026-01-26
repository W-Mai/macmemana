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
    collections::HashMap,
    fs::File,
};

use anyhow::Result;
use app::{App, ProcessDetails, SortColumn};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use scanner::{ProcessMemory, format_size};
use sysinfo::{System, Pid};

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
    BatchUpdate(HashMap<i32, (crate::footprint::FootprintData, u64)>), // Batch update with footprint & compressed
    DeepScanStart(usize),    // Total items for deep scan
    DeepScanProgress(usize), // Current items completed
    Complete,                // Done
    SingleResult(ProcessMemory), // Single process refresh result
    DetailResult(ProcessDetails), // Single process detail result
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.cli {
        run_cli(args)?;
    } else {
        // Redirect stderr to /dev/null to prevent TUI corruption
        if let Ok(file) = File::create("/dev/null") {
            // Use libc to dup2 file descriptor to stderr (fd 2)
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
            }
        }
        
        // Setup panic hook to restore terminal
        let original_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
            original_hook(panic_info);
        }));

        run_tui()?;
    }

    Ok(())
}



fn run_cli(args: Args) -> Result<()> {
    // CLI mode logic (kept simple for now, using optimized full scan if possible or fallback)
    // For CLI, we might want to stick to the "all at once" approach or implement wait-for-deep-scan.
    // Let's keep it blocking for CLI as user expects a report.
    // But we need to use the new logic if we want consistency.
    // Re-implementing a blocking version of the 2-stage scan for CLI:
    
    let mut out = io::stdout().lock();
    writeln!(out, "Scanning processes...")?;

    // 1. Quick scan
    let mut sys = System::new_all();
    sys.refresh_all();
    let mut processes: Vec<ProcessMemory> = sys
        .processes()
        .iter()
        .map(|(pid, process)| {
            ProcessMemory::new_simple(
                pid.as_u32() as i32,
                process.name().to_string(),
                process.memory(),
            )
        })
        .collect();

    // 2. Deep scan
    writeln!(out, "Performing deep analysis on {} processes...", processes.len())?;
    
    // Get compressed map first
    let compressed_map = crate::top::get_all_processes_compressed().unwrap_or_default();
    
    let pids: Vec<i32> = processes.iter().map(|p| p.pid).collect();
    let chunk_size = 50;
    
    // Process chunks
    for chunk in pids.chunks(chunk_size) {
        if let Ok(fp_map) = crate::footprint::get_footprint_for_pids(chunk) {
             for p in &mut processes {
                 if let Some(data) = fp_map.get(&p.pid) {
                     let compressed = *compressed_map.get(&p.pid).unwrap_or(&0);
                     p.merge_footprint(data, compressed);
                 }
             }
        }
    }
    
    // Filter empty
    processes.retain(|p| p.total() > 0);

    let system_swap_str = get_system_swap().unwrap_or_else(|| String::from("0B"));
    let system_swap_bytes = scanner::parse_size(&system_swap_str);
    
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
        if e.kind() == io::ErrorKind::BrokenPipe { return Ok(()); }
        return Err(e.into());
    }

    if let Err(e) = writeln!(out, "{}", "-".repeat(100)) {
        if e.kind() == io::ErrorKind::BrokenPipe { return Ok(()); }
        return Err(e.into());
    }

    for p in &processes {
        if (p.swap_disk > 0 || p.physical_footprint > 100 * 1024 * 1024)
            && let Err(e) = writeln!(
                out,
                "{:<8} {:<30} {:<15} {:<15} {:<15}",
                p.pid,
                truncate(&p.name, 30),
                format_size(p.physical_footprint),
                format_size(p.swap_disk),
                format_size(p.total())
            )
        {
            if e.kind() == io::ErrorKind::BrokenPipe { return Ok(()); }
            return Err(e.into());
        }
    }
    if let Err(e) = writeln!(
        out,
        "System Swap Used: {} | Swap Sum: {} | Delta: {}",
        system_swap_str,
        format_size(sum_swap_bytes),
        format_size(sum_swap_bytes.abs_diff(system_swap_bytes))
    ) {
        if e.kind() == io::ErrorKind::BrokenPipe { return Ok(()); }
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
    let output = Command::new("sysctl").arg("vm.swapusage").output().ok()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(used_part) = stdout.split("used = ").nth(1)
            && let Some(val) = used_part.split_whitespace().next()
        {
            return Some(val.to_string());
        }
    }
    None
}

fn run_tui() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    let (tx, rx) = mpsc::channel();
    let tx_scan = tx.clone();

    thread::spawn(move || {
        perform_scan(tx_scan);
    });
    
    app.is_loading = true;
    if let Some(s) = get_system_swap() {
        app.system_swap = s;
        app.system_swap_bytes = scanner::parse_size(&app.system_swap);
    }

    let tick_rate = Duration::from_millis(100);
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
                    if app.detail_view_open {
                        app.detail_view_open = false;
                    } else {
                        app.quit();
                    }
                }
                KeyCode::Esc => {
                    if app.detail_view_open {
                        app.detail_view_open = false;
                    }
                }
                KeyCode::Char('r') => {
                    if app.detail_view_open {
                         // Refresh details if open
                         if let Some(detail) = &app.current_detail {
                            let pid = detail.pid;
                            let name = detail.name.clone();
                            let tx = tx.clone();
                            app.is_loading = true; // reusing spinner
                            thread::spawn(move || {
                                fetch_details(pid, name, tx);
                            });
                         }
                    } else if !app.is_loading && app.deep_scan_progress.is_none() {
                        app.is_loading = true;
                        app.processes.clear();
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
                KeyCode::Char('R') => {
                     // Single process refresh
                     if let Some(i) = app.state.selected()
                        && let Some(p) = app.processes.get(i)
                     {
                        let pid = p.pid;
                        let name = p.name.clone();
                        let tx = tx.clone();
                        // Don't set full loading state, maybe just a spinner message?
                        // app.status_message = Some(format!("Refreshing {}...", name));
                        // Actually, let's just do it.
                        thread::spawn(move || {
                            if let Ok(pm) = scanner::get_process_memory(pid, &name) {
                                let _ = tx.send(EventWrapper::SingleResult(pm));
                            }
                        });
                     }
                }
                KeyCode::Enter => {
                    if !app.detail_view_open
                        && let Some(i) = app.state.selected()
                        && let Some(p) = app.processes.get(i)
                    {
                        app.detail_view_open = true;
                        app.current_detail = None;
                        let pid = p.pid;
                        let name = p.name.clone();
                        let tx = tx.clone();
                        thread::spawn(move || {
                            fetch_details(pid, name, tx);
                        });
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !app.detail_view_open {
                        app.next();
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if !app.detail_view_open {
                        app.previous();
                    }
                }
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
                    app.sort_desc = false;
                    app.sort();
                }
                KeyCode::Char('i') => {
                    app.sort_column = SortColumn::Pid;
                    app.sort_desc = false;
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

        while let Ok(event) = rx.try_recv() {
            match event {
                EventWrapper::Start(total) => {
                    app.scan_progress = Some((0, total));
                    app.status_message = Some("Quick scanning...".to_string());
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
                EventWrapper::DeepScanStart(total) => {
                    app.is_loading = false; // Initial loading done
                    app.scan_progress = None;
                    app.deep_scan_progress = Some((0, total));
                    app.status_message = Some(format!("Deep scanning 0/{}...", total));
                }
                EventWrapper::DeepScanProgress(current) => {
                    if let Some((_, total)) = app.deep_scan_progress {
                        app.deep_scan_progress = Some((current, total));
                        app.status_message = Some(format!("Deep scanning {}/{}...", current, total));
                    }
                }
                EventWrapper::BatchUpdate(updates) => {
                    app.update_processes(updates);
                }
                EventWrapper::Complete => {
                    app.is_loading = false;
                    app.scan_progress = None;
                    app.deep_scan_progress = None;
                    app.current_scanning = None;
                    app.status_message = Some("Scan complete".to_string());
                    app.normalize_swap_to_system();
                    app.sort();
                    if app.state.selected().is_none() && !app.processes.is_empty() {
                        app.state.select(Some(0));
                    }
                }
                EventWrapper::SingleResult(pm) => {
                    // Find and update
                    if let Some(idx) = app.processes.iter().position(|p| p.pid == pm.pid) {
                        app.processes[idx] = pm;
                        app.recalculate_totals();
                        app.normalize_swap_to_system();
                    }
                }
                EventWrapper::DetailResult(details) => {
                    app.is_loading = false;
                    app.current_detail = Some(details);
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

fn fetch_details(pid: i32, name: String, tx: mpsc::Sender<EventWrapper>) {
    // 1. Sysinfo for general info
    let mut sys = System::new();
    let sys_pid = Pid::from(pid as usize);
    sys.refresh_process(sys_pid);
    
    if let Some(proc) = sys.process(sys_pid) {
        let cmd = proc.cmd().to_vec();
        let exe = proc.exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let cwd = proc.cwd().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        // let _environ = proc.environ().to_vec();
        let status = proc.status().to_string();
        let start_time = proc.start_time();
        let cpu_usage = proc.cpu_usage();
        
        // 2. Memory info
        let memory_info = if let Ok(pm) = scanner::get_process_memory(pid, &name) {
            pm
        } else {
             ProcessMemory::new_simple(pid, name.clone(), proc.memory())
        };
        
        let details = ProcessDetails {
            pid,
            name,
            cmd,
            exe,
            cwd,
            // environ,
            status,
            start_time,
            cpu_usage,
            memory_info,
        };
        
        let _ = tx.send(EventWrapper::DetailResult(details));
    }
}

fn perform_scan(tx: mpsc::Sender<EventWrapper>) {
    // 1. Fast Path: Sysinfo
    let mut sys = System::new_all();
    sys.refresh_all();
    
    let pids: Vec<(i32, String, u64)> = sys
        .processes()
        .iter()
        .map(|(pid, process)| (pid.as_u32() as i32, process.name().to_string(), process.memory()))
        .collect();

    let total = pids.len();
    let _ = tx.send(EventWrapper::Start(total));
    
    // Send initial results immediately
    for (i, (pid, name, rss)) in pids.iter().enumerate() {
        let _ = tx.send(EventWrapper::Progress(i + 1, name.clone()));
        let p = ProcessMemory::new_simple(*pid, name.clone(), *rss);
        let _ = tx.send(EventWrapper::Result(p));
    }
    
    // 2. Deep Path: Footprint + Top
    let _ = tx.send(EventWrapper::DeepScanStart(total));
    
    // Fetch compressed memory map once
    let compressed_map = crate::top::get_all_processes_compressed().unwrap_or_default();
    
    let pid_list: Vec<i32> = pids.iter().map(|(pid, _, _)| *pid).collect();
    let chunk_size = 30; // Batch size
    let mut processed_count = 0;
    
    for chunk in pid_list.chunks(chunk_size) {
        if let Ok(fp_map) = crate::footprint::get_footprint_for_pids(chunk) {
            let mut updates = HashMap::new();
            for pid in chunk {
                if let Some(data) = fp_map.get(pid) {
                    let compressed = *compressed_map.get(pid).unwrap_or(&0);
                    updates.insert(*pid, (data.clone(), compressed));
                }
            }
            if !updates.is_empty() {
                let _ = tx.send(EventWrapper::BatchUpdate(updates));
            }
        }
        
        processed_count += chunk.len();
        let _ = tx.send(EventWrapper::DeepScanProgress(processed_count));
    }

    let _ = tx.send(EventWrapper::Complete);
}

fn normalize_process_swaps(processes: &mut [ProcessMemory], system_swap_bytes: u64) {
    // Same logic as before
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
