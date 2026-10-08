use crate::{
    collector::{block, fc, net, xenstore, xm},
    log::{LogEvent, LogSource, WarningSuppressor},
    topology::network::{
        EvidenceAvailability, NetworkEvidence, NetworkTopology, validate_network_topology,
    },
    topology::status::{
        Availability, CollectorDiagnostic, DiagnosticKind, DiagnosticStage, LayerStatus,
        LayerStatuses, classify_diagnostic,
    },
    topology::view::{TopologyUiState, build_network_ui, build_storage_ui},
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
    pub device: Option<String>,
    pub virtual_device: Option<String>,
    pub physical_device: Option<String>,
    pub backing_file: Option<String>,
    pub major_minor: Option<String>,
    pub host_device: Option<String>,
    pub dm_name: Option<String>,
    pub dm_uuid: Option<String>,
    pub wwid: Option<String>,
    pub confidence: MappingConfidence,
    pub unresolved_stage: Option<MappingStage>,
    pub unresolved_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingStage {
    Backend,
    PhysicalDevice,
    HostBlock,
    DeviceMapper,
    Wwid,
}

#[derive(Debug, Clone, Default)]
pub struct HostBlockDevice {
    pub name: String,
    pub kname: String,
    pub major_minor: Option<String>,
    pub device_type: String,
    pub size_bytes: Option<u64>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub wwn: Option<String>,
    pub mountpoint: Option<String>,
    pub dm_name: Option<String>,
    pub dm_uuid: Option<String>,
    pub holders: Vec<String>,
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
    pub members_available: bool,
    pub operstate: Option<String>,
    pub mtu: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct StorageIdentity {
    pub wwid: String,
    pub mapper: String,
    pub health: StorageHealth,
    pub redundancy: StorageRedundancy,
    pub scope: StorageScope,
}

/// Return the stable host-side storage identity used to join VBD and
/// multipath evidence.  Mapper aliases and kernel dm names are local display
/// metadata; the WWID is the canonical identity.
pub fn canonical_wwid(value: &str) -> String {
    let value = value.trim().strip_prefix("mpath-").unwrap_or(value.trim());
    value
        .strip_prefix("0x")
        .unwrap_or(value)
        .to_ascii_lowercase()
}

#[cfg(test)]
mod canonical_tests {
    use super::canonical_wwid;

    #[test]
    fn canonical_wwid_ignores_local_prefix_and_case() {
        assert_eq!(canonical_wwid(" mpath-0x3600ABC "), "3600abc");
        assert_eq!(canonical_wwid("3600ABC"), "3600abc");
    }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StorageRedundancy {
    Single,
    Redundant,
    Reduced,
    None,
    #[default]
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
    pub host_block_devices: Vec<HostBlockDevice>,
    pub block_inventory_source: Option<block::BlockInventorySource>,
    pub block_inventory_fallback_reason: Option<String>,
    pub vifs: Vec<VifMapping>,
    pub interfaces: Vec<NetInterface>,
    pub network: NetworkTopology,
    pub network_ui: TopologyUiState,
    pub storage: Vec<StorageIdentity>,
    pub storage_scope: StorageScope,
    pub layers: LayerStatuses,
    pub diagnostics: Vec<CollectorDiagnostic>,
    pub storage_ui: TopologyUiState,
}

pub fn storage_health(active_paths: u32, total_paths: u32) -> StorageHealth {
    match (active_paths, total_paths) {
        (active, total) if total > 0 && active == total => StorageHealth::Healthy,
        (active, total) if active > 0 && active < total => StorageHealth::Degraded,
        (0, total) if total > 0 => StorageHealth::Failed,
        _ => StorageHealth::Unknown,
    }
}

pub fn storage_redundancy(active_paths: u32, total_paths: u32) -> StorageRedundancy {
    match (active_paths, total_paths) {
        (0, 0) => StorageRedundancy::Unknown,
        (0, _) => StorageRedundancy::None,
        (1, 1) => StorageRedundancy::Single,
        (active, total) if active == total && total >= 2 => StorageRedundancy::Redundant,
        (active, total) if active > 0 && active < total => StorageRedundancy::Reduced,
        _ => StorageRedundancy::Unknown,
    }
}

pub fn spawn_collector(output: Arc<Mutex<TopologySnapshot>>, log_tx: Sender<LogEvent>) {
    thread::spawn(move || {
        let mut warnings = WarningSuppressor::default();
        loop {
            let inventory = match xm::get_domains() {
                Ok(domains) => domains,
                Err(error) => {
                    if let Ok(mut shared) = output.lock() {
                        shared.layers.inventory = LayerStatus {
                            freshness: crate::topology::status::Freshness::Stale,
                            availability: Availability::Error,
                            reason: Some(error.to_string()),
                        };
                        shared.meta.status = SnapshotStatus::Stale;
                    }
                    warnings.emit(
                        &log_tx,
                        LogEvent::warn(
                            LogSource::Domain,
                            format!("topology inventory unavailable: {error}"),
                        ),
                    );
                    thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };
            let mut snapshot = TopologySnapshot::default();
            let mut partial_failure = false;
            let mut used_fallback = false;
            let mut vbd_attempts = 0usize;
            let mut vbd_successes = 0usize;
            let mut vif_attempts = 0usize;
            let mut vif_successes = 0usize;
            snapshot.domains = inventory
                .iter()
                .map(|domain| DomainIdentity {
                    domid: domain.id,
                    name: domain.name.clone(),
                })
                .collect();
            snapshot.layers.inventory = if snapshot.domains.is_empty() {
                LayerStatus::empty("xm inventory returned no domains")
            } else {
                LayerStatus::live()
            };
            match block::read_block_inventory() {
                Ok(block_inventory) => {
                    used_fallback =
                        block_inventory.source == block::BlockInventorySource::SysfsFallback;
                    snapshot.block_inventory_source = Some(block_inventory.source);
                    snapshot.block_inventory_fallback_reason = block_inventory.fallback_reason;
                    snapshot.host_block_devices = block_inventory.devices;
                    snapshot.layers.block_inventory = if used_fallback {
                        let reason = snapshot
                            .block_inventory_fallback_reason
                            .clone()
                            .unwrap_or_else(|| "sysfs fallback used".into());
                        snapshot.diagnostics.push(classify_diagnostic(
                            "block_inventory",
                            DiagnosticStage::Capability,
                            reason.clone(),
                        ));
                        snapshot.diagnostics.push(CollectorDiagnostic {
                            layer: "block_inventory",
                            stage: DiagnosticStage::Capability,
                            kind: DiagnosticKind::CollectorIncompatible,
                            reason: format!(
                                "primary lsblk inventory was incompatible; sysfs fallback succeeded: {reason}"
                            ),
                        });
                        LayerStatus::incompatible(reason)
                    } else if snapshot.host_block_devices.is_empty() {
                        LayerStatus::empty("host block inventory is empty")
                    } else {
                        LayerStatus::live()
                    };
                }
                Err(error) => {
                    partial_failure = true;
                    let reason = error.to_string();
                    snapshot.block_inventory_fallback_reason = Some(reason.clone());
                    snapshot.layers.block_inventory = LayerStatus::error(reason.clone());
                    snapshot.diagnostics.push(classify_diagnostic(
                        "block_inventory",
                        DiagnosticStage::Read,
                        reason,
                    ));
                    warnings.emit(
                        &log_tx,
                        LogEvent::warn(
                            LogSource::Storage,
                            format!("failed to read host block inventory: {error}"),
                        ),
                    );
                }
            }
            for domain in &inventory {
                if domain.id == 0 {
                    continue;
                }
                vbd_attempts += 1;
                match xenstore::read_domain_block_devices(domain.id, &snapshot.host_block_devices) {
                    Ok(mut devices) => {
                        vbd_successes += 1;
                        snapshot.block_devices.append(&mut devices);
                    }
                    Err(error) => {
                        partial_failure = true;
                        snapshot.diagnostics.push(classify_diagnostic(
                            "vbd",
                            DiagnosticStage::Read,
                            error.to_string(),
                        ));
                        warnings.emit(
                            &log_tx,
                            LogEvent::warn(
                                LogSource::Domain,
                                format!(
                                    "failed to read VBD topology for dom{}: {error}",
                                    domain.id
                                ),
                            ),
                        );
                    }
                }
                vif_attempts += 1;
                match xenstore::read_domain_vifs(domain.id) {
                    Ok(mut vifs) => {
                        vif_successes += 1;
                        snapshot.vifs.append(&mut vifs);
                    }
                    Err(error) => {
                        partial_failure = true;
                        snapshot.diagnostics.push(classify_diagnostic(
                            "vif",
                            DiagnosticStage::Read,
                            error.to_string(),
                        ));
                        warnings.emit(
                            &log_tx,
                            LogEvent::warn(
                                LogSource::Network,
                                format!(
                                    "failed to read VIF topology for dom{}: {error}",
                                    domain.id
                                ),
                            ),
                        );
                    }
                }
            }
            if used_fallback {
                // Sysfs is useful host evidence, but it is not equivalent to a successful
                // lsblk collection. Do not promote those relationships to exact evidence.
                mark_fallback_mappings(&mut snapshot.block_devices);
            }
            let interface_evidence = match net::read_net_topology() {
                Ok(interfaces) => {
                    snapshot.interfaces = interfaces;
                    EvidenceAvailability::Available
                }
                Err(error) => {
                    partial_failure = true;
                    warnings.emit(
                        &log_tx,
                        LogEvent::warn(
                            LogSource::Network,
                            format!("failed to read network topology: {error}"),
                        ),
                    );
                    EvidenceAvailability::Error
                }
            };
            snapshot.layers.vbd = layer_from_attempts(
                vbd_attempts,
                vbd_successes,
                snapshot.block_devices.len(),
                "guest VBD topology is not applicable on Domain-0-only host",
                "VBD collector unavailable for all guest domains",
            );
            snapshot.layers.storage_mapping =
                storage_mapping_status(&snapshot.block_devices, &snapshot.layers.vbd);
            if vbd_attempts == 0 {
                snapshot.diagnostics.push(CollectorDiagnostic {
                    layer: "vbd",
                    stage: DiagnosticStage::Empty,
                    kind: DiagnosticKind::EmptyExpected,
                    reason: "Domain-0-only host: guest VBD topology is not applicable".into(),
                });
            }
            for device in snapshot
                .block_devices
                .iter()
                .filter(|device| device.unresolved_stage.is_some())
            {
                snapshot.diagnostics.push(CollectorDiagnostic {
                    layer: "storage_mapping",
                    stage: DiagnosticStage::Mapping,
                    kind: DiagnosticKind::Partial,
                    reason: format!(
                        "dom{} VBD {} unresolved at {:?}: {}",
                        device.domid,
                        device.frontend,
                        device.unresolved_stage,
                        device
                            .unresolved_reason
                            .as_deref()
                            .unwrap_or("reason unavailable")
                    ),
                });
            }
            let vif_evidence = match (vif_attempts, vif_successes) {
                (0, _) => EvidenceAvailability::Available,
                (attempts, successes) if attempts == successes => EvidenceAvailability::Available,
                (_, 0) => EvidenceAvailability::Unavailable,
                _ => EvidenceAvailability::Partial,
            };
            snapshot.layers.vif = layer_from_attempts(
                vif_attempts,
                vif_successes,
                snapshot.vifs.len(),
                "guest VIF topology is not applicable on Domain-0-only host",
                "VIF collector unavailable for all guest domains",
            );
            if vif_attempts == 0 {
                snapshot.diagnostics.push(CollectorDiagnostic {
                    layer: "vif",
                    stage: DiagnosticStage::Empty,
                    kind: DiagnosticKind::EmptyExpected,
                    reason: "Domain-0-only host: guest VIF topology is not applicable".into(),
                });
            }
            snapshot.layers.network_inventory = match interface_evidence {
                EvidenceAvailability::Available if snapshot.interfaces.is_empty() => {
                    LayerStatus::empty("host network interface inventory is empty")
                }
                EvidenceAvailability::Available => LayerStatus::live(),
                EvidenceAvailability::Partial => {
                    LayerStatus::partial("host network interface inventory is partial")
                }
                EvidenceAvailability::Unavailable => {
                    LayerStatus::unavailable("host network interface inventory unavailable")
                }
                EvidenceAvailability::Error => {
                    LayerStatus::error("host network interface collector failed")
                }
            };
            snapshot.network = validate_network_topology(
                &snapshot.domains,
                &snapshot.vifs,
                &snapshot.interfaces,
                NetworkEvidence {
                    vifs: vif_evidence,
                    interfaces: interface_evidence,
                },
            );
            snapshot.network_ui = build_network_ui(&snapshot.network);
            match fc::read_multipath_maps() {
                Ok(maps) => {
                    snapshot.layers.multipath = if maps.is_empty() {
                        LayerStatus::empty("no Compellent multipath maps observed")
                    } else {
                        LayerStatus::live()
                    };
                    snapshot.storage = maps
                        .into_iter()
                        .map(|map| StorageIdentity {
                            wwid: canonical_wwid(&map.wwid),
                            mapper: map.mapper,
                            health: storage_health(map.active_paths, map.total_paths),
                            redundancy: storage_redundancy(map.active_paths, map.total_paths),
                            scope: StorageScope::HostSide,
                        })
                        .collect();
                }
                Err(error) => {
                    partial_failure = true;
                    snapshot.layers.multipath = LayerStatus::error(error.to_string());
                    snapshot.diagnostics.push(classify_diagnostic(
                        "multipath",
                        DiagnosticStage::Execute,
                        error.to_string(),
                    ));
                    warnings.emit(
                        &log_tx,
                        LogEvent::warn(
                            LogSource::Storage,
                            format!("failed to read multipath topology: {error}"),
                        ),
                    );
                }
            }
            snapshot.storage_ui = build_storage_ui(
                &snapshot.block_devices,
                &snapshot.storage,
                &snapshot.layers.storage_mapping,
                &snapshot.layers.multipath,
            );
            snapshot.meta.collected_at = Some(Instant::now());
            snapshot.meta.generation = snapshot.meta.generation.saturating_add(1);
            snapshot.meta.status = if partial_failure {
                SnapshotStatus::Stale
            } else if used_fallback {
                SnapshotStatus::MergeFallback
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

fn layer_from_attempts(
    attempts: usize,
    successes: usize,
    rows: usize,
    empty_reason: &str,
    unavailable_reason: &str,
) -> LayerStatus {
    match (attempts, successes, rows) {
        (0, _, _) => LayerStatus::empty(empty_reason),
        (_, 0, _) => LayerStatus::unavailable(unavailable_reason),
        (attempts, successes, _) if attempts != successes => LayerStatus::partial(format!(
            "collector succeeded for {successes}/{attempts} guest domains"
        )),
        (_, _, 0) => LayerStatus::empty("collector completed with no rows"),
        _ => LayerStatus::live(),
    }
}

fn storage_mapping_status(devices: &[DomainBlockDevice], vbd_status: &LayerStatus) -> LayerStatus {
    if devices.is_empty() {
        return match vbd_status.availability {
            Availability::Empty => LayerStatus::empty(
                vbd_status
                    .reason
                    .clone()
                    .unwrap_or_else(|| "no guest VBD mappings".into()),
            ),
            Availability::Unavailable | Availability::Error => LayerStatus::unavailable(
                "storage mapping unavailable because VBD evidence is unavailable",
            ),
            _ => LayerStatus::empty("no VBD mappings to resolve"),
        };
    }
    let unresolved = devices
        .iter()
        .filter(|device| device.unresolved_stage.is_some())
        .count();
    if unresolved == 0 {
        LayerStatus::live()
    } else {
        LayerStatus::partial(format!(
            "{unresolved}/{} VBD mappings have unresolved stage/reason",
            devices.len()
        ))
    }
}

fn mark_fallback_mappings(devices: &mut [DomainBlockDevice]) {
    for device in devices {
        if device.confidence == MappingConfidence::Exact {
            device.confidence = MappingConfidence::Fallback;
        }
    }
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

    #[test]
    fn classifies_storage_redundancy_independently_from_health() {
        assert_eq!(storage_redundancy(1, 1), StorageRedundancy::Single);
        assert_eq!(storage_redundancy(2, 2), StorageRedundancy::Redundant);
        assert_eq!(storage_redundancy(1, 2), StorageRedundancy::Reduced);
        assert_eq!(storage_redundancy(0, 2), StorageRedundancy::None);
        assert_eq!(storage_redundancy(0, 0), StorageRedundancy::Unknown);
    }

    #[test]
    fn fresh_vbd_data_with_unresolved_mapping_is_partial_not_stale() {
        let devices = vec![DomainBlockDevice {
            domid: 1858,
            frontend: "51712".into(),
            unresolved_stage: Some(MappingStage::HostBlock),
            unresolved_reason: Some("no host block match".into()),
            ..Default::default()
        }];

        let status = storage_mapping_status(&devices, &LayerStatus::live());

        assert_eq!(status.freshness, crate::topology::status::Freshness::Live);
        assert_eq!(status.availability, Availability::Partial);
    }

    #[test]
    fn sysfs_fallback_downgrades_exact_mapping_without_erasing_unresolved_evidence() {
        let mut devices = vec![
            DomainBlockDevice {
                confidence: MappingConfidence::Exact,
                ..Default::default()
            },
            DomainBlockDevice {
                confidence: MappingConfidence::Unknown,
                unresolved_stage: Some(MappingStage::Wwid),
                unresolved_reason: Some("WWID unavailable".into()),
                ..Default::default()
            },
        ];

        mark_fallback_mappings(&mut devices);

        assert_eq!(devices[0].confidence, MappingConfidence::Fallback);
        assert_eq!(devices[1].confidence, MappingConfidence::Unknown);
        assert_eq!(devices[1].unresolved_stage, Some(MappingStage::Wwid));
        assert_eq!(
            devices[1].unresolved_reason.as_deref(),
            Some("WWID unavailable")
        );
    }

    #[test]
    fn ap8_fixture_keeps_six_vbd_unresolved_reasons_without_claiming_exact_mapping() {
        let devices = [
            (1859, "51712"),
            (1858, "51712"),
            (1858, "51728"),
            (1860, "51712"),
            (1860, "51728"),
            (1862, "51712"),
        ]
        .into_iter()
        .map(|(domid, frontend)| DomainBlockDevice {
            domid,
            frontend: frontend.into(),
            virtual_device: Some(frontend.into()),
            unresolved_stage: Some(MappingStage::PhysicalDevice),
            unresolved_reason: Some(
                "backend has neither physical-device nor params host evidence".into(),
            ),
            ..Default::default()
        })
        .collect::<Vec<_>>();

        let status = storage_mapping_status(&devices, &LayerStatus::live());

        assert_eq!(status.freshness, crate::topology::status::Freshness::Live);
        assert_eq!(status.availability, Availability::Partial);
        assert!(status.reason.as_deref().unwrap().contains("6/6"));
        assert!(devices.iter().all(|device| {
            device.confidence == MappingConfidence::Unknown
                && device.unresolved_stage.is_some()
                && device.unresolved_reason.is_some()
                && device.wwid.is_none()
        }));
    }
}
