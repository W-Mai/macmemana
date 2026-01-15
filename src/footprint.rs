use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::process::{Command, Stdio};
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
    footprint: u64,
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
    #[allow(dead_code)]
    pub name: String,
    pub physical_footprint: u64,
    pub swapped_total: u64,
}

pub fn get_footprint_for_pids(pids: &[i32]) -> Result<HashMap<i32, FootprintData>> {
    if pids.is_empty() {
        return Ok(HashMap::new());
    }

    let tmp_path =
        std::env::temp_dir().join(format!("macmemana_footprint_{}.json", Uuid::new_v4()));

    let mut cmd = Command::new("footprint");
    cmd.arg("-j").arg(&tmp_path);
    
    // Add all PIDs as arguments
    for pid in pids {
        cmd.arg(pid.to_string());
    }

    // Silence stderr and stdout to prevent TUI corruption
    let status = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("Failed to execute footprint")?;

    if !status.success() {
        // Clean up if created empty file
        if tmp_path.exists() {
            let _ = fs::remove_file(&tmp_path);
        }
        return Err(anyhow::anyhow!(
            "footprint command failed"
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
