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
    log::{LogEvent, LogSource, WarningSuppressor},
    topology::{
        snapshot::{
            MappingConfidence, StorageHealth, StorageRedundancy, storage_health, storage_redundancy,
        },
        status::{
            Availability, CollectorDiagnostic, DiagnosticStage, LayerStatus, LayerStatuses,
            classify_diagnostic,
        },
        view::{Evidence, NodeKind, RelationKind, TopologyEdge, TopologyNode, TopologyUiState},
    },
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
pub struct FcRport {
    pub name: String,
    pub host: u32,
    pub channel: u32,
    pub identity: u32,
    pub node_wwn: Option<String>,
    pub port_wwn: Option<String>,
    pub state: Option<String>,
    pub roles: Option<String>,
    pub target_id: Option<i32>,
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
    pub health: StorageHealth,
    pub redundancy: StorageRedundancy,
    pub io: Option<DiskRate>,
    pub paths: Vec<MultipathPath>,
}

#[derive(Debug, Clone, Default)]
pub struct FcSnapshot {
    pub hosts: Vec<FcHost>,
    pub targets: Vec<FcTarget>,
    pub rports: Vec<FcRport>,
    pub maps: Vec<MultipathMap>,
    pub layers: LayerStatuses,
    pub diagnostics: Vec<CollectorDiagnostic>,
    pub topology: TopologyUiState,
}

