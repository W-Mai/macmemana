use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::process::Command;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct FootprintOutput {
    processes: Vec<FootprintProcess>,
}

#[derive(Debug, Deserialize)]
struct FootprintProcess {
    pid: i32,
    name: String,
    #[serde(default)]
    footprint: u64, // Matches "total footprint" in JSON (Physical + Swapped? No, seems to be Dirty/Physical)
    // Actually check JSON output again:
    // "footprint": 8995904
    // "auxiliary": { "phys_footprint": 9028672 }
    // They are close but not identical. "footprint" might be Dirty.
    // We prefer "phys_footprint" from auxiliary if available.
    auxiliary: Option<FootprintAuxiliary>,
    summary: Option<FootprintSummary>,
}

#[derive(Debug, Deserialize)]
struct FootprintAuxiliary {
    phys_footprint: u64,
}

#[derive(Debug, Deserialize)]
struct FootprintSummary {
    total: Option<FootprintTotal>,
}

#[derive(Debug, Deserialize)]
struct FootprintTotal {
    swapped: u64,
}

#[derive(Debug, Clone)]
pub struct FootprintData {
    #[allow(dead_code)]
    pub pid: i32,
    pub name: String,
    pub physical_footprint: u64,
    pub swapped_total: u64,
}

pub fn get_all_processes_footprint() -> Result<HashMap<i32, FootprintData>> {
    let tmp_path =
        std::env::temp_dir().join(format!("macmemana_footprint_{}.json", Uuid::new_v4()));

    // Check if we have root, otherwise this might fail for -a.
    // If it fails, we return error and scanner can fallback.
    let status = Command::new("footprint")
        .arg("-a")
        .arg("-j")
        .arg(&tmp_path)
        .status()
        .context("Failed to execute footprint")?;

    if !status.success() {
        // Clean up if created empty file
        if tmp_path.exists() {
            let _ = fs::remove_file(&tmp_path);
        }
        return Err(anyhow::anyhow!(
            "footprint command failed (likely permission denied for -a)"
        ));
    }

    let content = fs::read_to_string(&tmp_path).context("Failed to read footprint output")?;
    let _ = fs::remove_file(&tmp_path); // Cleanup immediately

    let output: FootprintOutput =
        serde_json::from_str(&content).context("Failed to parse footprint JSON")?;

    let mut map = HashMap::new();
    for p in output.processes {
        let phys = p
            .auxiliary
            .as_ref()
            .map(|a| a.phys_footprint)
            .unwrap_or(p.footprint);
        let swapped = p
            .summary
            .as_ref()
            .and_then(|s| s.total.as_ref())
            .map(|t| t.swapped)
            .unwrap_or(0);

        map.insert(
            p.pid,
            FootprintData {
                pid: p.pid,
                name: p.name,
                physical_footprint: phys,
                swapped_total: swapped,
            },
        );
    }

    Ok(map)
}
