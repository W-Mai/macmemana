use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct ProcessMemory {
    pub pid: i32,
    pub name: String,
    pub physical_footprint: u64,
    pub compressed: u64,
    pub swapped_total: u64,
    pub swap_disk_est: u64,
    pub swap_disk: u64,
}

impl ProcessMemory {
    pub fn total(&self) -> u64 {
        self.physical_footprint + self.swap_disk
    }

    /// Creates a simple ProcessMemory from sysinfo data (RSS as placeholder for Phys).
    /// Used for immediate "Fast Path" display.
    pub fn new_simple(pid: i32, name: String, rss: u64) -> Self {
        Self {
            pid,
            name,
            physical_footprint: rss, // Approximation for fast display
            compressed: 0,
            swapped_total: 0,
            swap_disk_est: 0,
            swap_disk: 0,
        }
    }

    /// Merges precise data from footprint into this struct.
    pub fn merge_footprint(&mut self, footprint_data: &crate::footprint::FootprintData, compressed: u64) {
        self.physical_footprint = footprint_data.physical_footprint;
        self.swapped_total = footprint_data.swapped_total;
        self.compressed = compressed;
        
        // Calculate swap estimates
        self.swap_disk_est = self.swapped_total.saturating_sub(self.compressed);
        self.swap_disk = self.swap_disk_est; // Initial value, will be normalized later
    }
}

