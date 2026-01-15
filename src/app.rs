use crate::scanner::ProcessMemory;
use ratatui::widgets::TableState;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortColumn {
    Pid,
    Name,
    Physical,
    Compressed,
    Swap,
    Total,
}

pub struct App {
    pub should_quit: bool,
    pub processes: Vec<ProcessMemory>,
    pub state: TableState,
    pub sort_column: SortColumn,
    pub sort_desc: bool,
    pub is_loading: bool,
    pub scan_progress: Option<(usize, usize)>, // (current, total)
    pub deep_scan_progress: Option<(usize, usize)>, // (current, total)
    pub status_message: Option<String>,
    pub current_scanning: Option<String>,
    pub spinner_idx: usize,
    pub total_swap: u64,
    pub total_phys: u64,
    pub system_swap: String,
    pub system_swap_bytes: u64,
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            processes: Vec::new(),
            state: TableState::default(),
            sort_column: SortColumn::Swap,
            sort_desc: true,
            is_loading: false,
            scan_progress: None,
            deep_scan_progress: None,
            status_message: None,
            current_scanning: None,
            spinner_idx: 0,
            total_swap: 0,
            total_phys: 0,
            system_swap: String::from("Unknown"),
            system_swap_bytes: 0,
        }
    }

    pub fn on_tick(&mut self) {
        if self.is_loading || self.deep_scan_progress.is_some() {
            self.spinner_idx = (self.spinner_idx + 1) % 4; // 4 frames for basic spinner
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn next(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i >= self.processes.len().saturating_sub(1) {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    pub fn previous(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    self.processes.len().saturating_sub(1)
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    pub fn sort(&mut self) {
        self.processes.sort_by(|a, b| {
            let ordering = match self.sort_column {
                SortColumn::Pid => a.pid.cmp(&b.pid),
                SortColumn::Name => a.name.cmp(&b.name),
                SortColumn::Physical => a.physical_footprint.cmp(&b.physical_footprint),
                SortColumn::Compressed => a.compressed.cmp(&b.compressed),
                SortColumn::Swap => a.swap_disk.cmp(&b.swap_disk),
                SortColumn::Total => a.total().cmp(&b.total()),
            };
            if self.sort_desc {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }

    pub fn add_process(&mut self, process: ProcessMemory) {
        self.total_swap += process.swap_disk;
        self.total_phys += process.physical_footprint;
        self.processes.push(process);
        // Only sort occasionally or at end? No, sort immediately for now.
        self.sort();
    }
    
    pub fn update_processes(&mut self, updates: HashMap<i32, (crate::footprint::FootprintData, u64)>) {
        let mut updated = false;
        
        for p in &mut self.processes {
            if let Some((fp_data, compressed)) = updates.get(&p.pid) {
                p.merge_footprint(fp_data, *compressed);
                updated = true;
            }
        }
        
        if updated {
            // Do NOT normalize yet during streaming update, just sort.
            // Normalization happens only at the end to avoid skewing data with partial updates.
            self.recalculate_totals();
            self.sort();
        }
    }
    
    fn recalculate_totals(&mut self) {
        self.total_swap = 0;
        self.total_phys = 0;
        for p in &self.processes {
            self.total_swap = self.total_swap.saturating_add(p.swap_disk);
            self.total_phys = self.total_phys.saturating_add(p.physical_footprint);
        }
    }

    pub fn normalize_swap_to_system(&mut self) {
        let system = self.system_swap_bytes;
        if system == 0 || self.processes.is_empty() {
            return;
        }

        let mut windowserver_idx = None;
        for (i, p) in self.processes.iter().enumerate() {
            if p.name.contains("WindowServer") {
                windowserver_idx = Some(i);
                break;
            }
        }

        let ws_display = if let Some(i) = windowserver_idx {
            let ws_est = self.processes[i].swap_disk_est;
            let ws_display = ws_est.min(system);
            self.processes[i].swap_disk = ws_display;
            ws_display
        } else {
            0
        };

        let remaining = system.saturating_sub(ws_display);
        let mut sum_other_est: u64 = 0;
        for (i, p) in self.processes.iter().enumerate() {
            if Some(i) == windowserver_idx {
                continue;
            }
            sum_other_est = sum_other_est.saturating_add(p.swap_disk_est);
        }

        if sum_other_est == 0 {
            for (i, p) in self.processes.iter_mut().enumerate() {
                if Some(i) == windowserver_idx {
                    continue;
                }
                p.swap_disk = 0;
            }
            self.total_swap = system;
            return;
        }

        let factor = remaining as f64 / sum_other_est as f64;
        let mut sum_scaled: u64 = 0;
        let mut max_other_idx = None;
        let mut max_other_est = 0;
        for (i, p) in self.processes.iter_mut().enumerate() {
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

        let target_sum = system;
        let current_sum = ws_display.saturating_add(sum_scaled);
        if target_sum != current_sum {
            let diff = target_sum as i128 - current_sum as i128;
            if let Some(i) = max_other_idx.or(windowserver_idx) {
                if diff.is_negative() {
                    let sub = (-diff) as u64;
                    self.processes[i].swap_disk = self.processes[i].swap_disk.saturating_sub(sub);
                } else {
                    self.processes[i].swap_disk =
                        self.processes[i].swap_disk.saturating_add(diff as u64);
                }
            }
        }

        self.recalculate_totals();
    }

    pub fn kill_selected(&mut self) {
        if let Some(i) = self.state.selected()
            && let Some(p) = self.processes.get(i)
        {
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(p.pid.to_string())
                .output();
        }
    }
}
