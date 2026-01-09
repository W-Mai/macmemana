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
    static RE_TOTAL_TABLE: OnceLock<Regex> = OnceLock::new();

    let re_physical = RE_PHYSICAL.get_or_init(|| Regex::new(r"Physical footprint:\s+([\d\.]+)([KMG]?)").unwrap());
    let re_compressed = RE_COMPRESSED.get_or_init(|| Regex::new(r"Compressed:\s+([\d\.]+)([KMG]?)").unwrap());
    let re_swap = RE_SWAP.get_or_init(|| Regex::new(r"Swap used:\s+([\d\.]+)([KMG]?)").unwrap());
    // Match TOTAL line in REGION TYPE table: TOTAL ...
    // Columns: VIRTUAL RESIDENT DIRTY SWAPPED ...
    // We want capture group 2 (Resident) and 4 (Swapped)
    // Regex: ^TOTAL\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)
    let re_total_table = RE_TOTAL_TABLE.get_or_init(|| Regex::new(r"^TOTAL\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)").unwrap());

    let mut phys = 0;
    let mut comp = 0;
    let mut swap = 0;
    let mut swap_from_table = 0;
    let mut found_total_table = false;

    for line in output.lines() {
        let line = line.trim();
        if let Some(caps) = re_physical.captures(line) {
            phys = parse_size(&format!("{}{}", &caps[1], &caps[2]));
        }
        if let Some(caps) = re_compressed.captures(line) {
            comp = parse_size(&format!("{}{}", &caps[1], &caps[2]));
        }
        if let Some(caps) = re_swap.captures(line) {
            swap = parse_size(&format!("{}{}", &caps[1], &caps[2]));
        }
        
        // Fallback: Parse table if explicit lines are missing
        if !found_total_table {
             if let Some(caps) = re_total_table.captures(line) {
                 swap_from_table = parse_size(&caps[4]);
                 found_total_table = true;
             }
        }
    }

    // Apply fallbacks
    // If we have explicit swap line (from summary), use it.
    // If NOT, we used to use swap_from_table. But we now know swap_from_table is Total Swapped (Comp + Disk).
    // So we should NOT eagerly assign it to `swap` (which implies disk swap).
    // We should let the logic below handle the split.
    
    // Estimate compressed if missing
    // Heuristic:
    // vmmap's "SWAPPED" column in the table (swap_from_table) roughly equals "VM Compressed" + "Swap Used" (Disk).
    // If we have an explicit "Swap used:" line (captured in `swap`), we can deduce Compressed.
    // Compressed = SWAPPED (table) - Swap used (explicit).
    //
    // If explicit "Swap used:" is missing (common on Sequoia), we assume Disk Swap is small/zero for per-process attribution
    // (unless we can find another source), so Compressed ~= SWAPPED (table).
    //
    // However, if we blindly set `swap = swap_from_table` (like we did before), we are saying it's ALL Disk Swap.
    // Activity Monitor puts this in "VM Compressed".
    // So we should shift `swap_from_table` to `comp` if `swap` (explicit) is 0.
    
    if comp == 0 {
        if swap > 0 && swap_from_table > swap {
            // We have explicit disk swap, and table swap is larger.
            // Difference is likely compressed.
            comp = swap_from_table.saturating_sub(swap);
        } else if swap == 0 && swap_from_table > 0 {
            // No explicit disk swap found. Assume table swap is mostly Compressed.
            // This matches Activity Monitor which shows "VM Compressed" for these values.
            comp = swap_from_table;
            // But wait, should we report swap as 0?
            // If the user wants to see "Swap", and we move it to "Compressed", Swap will be 0.
            // This is actually CORRECT if it's compressed memory and not disk swap.
            // Activity Monitor shows "Swap Used: 0 bytes" for these processes usually.
        }
    }
    
    // Safety check: if we moved everything to comp, ensure swap is 0 if it was derived from table
    if swap == swap_from_table && comp == swap_from_table {
        // We just duplicated it.
        // If we decided it's compressed, we should zero out swap (disk).
        swap = 0;
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

    #[test]
    fn test_parse_vmmap_output_table_fallback() {
        let output = r#"
Process:         WindowServer [413]
...
Physical footprint:         3.0G
...
                                VIRTUAL RESIDENT    DIRTY  SWAPPED VOLATILE   NONVOL    EMPTY   REGION 
REGION TYPE                        SIZE     SIZE     SIZE     SIZE     SIZE     SIZE     SIZE    COUNT (non-coalesced) 
===========                     ======= ========    =====  ======= ========   ======    =====  ======= 
TOTAL                              6.4G     1.1G     1.1G     1.9G       0K   257.5M   197.0M    63744 
        "#;
        
        let mem = parse_vmmap_output(413, "WindowServer", output).unwrap();
        // Phys: 3.0G
        assert_eq!(mem.physical_footprint, (3.0 * 1024.0 * 1024.0 * 1024.0) as u64);
        
        // Old logic: Swap = 1.9G, Comp = Phys - Dirty = 1.9G.
        // New logic: 
        // 1. explicit swap = 0.
        // 2. swap_from_table = 1.9G.
        // 3. comp = swap_from_table = 1.9G.
        // 4. swap (disk) remains 0.
        
        // This is arguably MORE correct for Activity Monitor matching, as AM shows 2.0G Compressed and 0 Swap (usually).
        assert_eq!(mem.compressed, (1.9 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(mem.swap_used, 0); 
    }
}
