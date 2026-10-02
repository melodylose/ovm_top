use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, Default, Clone)]
pub struct XmInfo {
    pub host: String,
    pub xen_version: String,
    pub nr_cpus: u32,
    pub nr_nodes: u32,
    pub total_memory_mb: u64,
    pub free_memory_mb: u64,
    pub scheduler: String,
}

pub fn mock_xm_info() -> XmInfo {
    XmInfo {
        host: "SWB-AMS-AP7".to_string(),
        xen_version: "4.4.4OVM".to_string(),
        nr_cpus: 32,
        nr_nodes: 2,
        total_memory_mb: 131026,
        free_memory_mb: 127234,
        scheduler: "credit".to_string(),
    }
}

pub fn get_xm_info() -> Result<XmInfo> {
    let output = Command::new("xm")
        .arg("info")
        .output()
        .context("failed to execute xm info")?;

    if !output.status.success() {
        anyhow::bail!(
            "xm info failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut info = XmInfo::default();

    let mut xen_major = String::new();
    let mut xen_minor = String::new();
    let mut xen_extra = String::new();

    for line in stdout.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };

        let key = key.trim();
        let value = value.trim();

        match key {
            "host" => info.host = value.to_string(),

            "nr_cpus" => {
                info.nr_cpus = value.parse().unwrap_or_default();
            }

            "nr_nodes" => {
                info.nr_nodes = value.parse().unwrap_or_default();
            }

            "total_memory" => {
                info.total_memory_mb = value.parse().unwrap_or_default();
            }

            "free_memory" => {
                info.free_memory_mb = value.parse().unwrap_or_default();
            }

            "xen_scheduler" => {
                info.scheduler = value.to_string();
            }
            "xen_major" => xen_major = value.to_string(),
            "xen_minor" => xen_minor = value.to_string(),
            "xen_extra" => xen_extra = value.to_string(),

            _ => {}
        }
    }

    info.xen_version = format!("{xen_major}.{xen_minor}{xen_extra}");

    Ok(info)
}
