use ratatui::widgets::TableState;
use crate::scanner::ProcessMemory;

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
    pub current_scanning: Option<String>,
    pub spinner_idx: usize,
    pub total_swap: u64,
    pub total_compressed: u64,
    pub total_phys: u64,
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            processes: Vec::new(),
            state: TableState::default(),
            sort_column: SortColumn::Swap, // Default sort by Swap as requested
            sort_desc: true,
            is_loading: false,
            scan_progress: None,
            current_scanning: None,
            spinner_idx: 0,
            total_swap: 0,
            total_compressed: 0,
            total_phys: 0,
        }
    }

    pub fn on_tick(&mut self) {
        if self.is_loading {
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
                SortColumn::Swap => a.swap_used.cmp(&b.swap_used),
                SortColumn::Total => a.total().cmp(&b.total()),
            };
            if self.sort_desc {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }

    pub fn set_processes(&mut self, processes: Vec<ProcessMemory>) {
        self.processes = processes;
        self.total_swap = self.processes.iter().map(|p| p.swap_used).sum();
        self.total_compressed = self.processes.iter().map(|p| p.compressed).sum();
        self.total_phys = self.processes.iter().map(|p| p.physical_footprint).sum();
        self.sort();
        if self.state.selected().is_none() && !self.processes.is_empty() {
            self.state.select(Some(0));
        }
    }

    pub fn kill_selected(&mut self) {
        if let Some(i) = self.state.selected() {
            if let Some(p) = self.processes.get(i) {
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(p.pid.to_string())
                    .output();
                // Optionally remove from list immediately or wait for refresh
            }
        }
    }
}
