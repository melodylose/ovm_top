use crate::collector::xm::XmDomain;
use crate::log::{LogEvent, LogSource};
use crate::topology::snapshot::{IdentitySource, SnapshotMeta, SnapshotStatus};
use anyhow::{Context, Result};
use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
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

#[derive(Debug, Clone)]
pub struct DomainView {
    pub name: String,
    pub id: u32,
    pub state: String,
    pub memory_mb: u64,
    pub vcpus: u32,
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub net_tx_kb: f64,
    pub net_rx_kb: f64,
    pub vbd_rd: u64,
    pub vbd_wr: u64,
    pub identity_source: IdentitySource,
}

pub type DomainSnapshotMeta = SnapshotMeta;

pub fn spawn_domain_view_collector(
    realtime: Arc<Mutex<Vec<DomainStats>>>,
    output: Arc<Mutex<Vec<DomainView>>>,
    meta: Arc<Mutex<DomainSnapshotMeta>>,
    log_tx: std::sync::mpsc::Sender<LogEvent>,
) {
    thread::spawn(move || {
        let mut last_merge_diagnostic = None;
        loop {
            let inventory = match crate::collector::xm::get_domains() {
                Ok(v) => v,
                Err(_) => {
                    let _ = log_tx.send(LogEvent::warn(
                        LogSource::Domain,
                        "failed to refresh domain inventory with xm list",
                    ));
                    thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };

            let realtime_snapshot = match realtime.lock() {
                Ok(v) => v.clone(),
                Err(_) => {
                    let _ = log_tx.send(LogEvent::warn(
                        LogSource::Domain,
                        "failed to read xentop domain snapshot",
                    ));
                    thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };

            let (merged, fallback_matches, unmatched_realtime, unmatched_inventory) =
                merge_domains_with_diagnostics(&inventory, &realtime_snapshot);
            if let Ok(mut snapshot_meta) = meta.lock() {
                snapshot_meta.status = if realtime_snapshot.is_empty() {
                    SnapshotStatus::NoData
                } else if fallback_matches > 0 || unmatched_realtime > 0 || unmatched_inventory > 0
                {
                    SnapshotStatus::MergeFallback
                } else {
                    SnapshotStatus::Live
                };
            }
            let diagnostic = (fallback_matches, unmatched_realtime, unmatched_inventory);
            if last_merge_diagnostic != Some(diagnostic) {
                let event =
                    if fallback_matches > 0 || unmatched_realtime > 0 || unmatched_inventory > 0 {
                        LogEvent::warn(
                            LogSource::Domain,
                            format!(
                                "domain merge used {fallback_matches} order fallbacks; \
                             {unmatched_inventory} inventory rows and \
                             {unmatched_realtime} realtime rows unmatched"
                            ),
                        )
                    } else {
                        LogEvent::info(
                            LogSource::Domain,
                            format!(
                                "domain merge healthy: inventory={} realtime={}",
                                inventory.len(),
                                realtime_snapshot.len()
                            ),
                        )
                    };
                let _ = log_tx.send(event);
                last_merge_diagnostic = Some(diagnostic);
            }

            if let Ok(mut shared) = output.lock() {
                *shared = merged;
            }

            thread::sleep(Duration::from_secs(2));
        }
    });
}

pub fn merge_domains(inventory: &[XmDomain], realtime: &[DomainStats]) -> Vec<DomainView> {
    merge_domains_with_diagnostics(inventory, realtime).0
}

pub fn merge_domains_with_diagnostics(
    inventory: &[XmDomain],
    realtime: &[DomainStats],
) -> (Vec<DomainView>, usize, usize, usize) {
    let mut used = vec![false; realtime.len()];
    let mut fallback_matches = 0;
    let mut matched_inventory = 0;
    let domains = inventory
        .iter()
        .enumerate()
        .map(|(index, xm)| {
            // Some xentop versions truncate NAME to a fixed-width column. In
            // that case several domains can have the same parsed name (or no
            // name match at all), so retain the stream order as a fallback.
            let named_match = realtime
                .iter()
                .enumerate()
                .find(|(position, x)| !used[*position] && x.name == xm.name)
                .map(|(position, _)| position);
            let mut identity_source = IdentitySource::Unknown;
            let matched_index = named_match.or_else(|| {
                let fallback = (index < realtime.len() && !used[index]).then_some(index);
                if fallback.is_some() {
                    fallback_matches += 1;
                    identity_source = IdentitySource::StreamOrder;
                }
                fallback
            });
            if named_match.is_some() {
                identity_source = IdentitySource::FullName;
            }
            if let Some(position) = matched_index {
                used[position] = true;
                matched_inventory += 1;
            }
            let rt = matched_index.and_then(|position| realtime.get(position));

            DomainView {
                name: xm.name.clone(),
                id: xm.id,
                state: xm.state.clone(),

                memory_mb: xm.memory_mb,
                vcpus: xm.vcpus,

                cpu_percent: rt.map(|x| x.cpu_percent).unwrap_or_default(),

                memory_percent: rt.map(|x| x.memory_percent).unwrap_or_default(),

                net_tx_kb: rt.map(|x| x.net_tx_kb).unwrap_or_default(),

                net_rx_kb: rt.map(|x| x.net_rx_kb).unwrap_or_default(),

                vbd_rd: rt.map(|x| x.vbd_rd).unwrap_or_default(),

                vbd_wr: rt.map(|x| x.vbd_wr).unwrap_or_default(),
                identity_source,
            }
        })
        .collect::<Vec<_>>();
    let unmatched_realtime = used.iter().filter(|used| !**used).count();
    let unmatched_inventory = inventory.len().saturating_sub(matched_inventory);
    (
        domains,
        fallback_matches,
        unmatched_realtime,
        unmatched_inventory,
    )
}

pub fn spawn_collector(
    domains: Arc<Mutex<Vec<DomainStats>>>,
    meta: Arc<Mutex<DomainSnapshotMeta>>,
    log_tx: std::sync::mpsc::Sender<LogEvent>,
) -> Result<()> {
    let mut child = Command::new("xentop")
        .args(["-b", "-f", "-d", "1"])
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to start xentop")?;

    let stdout = child
        .stdout
        .take()
        .context("failed to capture xentop stdout")?;

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut snapshot = Vec::new();
        let mut duplicate_warning_sent = false;

        for line in reader.lines() {
            let Ok(line) = line else {
                let _ = log_tx.send(LogEvent::warn(
                    LogSource::Domain,
                    "xentop output stream ended",
                ));
                break;
            };

            if is_header(&line) {
                if !snapshot.is_empty() {
                    publish_snapshot(
                        &domains,
                        &meta,
                        &mut snapshot,
                        &log_tx,
                        &mut duplicate_warning_sent,
                    );
                }
                continue;
            }

            if let Some(stats) = parse_line(&line) {
                snapshot.push(stats);
            }
        }

        if !snapshot.is_empty() {
            publish_snapshot(
                &domains,
                &meta,
                &mut snapshot,
                &log_tx,
                &mut duplicate_warning_sent,
            );
        }
    });

    Ok(())
}

