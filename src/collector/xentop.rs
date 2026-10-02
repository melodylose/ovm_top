use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

#[derive(Debug, Clone, Default)]
pub struct DomainStats {
    pub name: String,
    pub state: String,
    pub cpu_seconds: f64,
    pub cpu_percent: f64,
    pub memory_kb: f64,
    pub memory_percent: f64,
    pub max_memory_kb: f64,
    pub max_memory_percent: f64,
    pub vcpus: u32,
    pub nets: u32,
    pub net_tx_kb: f64,
    pub net_rx_kb: f64,
    pub vbds: u32,
    pub vbd_oo: u64,
    pub vbd_rd: u64,
    pub vbd_wr: u64,
    pub vbd_rsect: u64,
    pub vbd_wsect: u64,
    pub ssid: u32,
}

pub fn spawn_collector(domains: Arc<Mutex<Vec<DomainStats>>>) -> Result<()> {
    let mut child = Command::new("xentop")
        .args(["-b", "-d", "1"])
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to start xentop")?;

    let stdout = child
        .stdout
        .take()
        .context("failed to capture xentop stdout")?;

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut latest = HashMap::<String, DomainStats>::new();

        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };

            if let Some(stats) = parse_line(&line) {
                latest.insert(stats.name.clone(), stats);

                let mut new_domains: Vec<_> = latest.values().cloned().collect();

                new_domains.sort_by(|a, b| a.name.cmp(&b.name));

                if let Ok(mut shared) = domains.lock() {
                    *shared = new_domains;
                }
            }
        }
    });

    Ok(())
}

pub fn get_domains_once() -> Result<Vec<DomainStats>> {
    let output = Command::new("sudo")
        .args(["env", "TERM=xterm", "xentop", "-b", "-d", "1", "-i", "2"])
        .output()
        .context("failed to execute xentop")?;

    if !output.status.success() {
        anyhow::bail!("xentop failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut latest = HashMap::<String, DomainStats>::new();

    for line in stdout.lines() {
        if let Some(stats) = parse_line(line) {
            latest.insert(stats.name.clone(), stats);
        }
    }

    let mut domains: Vec<_> = latest.into_values().collect();

    domains.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(domains)
}

pub fn parse_line(line: &str) -> Option<DomainStats> {
    let cols: Vec<&str> = line.split_whitespace().collect();

    if cols.len() < 19 {
        return None;
    }

    if cols[0] == "NAME" {
        return None;
    }

    Some(DomainStats {
        name: cols[0].to_string(),
        state: cols[1].to_string(),
        cpu_seconds: cols[2].parse().ok()?,
        cpu_percent: cols[3].parse().ok()?,
        memory_kb: cols[4].parse().ok()?,
        memory_percent: cols[5].parse().ok()?,
        max_memory_kb: cols[6].parse().ok()?,
        max_memory_percent: cols[7].parse().ok()?,
        vcpus: cols[8].parse().ok()?,
        nets: cols[9].parse().ok()?,
        net_tx_kb: cols[10].parse().ok()?,
        net_rx_kb: cols[11].parse().ok()?,
        vbds: cols[12].parse().ok()?,
        vbd_oo: cols[13].parse().ok()?,
        vbd_rd: cols[14].parse().ok()?,
        vbd_wr: cols[15].parse().ok()?,
        vbd_rsect: cols[16].parse().ok()?,
        vbd_wsect: cols[17].parse().ok()?,
        ssid: cols[18].parse().ok()?,
    })
}
