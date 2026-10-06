use anyhow::{Context, Result};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{Arc, Mutex, mpsc::Sender},
    thread,
    time::Duration,
};

use crate::{
    collector::disk::{self, DiskRate, DiskStats},
    log::{LogEvent, LogSource},
};

#[derive(Debug, Clone, Default)]
pub struct FcHost {
    pub name: String,
    pub node_wwn: Option<String>,
    pub port_wwn: Option<String>,
    pub port_state: Option<String>,
    pub speed: Option<String>,
    pub fabric_name: Option<String>,
    pub symbolic_name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FcTarget {
    pub name: String,
    pub host: u32,
    pub channel: u32,
    pub target: u32,
    pub port_wwn: Option<String>,
    pub node_wwn: Option<String>,
    pub port_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MultipathPath {
    pub hctl: String,
    pub device: String,
    pub major_minor: String,
    pub host: u32,
    pub channel: u32,
    pub target: u32,
    pub lun: u64,
    pub state: String,
}

#[derive(Debug, Clone, Default)]
pub struct MultipathMap {
    pub wwid: String,
    pub mapper: String,
    pub vendor: String,
    pub model: String,
    pub size: String,
    pub policy: String,
    pub status: String,
    pub active_paths: u32,
    pub total_paths: u32,
    pub io: Option<DiskRate>,
    pub paths: Vec<MultipathPath>,
}

#[derive(Debug, Clone, Default)]
pub struct FcSnapshot {
    pub hosts: Vec<FcHost>,
    pub targets: Vec<FcTarget>,
    pub maps: Vec<MultipathMap>,
}

pub fn spawn_fc_collector(output: Arc<Mutex<FcSnapshot>>, log_tx: Sender<LogEvent>) {
    thread::spawn(move || {
        let mut previous_disk = match disk::read_diskstats() {
            Ok(stats) => stats,
            Err(error) => {
                let _ = log_tx.send(LogEvent::warn(
                    LogSource::Disk,
                    format!("FC/SAN disk I/O unavailable: {error}"),
                ));
                Vec::new()
            }
        };

        loop {
            thread::sleep(Duration::from_secs(2));
            let current_disk = match disk::read_diskstats() {
                Ok(stats) => stats,
                Err(error) => {
                    let _ = log_tx.send(LogEvent::warn(
                        LogSource::Disk,
                        format!("failed to refresh FC/SAN disk I/O: {error}"),
                    ));
                    continue;
                }
            };

            match collect_snapshot(&previous_disk, &current_disk, Duration::from_secs(2)) {
                Ok(snapshot) => {
                    if let Ok(mut shared) = output.lock() {
                        *shared = snapshot;
                    }
                }
                Err(error) => {
                    let _ = log_tx.send(LogEvent::warn(
                        LogSource::Domain,
                        format!("failed to refresh FC/SAN inventory: {error}"),
                    ));
                }
            }
            previous_disk = current_disk;
        }
    });
}

pub fn collect_snapshot(
    previous_disk: &[DiskStats],
    current_disk: &[DiskStats],
    elapsed: Duration,
) -> Result<FcSnapshot> {
    let hosts = read_fc_hosts()?;
    let targets = read_fc_targets()?;
    let mut maps = read_multipath_maps()?;

    for map in &mut maps {
        map.io = current_disk
            .iter()
            .find(|stat| stat.name == map.mapper)
            .and_then(|stat| disk::calculate_rate(previous_disk, stat, elapsed));
    }

    Ok(FcSnapshot {
        hosts,
        targets,
        maps,
    })
}

pub fn read_fc_hosts() -> Result<Vec<FcHost>> {
    let mut hosts = Vec::new();
    for entry in fs::read_dir("/sys/class/fc_host").context("read /sys/class/fc_host")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("host") {
            continue;
        }
        let path = entry.path();
        hosts.push(FcHost {
            name,
            node_wwn: read_attr(&path, "node_name"),
            port_wwn: read_attr(&path, "port_name"),
            port_state: read_attr(&path, "port_state"),
            speed: read_attr(&path, "speed"),
            fabric_name: read_attr(&path, "fabric_name"),
            symbolic_name: read_attr(&path, "symbolic_name"),
        });
    }
    hosts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(hosts)
}

pub fn read_fc_targets() -> Result<Vec<FcTarget>> {
    let mut targets = Vec::new();
    for entry in fs::read_dir("/sys/class/fc_transport").context("read /sys/class/fc_transport")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((host, channel, target)) = parse_target_name(&name) else {
            continue;
        };
        let path = entry.path();
        targets.push(FcTarget {
            name,
            host,
            channel,
            target,
            port_wwn: read_attr(&path, "port_name"),
            node_wwn: read_attr(&path, "node_name"),
            port_id: read_attr(&path, "port_id"),
        });
    }
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(targets)
}

