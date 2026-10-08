use crate::log::{LogEvent, LogSource, WarningSuppressor};
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
        let mut warnings = WarningSuppressor::default();
        let mut previous = match read_net_dev() {
            Ok(stats) => stats,
            Err(error) => {
                warnings.emit(
                    &log_tx,
                    LogEvent::error(
                        LogSource::Network,
                        format!("failed to read /proc/net/dev: {error}"),
                    ),
                );
                return;
            }
        };

        loop {
            thread::sleep(Duration::from_secs(1));

            let current = match read_net_dev() {
                Ok(stats) => stats,
                Err(error) => {
                    warnings.emit(
                        &log_tx,
                        LogEvent::warn(
                            LogSource::Network,
                            format!("failed to refresh /proc/net/dev: {error}"),
                        ),
                    );
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

    rates.sort_by(|a, b| natural_name_cmp(&a.name, &b.name));
    rates
}

fn natural_name_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut ai, mut bi) = (0, 0);
    while ai < a.len() && bi < b.len() {
        let a_digit = a[ai].is_ascii_digit();
        let b_digit = b[bi].is_ascii_digit();
        let mut a_end = ai + 1;
        let mut b_end = bi + 1;
        while a_end < a.len() && a[a_end].is_ascii_digit() == a_digit {
            a_end += 1;
        }
        while b_end < b.len() && b[b_end].is_ascii_digit() == b_digit {
            b_end += 1;
        }
        let ordering = if a_digit && b_digit {
            let a_number = &a[ai..a_end];
            let b_number = &b[bi..b_end];
            let a_trimmed = a_number
                .iter()
                .position(|byte| *byte != b'0')
                .unwrap_or(a_number.len());
            let b_trimmed = b_number
                .iter()
                .position(|byte| *byte != b'0')
                .unwrap_or(b_number.len());
            (a_number.len() - a_trimmed)
                .cmp(&(b_number.len() - b_trimmed))
                .then_with(|| a_number[a_trimmed..].cmp(&b_number[b_trimmed..]))
        } else {
            a[ai..a_end].cmp(&b[bi..b_end])
        };
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
        ai = a_end;
        bi = b_end;
    }
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
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

    interfaces.sort_by(|a, b| natural_name_cmp(&a.name, &b.name));
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
        let (members, members_available) = if kind == "bond" {
            read_bond_members(&path, &name)
        } else {
            (Vec::new(), false)
        };
        interfaces.push(NetInterface {
            name,
            kind: kind.to_string(),
            master,
            members,
            members_available,
            operstate: read_trimmed(path.join("operstate")),
            mtu: read_trimmed(path.join("mtu")).and_then(|value| value.parse().ok()),
        });
    }
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(interfaces)
}

#[cfg(test)]
mod tests {
    use super::natural_name_cmp;

    #[test]
    fn interface_names_use_natural_numeric_order() {
        let mut names = ["eth10", "eth2", "eno1", "eth1"];
        names.sort_by(|a, b| natural_name_cmp(a, b));
        assert_eq!(names, ["eno1", "eth1", "eth2", "eth10"]);
    }
}

fn read_bond_members(path: &std::path::Path, name: &str) -> (Vec<String>, bool) {
    let sysfs = path.join("bonding/slaves");
    if let Ok(value) = fs::read_to_string(sysfs) {
        return (value.split_whitespace().map(str::to_string).collect(), true);
    }
    match fs::read_to_string(format!("/proc/net/bonding/{name}")) {
        Ok(value) => (
            value
                .lines()
                .filter_map(|line| line.strip_prefix("Slave Interface: "))
                .map(str::to_string)
                .collect(),
            true,
        ),
        Err(_) => (Vec::new(), false),
    }
}

fn read_trimmed(path: impl AsRef<std::path::Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
