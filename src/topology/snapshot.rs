use crate::{
    collector::{fc, net, xenstore, xm},
    log::{LogEvent, LogSource},
};
use std::time::Instant;
use std::{
    sync::{Arc, Mutex, mpsc::Sender},
    thread,
    time::Duration,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DomainIdentity {
    pub domid: u32,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    FullName,
    StreamOrder,
    Unknown,
}

impl Default for IdentitySource {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotStatus {
    NoData,
    Live,
    Stale,
    MergeFallback,
    MergeError,
}

impl Default for SnapshotStatus {
    fn default() -> Self {
        Self::NoData
    }
}

#[derive(Debug, Clone, Default)]
pub struct SnapshotMeta {
    pub collected_at: Option<Instant>,
    pub generation: u64,
    pub status: SnapshotStatus,
}

#[derive(Debug, Clone, Default)]
pub struct DomainBlockDevice {
    pub domid: u32,
    pub frontend: String,
    pub backend: String,
    pub major_minor: Option<String>,
    pub device_name: Option<String>,
    pub wwid: Option<String>,
    pub confidence: MappingConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingConfidence {
    Exact,
    Derived,
    Fallback,
    Unknown,
}

impl Default for MappingConfidence {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Default)]
pub struct VifMapping {
    pub domid: u32,
    pub vif: String,
    pub mac: Option<String>,
    pub bridge: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NetInterface {
    pub name: String,
    pub kind: String,
    pub master: Option<String>,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StorageIdentity {
    pub wwid: String,
    pub mapper: String,
    pub health: StorageHealth,
    pub scope: StorageScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageScope {
    HostSide,
    ArraySideUnavailable,
}

impl Default for StorageScope {
    fn default() -> Self {
        Self::HostSide
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageHealth {
    Healthy,
    Degraded,
    Failed,
    Unknown,
}

impl Default for StorageHealth {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Default)]
pub struct TopologySnapshot {
    pub meta: SnapshotMeta,
    pub domains: Vec<DomainIdentity>,
    pub block_devices: Vec<DomainBlockDevice>,
    pub vifs: Vec<VifMapping>,
    pub interfaces: Vec<NetInterface>,
    pub storage: Vec<StorageIdentity>,
    pub storage_scope: StorageScope,
}

pub fn storage_health(active_paths: u32, total_paths: u32) -> StorageHealth {
    match (active_paths, total_paths) {
        (active, total) if total > 0 && active == total => StorageHealth::Healthy,
        (active, total) if active > 0 && active < total => StorageHealth::Degraded,
        (0, total) if total > 0 => StorageHealth::Failed,
        _ => StorageHealth::Unknown,
    }
}

pub fn spawn_collector(output: Arc<Mutex<TopologySnapshot>>, log_tx: Sender<LogEvent>) {
    thread::spawn(move || {
        loop {
            let inventory = match xm::get_domains() {
                Ok(domains) => domains,
                Err(error) => {
                    let _ = log_tx.send(LogEvent::warn(
                        LogSource::Domain,
                        format!("topology inventory unavailable: {error}"),
                    ));
                    thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };
            let mut snapshot = TopologySnapshot::default();
            let mut partial_failure = false;
            snapshot.domains = inventory
                .iter()
                .map(|domain| DomainIdentity {
                    domid: domain.id,
                    name: domain.name.clone(),
                })
                .collect();
            for domain in &inventory {
                match xenstore::read_domain_block_devices(domain.id) {
                    Ok(mut devices) => snapshot.block_devices.append(&mut devices),
                    Err(error) => {
                        partial_failure = true;
                        let _ = log_tx.send(LogEvent::warn(
                            LogSource::Domain,
                            format!("failed to read VBD topology for dom{}: {error}", domain.id),
                        ));
                    }
                }
                match xenstore::read_domain_vifs(domain.id) {
                    Ok(mut vifs) => snapshot.vifs.append(&mut vifs),
                    Err(error) => {
                        partial_failure = true;
                        let _ = log_tx.send(LogEvent::warn(
                            LogSource::Network,
                            format!("failed to read VIF topology for dom{}: {error}", domain.id),
                        ));
                    }
                }
            }
            match net::read_net_topology() {
                Ok(interfaces) => snapshot.interfaces = interfaces,
                Err(error) => {
                    partial_failure = true;
                    let _ = log_tx.send(LogEvent::warn(
                        LogSource::Network,
                        format!("failed to read network topology: {error}"),
                    ));
                }
            }
            match fc::read_multipath_maps() {
                Ok(maps) => {
                    snapshot.storage = maps
                        .into_iter()
                        .map(|map| StorageIdentity {
                            wwid: map.wwid,
                            mapper: map.mapper,
                            health: storage_health(map.active_paths, map.total_paths),
                            scope: StorageScope::HostSide,
                        })
                        .collect();
                }
                Err(error) => {
                    partial_failure = true;
                    let _ = log_tx.send(LogEvent::warn(
                        LogSource::Storage,
                        format!("failed to read multipath topology: {error}"),
                    ));
                }
            }
            snapshot.meta.collected_at = Some(Instant::now());
            snapshot.meta.generation = snapshot.meta.generation.saturating_add(1);
            snapshot.meta.status = if partial_failure {
                SnapshotStatus::Stale
            } else {
                SnapshotStatus::Live
            };
            if let Ok(mut shared) = output.lock() {
                *shared = snapshot;
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_storage_path_health() {
        assert_eq!(storage_health(2, 2), StorageHealth::Healthy);
        assert_eq!(storage_health(1, 2), StorageHealth::Degraded);
        assert_eq!(storage_health(0, 2), StorageHealth::Failed);
        assert_eq!(storage_health(0, 0), StorageHealth::Unknown);
    }
}