pub(crate) fn parse_size(size_str: &str) -> u64 {
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

pub(crate) fn format_size(bytes: u64) -> String {
    const K: f64 = 1024.0;
    const M: f64 = 1024.0 * 1024.0;
    const G: f64 = 1024.0 * 1024.0 * 1024.0;

    let b = bytes as f64;
    if bytes >= G as u64 {
        format!("{:.1}G", b / G)
    } else if bytes >= M as u64 {
        format!("{:.1}M", b / M)
    } else if bytes >= K as u64 {
        format!("{:.1}K", b / K)
    } else {
        format!("{}B", bytes)
    }
}

#[allow(dead_code)]
pub fn get_process_memory(pid: i32, name: &str) -> Result<ProcessMemory> {
    let output = Command::new("vmmap")
        .arg("-summary")
        .arg(pid.to_string())
        .output()
        .context("Failed to execute vmmap")?;

    if !output.status.success() {
        return Ok(ProcessMemory {
            pid,
            name: name.to_string(),
            physical_footprint: 0,
            compressed: 0,
            swapped_total: 0,
            swap_disk_est: 0,
            swap_disk: 0,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_vmmap_output(pid, name, &stdout)
}

fn parse_vmmap_output(pid: i32, name: &str, output: &str) -> Result<ProcessMemory> {
    static RE_PHYSICAL: OnceLock<Regex> = OnceLock::new();
    static RE_COMPRESSED: OnceLock<Regex> = OnceLock::new();
    static RE_SWAP_USED: OnceLock<Regex> = OnceLock::new();
    static RE_WRITABLE: OnceLock<Regex> = OnceLock::new();
    static RE_TOTAL_TABLE: OnceLock<Regex> = OnceLock::new();
    static RE_TOTAL_TABLE_MINUS_RESERVED: OnceLock<Regex> = OnceLock::new();

    let re_physical =
        RE_PHYSICAL.get_or_init(|| Regex::new(r"Physical footprint:\s+([\d\.]+)([KMG]?)").unwrap());
    let re_compressed =
        RE_COMPRESSED.get_or_init(|| Regex::new(r"Compressed:\s+([\d\.]+)([KMG]?)").unwrap());
    let re_swap_used =
        RE_SWAP_USED.get_or_init(|| Regex::new(r"Swap used:\s+([\d\.]+)([KMG]?)").unwrap());
    let re_writable = RE_WRITABLE
        .get_or_init(|| Regex::new(r"Writable regions:.*swapped_out=([\d\.]+)([KMG]?)").unwrap());

    let re_total_table = RE_TOTAL_TABLE
        .get_or_init(|| Regex::new(r"^TOTAL\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)").unwrap());
    let re_total_table_minus_reserved = RE_TOTAL_TABLE_MINUS_RESERVED.get_or_init(|| {
        Regex::new(r"^TOTAL,\s+minus\s+reserved\s+VM\s+space\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)")
            .unwrap()
    });

    let mut phys = 0;
    let mut compressed = 0;
    let mut swap_used = 0;
    let mut resident_from_table = 0;
    let mut swap_from_table = 0;
    let mut writable_swapped_out = 0;
    let mut found_total_table = false;
    let mut in_region_type_table = false;

    for line in output.lines() {
        let line = line.trim();
        if let Some(caps) = re_physical.captures(line) {
            phys = parse_size(&format!("{}{}", &caps[1], &caps[2]));
        }
        if let Some(caps) = re_compressed.captures(line) {
            compressed = parse_size(&format!("{}{}", &caps[1], &caps[2]));
        }
        if let Some(caps) = re_swap_used.captures(line) {
            swap_used = parse_size(&format!("{}{}", &caps[1], &caps[2]));
        }
        if let Some(caps) = re_writable.captures(line) {
            writable_swapped_out = parse_size(&format!("{}{}", &caps[1], &caps[2]));
        }

        if line.starts_with("REGION TYPE") {
            in_region_type_table = true;
        }
        if !in_region_type_table
            && line.contains("VIRTUAL")
            && line.contains("RESIDENT")
            && line.contains("SWAPPED")
            && !line.contains("MALLOC")
        {
            in_region_type_table = true;
        }

        if in_region_type_table {
            if let Some(caps) = re_total_table_minus_reserved.captures(line) {
                resident_from_table = parse_size(&caps[2]);
                swap_from_table = parse_size(&caps[4]);
                found_total_table = true;
                continue;
            }

            if !found_total_table && let Some(caps) = re_total_table.captures(line) {
                resident_from_table = parse_size(&caps[2]);
                swap_from_table = parse_size(&caps[4]);
                found_total_table = true;
            }
        }
    }

    let swapped_total = if swap_from_table > 0 {
        swap_from_table
    } else if writable_swapped_out > 0 {
        writable_swapped_out
    } else if phys > 0 && resident_from_table > 0 {
        phys.saturating_sub(resident_from_table)
    } else {
        0
    };

    let disk_from_swapped = swapped_total.saturating_sub(compressed);
    let swap_disk_est = if swapped_total > 0 {
        if swap_used > 0 {
            disk_from_swapped.min(swap_used)
        } else {
            disk_from_swapped
        }
    } else if swap_used > 0 {
        swap_used
    } else {
        0
    };

    Ok(ProcessMemory {
        pid,
        name: name.to_string(),
        physical_footprint: phys,
        compressed,
        swapped_total,
        swap_disk_est,
        swap_disk: swap_disk_est,
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
Swap used:             1.23G
                                VIRTUAL RESIDENT    DIRTY  SWAPPED
TOTAL                              1.0G     1.0G     1.0G     5.84G
        "#;

        let mem = parse_vmmap_output(12345, "TestApp", output).unwrap();
        assert_eq!(
            mem.physical_footprint,
            (8.12 * 1024.0 * 1024.0 * 1024.0) as u64
        );
        assert_eq!(mem.swapped_total, (5.84 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(mem.compressed, (2.31 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(mem.swap_disk_est, (1.23 * 1024.0 * 1024.0 * 1024.0) as u64);
    }

    #[test]
    fn test_parse_vmmap_output_table_priority() {
        // Case where both Table and Writable are present. Table (1.9G) should win over Writable (1.7G).
        let output = r#"
Process:         WindowServer [413]
Physical footprint:         3.0G
Writable regions: Total=3.0G written=127.6M(4%) resident=360.6M(12%) swapped_out=1.7G(57%)
                                VIRTUAL RESIDENT    DIRTY  SWAPPED VOLATILE   NONVOL    EMPTY   REGION 
REGION TYPE                        SIZE     SIZE     SIZE     SIZE     SIZE     SIZE     SIZE    COUNT (non-coalesced) 
===========                     ======= ========    =====  ======= ========   ======    =====  ======= 
TOTAL                              6.4G     1.1G     1.1G     1.9G       0K   257.5M   197.0M    63744 
        "#;

        let mem = parse_vmmap_output(413, "WindowServer", output).unwrap();
        // Phys: 3.0G
        assert_eq!(
            mem.physical_footprint,
            (3.0 * 1024.0 * 1024.0 * 1024.0) as u64
        );
        // Swapped should be 1.9G (from Table), not 1.7G (from Writable)
        assert_eq!(mem.swapped_total, (1.9 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(mem.swap_disk_est, (1.9 * 1024.0 * 1024.0 * 1024.0) as u64);
    }

    #[test]
    fn test_parse_vmmap_output_phys_fallback() {
        // Case where Table SWAPPED is missing or 0, but we have Phys and Res
        let output = r#"
Process:         TestApp [123]
Physical footprint:         3.0G
                                VIRTUAL RESIDENT    DIRTY  SWAPPED
TOTAL                              6.4G     1.0G     1.0G     0K
        "#;
        let mem = parse_vmmap_output(123, "TestApp", output).unwrap();
        // Phys 3.0G. Res 1.0G. Swap 0.
        // Swapped = Phys - Res = 2.0G
        assert_eq!(mem.swapped_total, (2.0 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(mem.swap_disk_est, (2.0 * 1024.0 * 1024.0 * 1024.0) as u64);
    }

    #[test]
    fn test_parse_vmmap_output_writable_fallback() {
        // Case where Summary is missing Compressed/Swap, Table is truncated, but Writable line is present
        let output = r#"
Process:         JetBrains Toolbox [18084]
Physical footprint:         591.1M
Writable regions: Total=803.6M written=512.8M(64%) resident=371.0M(46%) swapped_out=213.9M(27%)
        "#;
        let mem = parse_vmmap_output(18084, "JetBrains Toolbox", output).unwrap();
        // Phys 591.1M.
        assert_eq!(mem.physical_footprint, (591.1 * 1024.0 * 1024.0) as u64);
        // Swapped = Writable swapped_out = 213.9M
        assert_eq!(mem.swapped_total, (213.9 * 1024.0 * 1024.0) as u64);
        assert_eq!(mem.swap_disk_est, (213.9 * 1024.0 * 1024.0) as u64);
    }

    #[test]
    fn test_parse_vmmap_output_disk_swap_est_from_swapped_minus_compressed() {
        let output = r#"
Process:         WindowServer [413]
Physical footprint:         3.9G
Compressed:               900M
                                VIRTUAL RESIDENT    DIRTY  SWAPPED
TOTAL                              6.2G     1.2G     1.3G     2.7G
        "#;
        let mem = parse_vmmap_output(413, "WindowServer", output).unwrap();
        assert_eq!(mem.swapped_total, (2.7 * 1024.0 * 1024.0 * 1024.0) as u64);
        let expected = parse_size("2.7G").saturating_sub(parse_size("900M"));
        assert_eq!(mem.swap_disk_est, expected);
    }

    #[test]
    fn test_parse_vmmap_output_windowserver_fixture() {
        let output = include_str!("../windowserver_vmmap.txt");
        let mem = parse_vmmap_output(413, "WindowServer", output).unwrap();
        assert_eq!(
            mem.physical_footprint,
            (3.1 * 1024.0 * 1024.0 * 1024.0) as u64
        );
        assert_eq!(mem.swap_disk_est, (1.8 * 1024.0 * 1024.0 * 1024.0) as u64);
    }
}
