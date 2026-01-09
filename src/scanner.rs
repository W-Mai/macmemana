use std::process::Command;
use regex::Regex;
use anyhow::{Result, Context};
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct ProcessMemory {
    pub pid: i32,
    pub name: String,
    pub physical_footprint: u64,
    pub compressed: u64,
    pub swap_used: u64,
}

impl ProcessMemory {
    pub fn total(&self) -> u64 {
        self.physical_footprint + self.swap_used + self.compressed
    }
}

fn parse_size(size_str: &str) -> u64 {
    let size_str = size_str.trim().to_uppercase();
    let (num_str, multiplier) = if size_str.ends_with('G') {
        (size_str.trim_end_matches('G'), 1024 * 1024 * 1024)
    } else if size_str.ends_with('M') {
        (size_str.trim_end_matches('M'), 1024 * 1024)
    } else if size_str.ends_with('K') {
        (size_str.trim_end_matches('K'), 1024)
    } else {
        (size_str.trim_end_matches('B'), 1)
    };

    let num: f64 = num_str.parse().unwrap_or(0.0);
    (num * multiplier as f64) as u64
}

pub fn get_process_memory(pid: i32, name: &str) -> Result<ProcessMemory> {
    let output = Command::new("vmmap")
        .arg("-summary")
        .arg(pid.to_string())
        .output()
        .context("Failed to execute vmmap")?;

    if !output.status.success() {
        // If vmmap fails (e.g. permission denied or process gone), return 0s or error.
        // For now, let's just return 0s to avoid crashing the whole scan.
        return Ok(ProcessMemory {
            pid,
            name: name.to_string(),
            physical_footprint: 0,
            compressed: 0,
            swap_used: 0,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_vmmap_output(pid, name, &stdout)
}

fn parse_vmmap_output(pid: i32, name: &str, output: &str) -> Result<ProcessMemory> {
    static RE_PHYSICAL: OnceLock<Regex> = OnceLock::new();
    static RE_COMPRESSED: OnceLock<Regex> = OnceLock::new();
    static RE_SWAP: OnceLock<Regex> = OnceLock::new();

    let re_physical = RE_PHYSICAL.get_or_init(|| Regex::new(r"Physical footprint:\s+([\d\.]+)([KMG]?)").unwrap());
    let re_compressed = RE_COMPRESSED.get_or_init(|| Regex::new(r"Compressed:\s+([\d\.]+)([KMG]?)").unwrap());
    let re_swap = RE_SWAP.get_or_init(|| Regex::new(r"Swap used:\s+([\d\.]+)([KMG]?)").unwrap());

    let mut phys = 0;
    let mut comp = 0;
    let mut swap = 0;

    for line in output.lines() {
        if let Some(caps) = re_physical.captures(line) {
            phys = parse_size(&format!("{}{}", &caps[1], &caps[2]));
        }
        if let Some(caps) = re_compressed.captures(line) {
            comp = parse_size(&format!("{}{}", &caps[1], &caps[2]));
        }
        if let Some(caps) = re_swap.captures(line) {
            swap = parse_size(&format!("{}{}", &caps[1], &caps[2]));
        }
    }

    Ok(ProcessMemory {
        pid,
        name: name.to_string(),
        physical_footprint: phys,
        compressed: comp,
        swap_used: swap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("1G"), 1024 * 1024 * 1024);
        assert_eq!(parse_size("500M"), 500 * 1024 * 1024);
        assert_eq!(parse_size("1.5G"), (1.5 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(parse_size("100K"), 100 * 1024);
    }

    #[test]
    fn test_parse_vmmap_output() {
        let output = r#"
Virtual Memory Map of process 12345 (TestApp)
Output report format:  2.4  -- 64-bit process
...
Physical footprint:     8.12G
Compressed:            2.31G
Swap used:             5.84G
        "#;
        
        let mem = parse_vmmap_output(12345, "TestApp", output).unwrap();
        assert_eq!(mem.physical_footprint, (8.12 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(mem.compressed, (2.31 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(mem.swap_used, (5.84 * 1024.0 * 1024.0 * 1024.0) as u64);
    }
}
