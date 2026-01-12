use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct ProcessMemory {
    pub pid: i32,
    pub name: String,
    pub physical_footprint: u64,
    pub swapped: u64,
}

impl ProcessMemory {
    pub fn total(&self) -> u64 {
        self.physical_footprint + self.swapped
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
            swapped: 0,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_vmmap_output(pid, name, &stdout)
}

fn parse_vmmap_output(pid: i32, name: &str, output: &str) -> Result<ProcessMemory> {
    static RE_PHYSICAL: OnceLock<Regex> = OnceLock::new();
    static RE_WRITABLE: OnceLock<Regex> = OnceLock::new();
    static RE_TOTAL_TABLE: OnceLock<Regex> = OnceLock::new();
    static RE_TOTAL_TABLE_MINUS_RESERVED: OnceLock<Regex> = OnceLock::new();

    let re_physical =
        RE_PHYSICAL.get_or_init(|| Regex::new(r"Physical footprint:\s+([\d\.]+)([KMG]?)").unwrap());
    // Parse Writable regions line: "Writable regions: Total=803.6M written=512.8M(64%) resident=371.0M(46%) swapped_out=213.9M(27%)"
    let re_writable = RE_WRITABLE
        .get_or_init(|| Regex::new(r"Writable regions:.*swapped_out=([\d\.]+)([KMG]?)").unwrap());

    // Match TOTAL line in REGION TYPE table: TOTAL ...
    // Columns: VIRTUAL RESIDENT DIRTY SWAPPED ...
    // We want capture group 2 (Resident) and 4 (Swapped)
    // Regex: ^TOTAL\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)
    let re_total_table = RE_TOTAL_TABLE
        .get_or_init(|| Regex::new(r"^TOTAL\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)").unwrap());
    let re_total_table_minus_reserved = RE_TOTAL_TABLE_MINUS_RESERVED.get_or_init(|| {
        Regex::new(r"^TOTAL,\s+minus\s+reserved\s+VM\s+space\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)")
            .unwrap()
    });

    let mut phys = 0;
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

    let swapped = if swap_from_table > 0 {
        swap_from_table
    } else if writable_swapped_out > 0 {
        writable_swapped_out
    } else if phys > 0 && resident_from_table > 0 {
        phys.saturating_sub(resident_from_table)
    } else {
        0
    };

    Ok(ProcessMemory {
        pid,
        name: name.to_string(),
        physical_footprint: phys,
        swapped,
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
                                VIRTUAL RESIDENT    DIRTY  SWAPPED
TOTAL                              1.0G     1.0G     1.0G     5.84G
        "#;

        let mem = parse_vmmap_output(12345, "TestApp", output).unwrap();
        assert_eq!(
            mem.physical_footprint,
            (8.12 * 1024.0 * 1024.0 * 1024.0) as u64
        );
        assert_eq!(mem.swapped, (5.84 * 1024.0 * 1024.0 * 1024.0) as u64);
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
        assert_eq!(mem.swapped, (1.9 * 1024.0 * 1024.0 * 1024.0) as u64);
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
        assert_eq!(mem.swapped, (2.0 * 1024.0 * 1024.0 * 1024.0) as u64);
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
        assert_eq!(mem.swapped, (213.9 * 1024.0 * 1024.0) as u64);
    }
}