pub fn get_domains_once() -> Result<Vec<DomainStats>> {
    let output = Command::new("sudo")
        .args([
            "env",
            "TERM=xterm",
            "xentop",
            "-b",
            "-f",
            "-d",
            "1",
            "-i",
            "2",
        ])
        .output()
        .context("failed to execute xentop")?;

    if !output.status.success() {
        anyhow::bail!("xentop failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut latest = Vec::new();
    let mut snapshot = Vec::new();

    for line in stdout.lines() {
        if is_header(line) {
            if !snapshot.is_empty() {
                latest = std::mem::take(&mut snapshot);
            }
            continue;
        }
        if let Some(stats) = parse_line(line) {
            snapshot.push(stats);
        }
    }
    if !snapshot.is_empty() {
        latest = snapshot;
    }

    Ok(latest)
}

fn is_header(line: &str) -> bool {
    line.split_whitespace().next() == Some("NAME")
}

fn publish_snapshot(
    output: &Arc<Mutex<Vec<DomainStats>>>,
    meta: &Arc<Mutex<DomainSnapshotMeta>>,
    snapshot: &mut Vec<DomainStats>,
    log_tx: &std::sync::mpsc::Sender<LogEvent>,
    duplicate_warning_sent: &mut bool,
) {
    let duplicate_names = snapshot
        .iter()
        .map(|domain| domain.name.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len()
        < snapshot.len();
    if duplicate_names && !*duplicate_warning_sent {
        let _ = log_tx.send(LogEvent::warn(
            LogSource::Domain,
            "xentop returned duplicate domain names; preserving stream order",
        ));
        *duplicate_warning_sent = true;
    }
    if let Ok(mut shared) = output.lock() {
        *shared = std::mem::take(snapshot);
    }
    if let Ok(mut snapshot_meta) = meta.lock() {
        snapshot_meta.collected_at = Some(std::time::Instant::now());
        snapshot_meta.generation = snapshot_meta.generation.saturating_add(1);
        snapshot_meta.status = SnapshotStatus::Live;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn realtime(name: &str, cpu_percent: f64) -> DomainStats {
        DomainStats {
            name: name.to_string(),
            cpu_percent,
            ..Default::default()
        }
    }

    #[test]
    fn preserves_duplicate_xentop_names_for_merge_order_fallback() {
        let inventory = vec![
            XmDomain {
                name: "domain-a".into(),
                id: 1,
                ..Default::default()
            },
            XmDomain {
                name: "domain-b".into(),
                id: 2,
                ..Default::default()
            },
        ];
        let realtime = vec![realtime("0004fb0000", 12.5), realtime("0004fb0000", 34.5)];

        let merged = merge_domains(&inventory, &realtime);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].cpu_percent, 12.5);
        assert_eq!(merged[1].cpu_percent, 34.5);
        assert_eq!(merged[0].identity_source, IdentitySource::StreamOrder);
        assert_eq!(merged[1].identity_source, IdentitySource::StreamOrder);
    }

    #[test]
    fn records_full_name_identity_when_realtime_name_matches_inventory() {
        let inventory = vec![XmDomain {
            name: "domain-a".into(),
            id: 7,
            ..Default::default()
        }];
        let realtime = vec![realtime("domain-a", 4.0)];

        let merged = merge_domains(&inventory, &realtime);

        assert_eq!(merged[0].identity_source, IdentitySource::FullName);
        assert_eq!(merged[0].id, 7);
        assert_eq!(merged[0].cpu_percent, 4.0);
    }

    #[test]
    fn parses_xentop_sample_without_collapsing_rows() {
        let first = "domain-a -----r 10 12.5 100 1.0 100 1.0 2 1 3 4 1 0 5 6 7 8 0";
        let second = "domain-a --b--- 20 34.5 200 2.0 200 2.0 4 2 6 8 2 0 9 10 11 12 0";

        assert!(parse_line(first).is_some());
        assert!(parse_line(second).is_some());
    }
}
