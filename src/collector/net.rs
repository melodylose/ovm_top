use crate::log::{LogEvent, LogSource};
use crate::topology::snapshot::NetInterface;
use anyhow::Result;
use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex, mpsc::Sender},
    thread,
    time::Duration,
};

#[derive(Debug, Clone, Default)]
pub struct NetStats {
    pub name: String,
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub rx_errors: u64,
    pub rx_dropped: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    pub tx_errors: u64,
    pub tx_dropped: u64,
}

#[derive(Debug, Clone, Default)]
pub struct NetRate {
    pub name: String,
    pub rx_bytes_per_sec: u64,
    pub rx_packets_per_sec: u64,
    pub rx_drops_per_sec: u64,
    pub tx_bytes_per_sec: u64,
    pub tx_packets_per_sec: u64,
    pub tx_drops_per_sec: u64,
}

pub fn spawn_network_collector(network: Arc<Mutex<Vec<NetRate>>>, log_tx: Sender<LogEvent>) {
    thread::spawn(move || {
        let mut previous = match read_net_dev() {
            Ok(stats) => stats,
            Err(error) => {
                let _ = log_tx.send(LogEvent::error(
                    LogSource::Network,
                    format!("failed to read /proc/net/dev: {error}"),
                ));
                return;
            }
        };

        loop {
            thread::sleep(Duration::from_secs(1));

            let current = match read_net_dev() {
                Ok(stats) => stats,
                Err(error) => {
                    let _ = log_tx.send(LogEvent::warn(
                        LogSource::Network,
                        format!("failed to refresh /proc/net/dev: {error}"),
                    ));
                    continue;
                }
            };

            let rates = calculate_rates(&previous, &current);

            if let Ok(mut shared) = network.lock() {
                *shared = rates;
            }

            previous = current;
        }
    });
}

pub fn calculate_rates(previous: &[NetStats], current: &[NetStats]) -> Vec<NetRate> {
    let previous_map: HashMap<&str, &NetStats> = previous
        .iter()
        .map(|stat| (stat.name.as_str(), stat))
        .collect();

    let mut rates = Vec::new();

    for curr in current {
        let Some(prev) = previous_map.get(curr.name.as_str()) else {
            continue;
        };

        rates.push(NetRate {
            name: curr.name.clone(),

            rx_bytes_per_sec: curr.rx_bytes.saturating_sub(prev.rx_bytes),
            rx_packets_per_sec: curr.rx_packets.saturating_sub(prev.rx_packets),
            rx_drops_per_sec: curr.rx_dropped.saturating_sub(prev.rx_dropped),

            tx_bytes_per_sec: curr.tx_bytes.saturating_sub(prev.tx_bytes),
            tx_packets_per_sec: curr.tx_packets.saturating_sub(prev.tx_packets),
            tx_drops_per_sec: curr.tx_dropped.saturating_sub(prev.tx_dropped),
        });
    }

    rates
}

pub fn read_net_dev() -> Result<Vec<NetStats>> {
    let content = fs::read_to_string("/proc/net/dev")?;

    let mut interfaces = Vec::new();

    for line in content.lines().skip(2) {
        let Some((name, data)) = line.split_once(':') else {
            continue;
        };

        let cols: Vec<&str> = data.split_whitespace().collect();

        if cols.len() < 16 {
            continue;
        }

        let stats = NetStats {
            name: name.trim().to_string(),

            // Receive
            rx_bytes: cols[0].parse().unwrap_or_default(),
            rx_packets: cols[1].parse().unwrap_or_default(),
            rx_errors: cols[2].parse().unwrap_or_default(),
            rx_dropped: cols[3].parse().unwrap_or_default(),

            // Transmit
            tx_bytes: cols[8].parse().unwrap_or_default(),
            tx_packets: cols[9].parse().unwrap_or_default(),
            tx_errors: cols[10].parse().unwrap_or_default(),
            tx_dropped: cols[11].parse().unwrap_or_default(),
        };

        interfaces.push(stats);
    }

    Ok(interfaces)
}

pub fn read_net_topology() -> Result<Vec<NetInterface>> {
    let mut interfaces = Vec::new();
    for entry in fs::read_dir("/sys/class/net")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let kind = if path.join("bonding").exists() {
            "bond"
        } else if path.join("bridge").exists() {
            "bridge"
        } else if name.starts_with("vif") {
            "vif"
        } else if path.join("device").exists() {
            "physical"
        } else {
            "virtual"
        };
        let master = fs::read_link(path.join("master")).ok().and_then(|link| {
            link.file_name()
                .map(|value| value.to_string_lossy().into_owned())
        });
        let members = if kind == "bond" {
            fs::read_to_string(format!("/proc/net/bonding/{name}"))
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.strip_prefix("Slave Interface: "))
                .map(str::to_string)
                .collect()
        } else {
            Vec::new()
        };
        interfaces.push(NetInterface {
            name,
            kind: kind.to_string(),
            master,
            members,
        });
    }
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(interfaces)
}