pub fn read_multipath_maps() -> Result<Vec<MultipathMap>> {
    let direct = Command::new("multipath").arg("-ll").output();
    let output = match direct {
        Ok(output) if output.status.success() => output,
        _ => Command::new("sudo")
            .args(["-n", "multipath", "-ll"])
            .output()
            .context("failed to execute multipath -ll")?,
    };
    if !output.status.success() {
        anyhow::bail!(
            "multipath -ll failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut maps = Vec::new();
    let mut current: Option<MultipathMap> = None;

    for line in stdout.lines() {
        let columns: Vec<&str> = line.split_whitespace().collect();
        if !line.starts_with(char::is_whitespace)
            && columns.len() >= 3
            && columns[1].starts_with("dm-")
        {
            push_compellent_map(&mut maps, current.take());
            let (vendor, model) = columns[2]
                .split_once(',')
                .map_or((columns[2].to_string(), String::new()), |(v, m)| {
                    (v.to_string(), m.to_string())
                });
            current = Some(MultipathMap {
                wwid: columns[0].to_string(),
                mapper: columns[1].to_string(),
                vendor,
                model,
                ..Default::default()
            });
            continue;
        }

        let Some(map) = current.as_mut() else {
            continue;
        };
        if let Some(size) = columns.iter().find(|column| column.starts_with("size=")) {
            map.size = size.trim_start_matches("size=").to_string();
        }
        if let Some(policy) = extract_quoted_value(line, "policy=") {
            map.policy = policy;
        }
        if let Some(status) = extract_value(line, "status=") {
            map.status = status;
        }
        if let Some(path) = parse_path_line(&columns) {
            map.paths.push(path);
        }
    }
    push_compellent_map(&mut maps, current);
    Ok(maps)
}

fn push_compellent_map(maps: &mut Vec<MultipathMap>, map: Option<MultipathMap>) {
    let Some(mut map) = map else { return };
    if !map.vendor.eq_ignore_ascii_case("COMPELNT")
        && !map.model.to_ascii_lowercase().contains("compellent")
    {
        return;
    }
    map.total_paths = map.paths.len() as u32;
    map.active_paths = map
        .paths
        .iter()
        .filter(|path| path.state.contains("active") && path.state.contains("ready"))
        .count() as u32;
    maps.push(map);
}

fn parse_path_line(columns: &[&str]) -> Option<MultipathPath> {
    if columns.len() < 5 || !columns[0].starts_with(['|', '`']) {
        return None;
    }
    let hctl = columns[1];
    let (host, channel, target, lun) = parse_hctl(hctl)?;
    Some(MultipathPath {
        hctl: hctl.to_string(),
        device: columns[2].to_string(),
        major_minor: columns[3].to_string(),
        host,
        channel,
        target,
        lun,
        state: columns[4..].join(" "),
    })
}

fn parse_hctl(value: &str) -> Option<(u32, u32, u32, u64)> {
    let mut parts = value.split(':');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

fn parse_target_name(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.strip_prefix("target")?.split(':');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

fn extract_quoted_value(line: &str, key: &str) -> Option<String> {
    let start = line.find(key)? + key.len();
    let rest = &line[start..];
    let value = rest.strip_prefix('\'')?.split('\'').next()?;
    Some(value.to_string())
}

fn extract_value(line: &str, key: &str) -> Option<String> {
    let start = line.find(key)? + key.len();
    Some(line[start..].split_whitespace().next()?.to_string())
}

fn read_attr(path: &Path, name: &str) -> Option<String> {
    fs::read_to_string(path.join(name))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fc_identifiers() {
        assert_eq!(parse_target_name("target8:0:6"), Some((8, 0, 6)));
        assert_eq!(parse_hctl("8:0:6:4"), Some((8, 0, 6, 4)));
    }

    #[test]
    fn filters_non_compellent_maps() {
        let mut maps = Vec::new();
        push_compellent_map(
            &mut maps,
            Some(MultipathMap {
                vendor: "DELL".into(),
                model: "PERC H710".into(),
                ..Default::default()
            }),
        );
        assert!(maps.is_empty());
    }
}