pub fn spawn_fc_collector(output: Arc<Mutex<FcSnapshot>>, log_tx: Sender<LogEvent>) {
    thread::spawn(move || {
        let mut warnings = WarningSuppressor::default();
        let mut previous_disk = match disk::read_diskstats() {
            Ok(stats) => stats,
            Err(error) => {
                warnings.emit(
                    &log_tx,
                    LogEvent::warn(
                        LogSource::Disk,
                        format!("FC/SAN disk I/O unavailable: {error}"),
                    ),
                );
                Vec::new()
            }
        };

        loop {
            thread::sleep(Duration::from_secs(2));
            let current_disk = match disk::read_diskstats() {
                Ok(stats) => stats,
                Err(error) => {
                    warnings.emit(
                        &log_tx,
                        LogEvent::warn(
                            LogSource::Disk,
                            format!("failed to refresh FC/SAN disk I/O: {error}"),
                        ),
                    );
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
                    warnings.emit(
                        &log_tx,
                        LogEvent::warn(
                            LogSource::Storage,
                            format!("failed to refresh FC/SAN inventory: {error}"),
                        ),
                    );
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
    let mut layers = LayerStatuses::default();
    let mut diagnostics = Vec::new();
    let hosts = match read_fc_hosts() {
        Ok(hosts) => {
            layers.fc_hba = if hosts.is_empty() {
                LayerStatus::empty("no FC HBA hosts observed")
            } else {
                LayerStatus::live()
            };
            hosts
        }
        Err(error) => {
            let reason = error.to_string();
            layers.fc_hba = LayerStatus::error(reason.clone());
            diagnostics.push(classify_diagnostic("fc_hba", DiagnosticStage::Read, reason));
            Vec::new()
        }
    };
    let targets_result = read_fc_targets();
    let rports_result = read_fc_rports();
    let rports = match rports_result {
        Ok(rports) => rports,
        Err(error) => {
            diagnostics.push(classify_diagnostic(
                "fc_transport",
                DiagnosticStage::Read,
                error.to_string(),
            ));
            Vec::new()
        }
    };
    let mut maps = match read_multipath_maps() {
        Ok(maps) => {
            layers.multipath = if maps.is_empty() {
                LayerStatus::empty("no Compellent multipath maps observed")
            } else {
                LayerStatus::live()
            };
            maps
        }
        Err(error) => {
            let reason = error.to_string();
            layers.multipath = LayerStatus::error(reason.clone());
            diagnostics.push(classify_diagnostic(
                "multipath",
                DiagnosticStage::Execute,
                reason,
            ));
            Vec::new()
        }
    };
    let targets = match targets_result {
        Ok(targets) if targets.is_empty() && rports.is_empty() && !maps.is_empty() => {
            let reason = "FC transport returned no target records while host-side FC/multipath evidence exists; sysfs layout unresolved";
            layers.fc_transport = LayerStatus::unavailable(reason);
            diagnostics.push(CollectorDiagnostic {
                layer: "fc_transport",
                stage: DiagnosticStage::Empty,
                kind: crate::topology::status::DiagnosticKind::Unavailable,
                reason: reason.into(),
            });
            targets
        }
        Ok(targets) if targets.is_empty() && rports.is_empty() => {
            layers.fc_transport = LayerStatus::empty("no FC transport targets observed");
            targets
        }
        Ok(targets) => {
            layers.fc_transport = LayerStatus::live();
            targets
        }
        Err(error) if !rports.is_empty() => {
            let reason = format!(
                "FC remote-port inventory is live, but FC transport target records are unavailable: {error}"
            );
            layers.fc_transport = LayerStatus::partial(reason.clone());
            diagnostics.push(CollectorDiagnostic {
                layer: "fc_transport",
                stage: DiagnosticStage::Read,
                kind: crate::topology::status::DiagnosticKind::Partial,
                reason,
            });
            Vec::new()
        }
        Err(error) => {
            let reason = error.to_string();
            layers.fc_transport = LayerStatus::error(reason.clone());
            diagnostics.push(classify_diagnostic(
                "fc_transport",
                DiagnosticStage::Read,
                reason,
            ));
            Vec::new()
        }
    };

    for map in &mut maps {
        map.io = current_disk
            .iter()
            .find(|stat| stat.name == map.mapper)
            .and_then(|stat| disk::calculate_rate(previous_disk, stat, elapsed));
    }

    let topology = build_fc_topology(&hosts, &targets, &maps, &layers.multipath);
    Ok(FcSnapshot {
        hosts,
        targets,
        rports,
        maps,
        layers,
        diagnostics,
        topology,
    })
}

fn build_fc_topology(
    hosts: &[FcHost],
    targets: &[FcTarget],
    maps: &[MultipathMap],
    status: &LayerStatus,
) -> TopologyUiState {
    let mut topology = TopologyUiState {
        status: status.clone(),
        ..Default::default()
    };
    let mut seen = std::collections::HashSet::new();
    for map in maps {
        let map_id = format!("fc:map:{}", map.wwid);
        push_fc_node(
            &mut topology,
            &mut seen,
            TopologyNode {
                id: map_id.clone(),
                label: format!("{} ({})", map.wwid, map.mapper),
                kind: NodeKind::MultipathMap,
                domain_id: None,
                health: Some(map.health),
                redundancy: Some(map.redundancy),
                freshness: status.freshness,
                availability: Availability::Available,
                confidence: MappingConfidence::Exact,
                reason: None,
            },
        );
        for path in &map.paths {
            let path_id = format!("fc:path:{}:{}", map.wwid, path.hctl);
            push_fc_node(
                &mut topology,
                &mut seen,
                TopologyNode {
                    id: path_id.clone(),
                    label: format!(
                        "{} {} {} LUN {}",
                        path.hctl, path.device, path.major_minor, path.lun
                    ),
                    kind: NodeKind::FcPath,
                    domain_id: None,
                    health: None,
                    redundancy: None,
                    freshness: status.freshness,
                    availability: Availability::Available,
                    confidence: MappingConfidence::Exact,
                    reason: Some(path.state.clone()),
                },
            );
            topology.edges.push(TopologyEdge {
                from: map_id.clone(),
                to: path_id.clone(),
                relation: RelationKind::PathVia,
                evidence: Evidence::HostObserved,
                reason: None,
            });

            let host_name = format!("host{}", path.host);
            let host_id = format!("fc:host:{host_name}");
            let host = hosts.iter().find(|host| host.name == host_name);
            push_fc_node(
                &mut topology,
                &mut seen,
                TopologyNode {
                    id: host_id.clone(),
                    label: host
                        .and_then(|host| host.port_wwn.as_deref())
                        .map(|wwpn| format!("{host_name} / {wwpn}"))
                        .unwrap_or_else(|| host_name.clone()),
                    kind: NodeKind::FcHost,
                    domain_id: None,
                    health: None,
                    redundancy: None,
                    freshness: status.freshness,
                    availability: if host.is_some() {
                        Availability::Available
                    } else {
                        Availability::Unavailable
                    },
                    confidence: MappingConfidence::Exact,
                    reason: host
                        .is_none()
                        .then(|| "FC host metadata unavailable".into()),
                },
            );
            topology.edges.push(TopologyEdge {
                from: path_id.clone(),
                to: host_id,
                relation: RelationKind::PathVia,
                evidence: if host.is_some() {
                    Evidence::HostObserved
                } else {
                    Evidence::Unavailable
                },
                reason: None,
            });

            let target_id = format!("fc:target:{}:{}:{}", path.host, path.channel, path.target);
            let target = targets.iter().find(|target| {
                target.host == path.host
                    && target.channel == path.channel
                    && target.target == path.target
            });
            push_fc_node(
                &mut topology,
                &mut seen,
                TopologyNode {
                    id: target_id.clone(),
                    label: target
                        .and_then(|target| target.port_wwn.as_deref())
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            format!("target{}:{}:{}", path.host, path.channel, path.target)
                        }),
                    kind: if target.is_some() {
                        NodeKind::FcTarget
                    } else {
                        NodeKind::Unknown
                    },
                    domain_id: None,
                    health: None,
                    redundancy: None,
                    freshness: status.freshness,
                    availability: if target.is_some() {
                        Availability::Available
                    } else {
                        Availability::Unavailable
                    },
                    confidence: MappingConfidence::Exact,
                    reason: target
                        .is_none()
                        .then(|| "FC transport/target metadata unavailable".into()),
                },
            );
            topology.edges.push(TopologyEdge {
                from: path_id.clone(),
                to: target_id,
                relation: RelationKind::PathVia,
                evidence: if target.is_some() {
                    Evidence::HostObserved
                } else {
                    Evidence::Unavailable
                },
                reason: None,
            });
        }
    }
    topology.reasons = topology
        .nodes
        .iter()
        .filter_map(|node| node.reason.clone())
        .collect();
    topology
}

fn push_fc_node(
    topology: &mut TopologyUiState,
    seen: &mut std::collections::HashSet<String>,
    node: TopologyNode,
) {
    if seen.insert(node.id.clone()) {
        topology.nodes.push(node);
    }
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
    hosts.sort_by(|a, b| {
        fc_host_index(&a.name)
            .cmp(&fc_host_index(&b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
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
    targets.sort_by(|a, b| {
        (a.host, a.channel, a.target)
            .cmp(&(b.host, b.channel, b.target))
            .then_with(|| a.port_wwn.cmp(&b.port_wwn))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(targets)
}

pub fn read_fc_rports() -> Result<Vec<FcRport>> {
    let mut rports = Vec::new();
    let mut found_class = false;
    for class in ["/sys/class/fc_remote_ports", "/sys/class/fc_rport"] {
        let entries = match fs::read_dir(class) {
            Ok(entries) => {
                found_class = true;
                entries
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("read {class}")),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((host, channel, identity)) = parse_rport_name(&name) else {
                continue;
            };
            if rports.iter().any(|rport: &FcRport| rport.name == name) {
                continue;
            }
            let path = entry.path();
            rports.push(FcRport {
                name,
                host,
                channel,
                identity,
                node_wwn: read_attr(&path, "node_name"),
                port_wwn: read_attr(&path, "port_name"),
                state: read_attr(&path, "port_state"),
                roles: read_attr(&path, "roles"),
                target_id: read_attr(&path, "scsi_target_id")
                    .and_then(|value| value.parse().ok())
                    .filter(|target_id| *target_id >= 0),
            });
        }
    }
    if !found_class {
        anyhow::bail!("read /sys/class/fc_remote_ports: class directory not found")
    }
    rports.sort_by(|a, b| {
        (a.host, a.channel, a.identity)
            .cmp(&(b.host, b.channel, b.identity))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(rports)
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
    let mut map_headers = 0usize;

    for line in stdout.lines() {
        let columns: Vec<&str> = line.split_whitespace().collect();
        if !line.starts_with(char::is_whitespace)
            && columns.len() >= 3
            && columns[1].starts_with("dm-")
        {
            map_headers += 1;
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
    if !stdout.trim().is_empty() && map_headers == 0 {
        anyhow::bail!("parse failure: multipath output contained no recognized map headers");
    }
    maps.sort_by(|a, b| {
        a.wwid
            .cmp(&b.wwid)
            .then_with(|| dm_index(&a.mapper).cmp(&dm_index(&b.mapper)))
            .then_with(|| a.mapper.cmp(&b.mapper))
    });
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
    map.paths.sort_by(|a, b| {
        (a.host, a.channel, a.target, a.lun)
            .cmp(&(b.host, b.channel, b.target, b.lun))
            .then_with(|| a.device.cmp(&b.device))
            .then_with(|| a.hctl.cmp(&b.hctl))
    });
    map.active_paths = map
        .paths
        .iter()
        .filter(|path| path.state.contains("active") && path.state.contains("ready"))
        .count() as u32;
    map.health = storage_health(map.active_paths, map.total_paths);
    map.redundancy = storage_redundancy(map.active_paths, map.total_paths);
    maps.push(map);
}

fn fc_host_index(name: &str) -> u32 {
    name.strip_prefix("host")
        .and_then(|value| value.parse().ok())
        .unwrap_or(u32::MAX)
}

fn dm_index(name: &str) -> u32 {
    name.strip_prefix("dm-")
        .and_then(|value| value.parse().ok())
        .unwrap_or(u32::MAX)
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

fn parse_rport_name(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.strip_prefix("rport-")?.split([':', '-']);
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
        assert_eq!(parse_rport_name("rport-0:0-10"), Some((0, 0, 10)));
        assert_eq!(parse_rport_name("rport-1:0-35"), Some((1, 0, 35)));
        assert_eq!(parse_rport_name("target0:0:10"), None);
    }

    #[test]
    fn parses_rport_evidence_fields_without_inventing_target_relation() {
        let rport = FcRport {
            name: "rport-0:0-31".into(),
            host: 0,
            channel: 0,
            identity: 31,
            node_wwn: Some("0x20000024ff5d6cd4".into()),
            port_wwn: Some("0x21000024ff5d6cd4".into()),
            state: Some("Online".into()),
            roles: Some("FCP Initiator".into()),
            target_id: "-1".parse::<i32>().ok().filter(|id| *id >= 0),
        };
        assert_eq!(rport.state.as_deref(), Some("Online"));
        assert_eq!(rport.roles.as_deref(), Some("FCP Initiator"));
        assert_eq!(rport.target_id, None);
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

    fn fixture_map(vendor: &str, path_count: usize) -> MultipathMap {
        MultipathMap {
            vendor: vendor.into(),
            model: if vendor == "COMPELNT" {
                "Compellent Vol".into()
            } else {
                "PERC H710".into()
            },
            paths: (0..path_count)
                .map(|index| MultipathPath {
                    hctl: format!("8:0:0:{index}"),
                    state: "active ready running".into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn ap7_fixture_has_thirteen_single_and_four_redundant_compellent_maps() {
        let mut maps = Vec::new();
        for _ in 0..13 {
            push_compellent_map(&mut maps, Some(fixture_map("COMPELNT", 1)));
        }
        for _ in 0..4 {
            push_compellent_map(&mut maps, Some(fixture_map("COMPELNT", 2)));
        }
        push_compellent_map(&mut maps, Some(fixture_map("DELL", 1)));

        assert_eq!(maps.len(), 17);
        assert_eq!(
            maps.iter()
                .filter(|map| map.redundancy == StorageRedundancy::Single)
                .count(),
            13
        );
        assert_eq!(
            maps.iter()
                .filter(|map| map.redundancy == StorageRedundancy::Redundant)
                .count(),
            4
        );
    }

    #[test]
    fn ap8_fixture_has_seventeen_redundant_compellent_maps() {
        let mut maps = Vec::new();
        for _ in 0..17 {
            push_compellent_map(&mut maps, Some(fixture_map("COMPELNT", 2)));
        }
        push_compellent_map(&mut maps, Some(fixture_map("DELL", 1)));

        assert_eq!(maps.len(), 17);
        assert!(
            maps.iter()
                .all(|map| map.redundancy == StorageRedundancy::Redundant)
        );
    }

    #[test]
    fn two_online_hbas_do_not_change_single_path_map_redundancy() {
        let hosts = [
            FcHost {
                name: "host8".into(),
                port_state: Some("Online".into()),
                ..Default::default()
            },
            FcHost {
                name: "host9".into(),
                port_state: Some("Online".into()),
                ..Default::default()
            },
        ];
        let mut maps = Vec::new();
        push_compellent_map(&mut maps, Some(fixture_map("COMPELNT", 1)));

        assert_eq!(
            hosts
                .iter()
                .filter(|host| host.port_state.as_deref() == Some("Online"))
                .count(),
            2
        );
        assert_eq!(maps[0].redundancy, StorageRedundancy::Single);
    }

    #[test]
    fn missing_fc_transport_creates_unavailable_node_without_invented_wwpn() {
        let mut maps = Vec::new();
        let mut map = fixture_map("COMPELNT", 1);
        map.wwid = "3600abc".into();
        map.paths[0].host = 8;
        map.paths[0].channel = 0;
        map.paths[0].target = 6;
        push_compellent_map(&mut maps, Some(map));

        let topology = build_fc_topology(&[], &[], &maps, &LayerStatus::live());
        let target = topology
            .nodes
            .iter()
            .find(|node| node.id == "fc:target:8:0:6")
            .unwrap();

        assert_eq!(target.kind, NodeKind::Unknown);
        assert_eq!(target.availability, Availability::Unavailable);
        assert_eq!(target.label, "target8:0:6");
        assert!(target.reason.as_deref().unwrap().contains("unavailable"));
    }
}
