use anyhow::{Context, Result};
use std::collections::HashMap;
use std::process::Command;

pub fn get_all_processes_compressed() -> Result<HashMap<i32, u64>> {
    let output = Command::new("top")
        .arg("-l")
        .arg("1")
        .arg("-stats")
        .arg("pid,compress") // Use -stats to get specific columns cleanly?
        // top -stats pid,compress might work and be easier to parse.
        .output()
        .context("Failed to execute top")?;

    if !output.status.success() {
        // Fallback to full top if stats not supported?
        // But macOS top supports -stats.
        return Err(anyhow::anyhow!("top command failed"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_top_output(&stdout)
}

fn parse_top_output(output: &str) -> Result<HashMap<i32, u64>> {
    let mut map = HashMap::new();
    let lines = output.lines();

    // Skip until we find header
    // Header with -stats pid,compress should be "PID   COMPRESS" (or CMPRS?)
    // Let's check output of top -l 1 -stats pid,compress first in thought block.
    // Assuming standard top output structure: Headers, then processes.

    // Actually, let's just parse line by line. If it starts with a number, it's a PID line.
    // If we use -stats, the columns are fixed.

    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        // Check if first part is a number (PID)
        if let Ok(pid) = parts[0].parse::<i32>() {
            // Second part should be compressed size
            let compressed_str = parts[1];
            let compressed = parse_size(compressed_str);
            map.insert(pid, compressed);
        }
    }

    Ok(map)
}

// Duplicated from scanner.rs to avoid circular deps or complex visibility changes for now,
// or I can import it if I make it public.
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
