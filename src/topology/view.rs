use crate::topology::{
    network::{NetworkTopology, NetworkTopologyStatus},
    snapshot::{
        DomainBlockDevice, MappingConfidence, StorageHealth, StorageIdentity, StorageRedundancy,
        canonical_wwid,
    },
    status::{Availability, Freshness, LayerStatus},
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Domain,
    Vif,
    AuxiliaryVif,
    Bridge,
    Bond,
    PhysicalNic,
    BlockDevice,
    DeviceMapper,
    MultipathMap,
    FcHost,
    FcTarget,
    FcPath,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    Owns,
    AttachedTo,
    Uplink,
    MemberOf,
    AuxiliaryOf,
    MapsTo,
    PathVia,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    HostObserved,
    Derived,
    Fallback,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyNode {
    pub id: String,
    pub label: String,
    pub kind: NodeKind,
    pub domain_id: Option<u32>,
    pub health: Option<StorageHealth>,
    pub redundancy: Option<StorageRedundancy>,
    pub freshness: Freshness,
    pub availability: Availability,
    pub confidence: MappingConfidence,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyEdge {
    pub from: String,
    pub to: String,
    pub relation: RelationKind,
    pub evidence: Evidence,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TopologyUiState {
    pub status: LayerStatus,
    pub nodes: Vec<TopologyNode>,
    pub edges: Vec<TopologyEdge>,
    pub reasons: Vec<String>,
}

pub fn build_network_ui(topology: &NetworkTopology) -> TopologyUiState {
    let status = match topology.status {
        NetworkTopologyStatus::Complete => LayerStatus::live(),
        NetworkTopologyStatus::Empty => LayerStatus::empty("no guest network topology"),
        NetworkTopologyStatus::Unavailable => {
            LayerStatus::unavailable("network relationship evidence unavailable")
        }
        NetworkTopologyStatus::Partial => {
            LayerStatus::partial("network topology has unresolved relationships")
        }
        NetworkTopologyStatus::Error => LayerStatus::error("network topology collector failed"),
    };
    let mut state = TopologyUiState {
        status,
        reasons: topology
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.reason.clone())
            .collect(),
        ..Default::default()
    };
    let mut seen = HashSet::new();
    for chain in &topology.chains {
        let domain = format!("domain:{}", chain.domid);
        let vif = format!("net:{}", chain.vif);
        push_node(
            &mut state.nodes,
            &mut seen,
            node(
                domain.clone(),
                format!("DomID {}", chain.domid),
                NodeKind::Domain,
                Some(chain.domid),
                &state.status,
            ),
        );
        let mut vif_node = node(
            vif.clone(),
            chain.vif.clone(),
            NodeKind::Vif,
            Some(chain.domid),
            &state.status,
        );
        vif_node.reason = (!chain.diagnostics.is_empty()).then(|| chain.diagnostics.join("; "));
        push_node(&mut state.nodes, &mut seen, vif_node);
        state.edges.push(edge(
            domain,
            vif.clone(),
            RelationKind::Owns,
            Evidence::HostObserved,
        ));

        let Some(bridge_name) = chain.bridge.as_deref() else {
            continue;
        };
        let bridge = format!("net:{bridge_name}");
        push_node(
            &mut state.nodes,
            &mut seen,
            node(
                bridge.clone(),
                bridge_name.into(),
                NodeKind::Bridge,
                Some(chain.domid),
                &state.status,
            ),
        );
        state.edges.push(edge(
            vif.clone(),
            bridge.clone(),
            RelationKind::AttachedTo,
            Evidence::HostObserved,
        ));
        for auxiliary in &chain.auxiliary_interfaces {
            let auxiliary_id = format!("net:{auxiliary}");
            push_node(
                &mut state.nodes,
                &mut seen,
                node(
                    auxiliary_id.clone(),
                    auxiliary.clone(),
                    NodeKind::AuxiliaryVif,
                    Some(chain.domid),
                    &state.status,
                ),
            );
            state.edges.push(edge(
                auxiliary_id,
                vif.clone(),
                RelationKind::AuxiliaryOf,
                Evidence::Derived,
            ));
        }
        for uplink in &chain.uplinks {
            let uplink_id = format!("net:{uplink}");
            let kind = if chain.physical_nics.contains(uplink) {
                NodeKind::PhysicalNic
            } else {
                NodeKind::Bond
            };
            push_node(
                &mut state.nodes,
                &mut seen,
                node(
                    uplink_id.clone(),
                    uplink.clone(),
                    kind,
                    Some(chain.domid),
                    &state.status,
                ),
            );
            state.edges.push(edge(
                bridge.clone(),
                uplink_id.clone(),
                RelationKind::Uplink,
                Evidence::HostObserved,
            ));
            for (member_uplink, physical) in &chain.uplink_members {
                if member_uplink != uplink {
                    continue;
                }
                let physical_id = format!("net:{physical}");
                push_node(
                    &mut state.nodes,
                    &mut seen,
                    node(
                        physical_id.clone(),
                        physical.clone(),
                        NodeKind::PhysicalNic,
                        Some(chain.domid),
                        &state.status,
                    ),
                );
                state.edges.push(edge(
                    physical_id,
                    uplink_id.clone(),
                    RelationKind::MemberOf,
                    Evidence::HostObserved,
                ));
            }
        }
    }
    state
}

pub fn build_storage_ui(
    devices: &[DomainBlockDevice],
    storage: &[StorageIdentity],
    mapping_status: &LayerStatus,
    multipath_status: &LayerStatus,
) -> TopologyUiState {
    let topology_status = if devices.is_empty() && !storage.is_empty() {
        multipath_status.clone()
    } else {
        mapping_status.clone()
    };
    let mut state = TopologyUiState {
        status: topology_status,
        ..Default::default()
    };
    let storage_by_wwid: HashMap<String, &StorageIdentity> = storage
        .iter()
        .map(|identity| (canonical_wwid(&identity.wwid), identity))
        .collect();
    let mut seen = HashSet::new();
    for identity in storage {
        let wwid = canonical_wwid(&identity.wwid);
        let mut map = node(
            format!("storage:wwid:{wwid}"),
            format!("WWID {wwid} (dm alias {})", identity.mapper),
            NodeKind::MultipathMap,
            None,
            multipath_status,
        );
        map.health = Some(identity.health);
        map.redundancy = Some(identity.redundancy);
        map.confidence = if multipath_status.freshness == Freshness::Fallback {
            MappingConfidence::Fallback
        } else {
            MappingConfidence::Exact
        };
        map.availability = multipath_status.availability;
        push_node(&mut state.nodes, &mut seen, map);
    }
    for device in devices {
        let domain = format!("domain:{}", device.domid);
        let vbd = format!("vbd:{}:{}", device.domid, device.frontend);
        push_node(
            &mut state.nodes,
            &mut seen,
            node(
                domain.clone(),
                format!("DomID {}", device.domid),
                NodeKind::Domain,
                Some(device.domid),
                mapping_status,
            ),
        );
        let mut vbd_node = node(
            vbd.clone(),
            format!("VBD {}", device.frontend),
            NodeKind::BlockDevice,
            Some(device.domid),
            mapping_status,
        );
        vbd_node.confidence = device.confidence;
        vbd_node.reason = device.unresolved_reason.clone();
        if device.unresolved_stage.is_some() {
            vbd_node.availability = Availability::Partial;
        }
        push_node(&mut state.nodes, &mut seen, vbd_node);
        state.edges.push(edge(
            domain,
            vbd.clone(),
            RelationKind::Owns,
            Evidence::HostObserved,
        ));

        let Some(host_name) = device.host_device.as_deref() else {
            continue;
        };
        let host = format!("block:{host_name}");
        let mut host_node = node(
            host.clone(),
            host_name.into(),
            NodeKind::BlockDevice,
            Some(device.domid),
            mapping_status,
        );
        host_node.confidence = device.confidence;
        if device.unresolved_stage.is_some() {
            host_node.availability = Availability::Partial;
            host_node.reason = device.unresolved_reason.clone();
        }
        push_node(&mut state.nodes, &mut seen, host_node);
        state.edges.push(edge(
            vbd,
            host.clone(),
            RelationKind::MapsTo,
            host_relation_evidence(device.confidence),
        ));
        let unresolved = device.unresolved_stage.is_some();
        let Some(dm_name) = device.dm_name.as_deref() else {
            continue;
        };
        let dm = format!("dm:{dm_name}");
        push_node(
            &mut state.nodes,
            &mut seen,
            node(
                dm.clone(),
                dm_name.into(),
                NodeKind::DeviceMapper,
                Some(device.domid),
                mapping_status,
            ),
        );
        if let Some(dm_node) = state.nodes.iter_mut().find(|node| node.id == dm) {
            dm_node.confidence = device.confidence;
            if unresolved {
                dm_node.availability = Availability::Partial;
                dm_node.reason = device.unresolved_reason.clone();
            }
        }
        state.edges.push(edge(
            host,
            dm.clone(),
            RelationKind::MapsTo,
            host_relation_evidence(device.confidence),
        ));
        let Some(raw_wwid) = device.wwid.as_deref() else {
            continue;
        };
        let wwid = canonical_wwid(raw_wwid);
        let map_id = format!("storage:wwid:{wwid}");
        if !storage_by_wwid.contains_key(&wwid) {
            let mut unknown = node(
                map_id.clone(),
                wwid.into(),
                NodeKind::MultipathMap,
                Some(device.domid),
                mapping_status,
            );
            unknown.availability = Availability::Unavailable;
            unknown.confidence = MappingConfidence::Unknown;
            unknown.reason = Some("WWID has no matching host multipath inventory".into());
            push_node(&mut state.nodes, &mut seen, unknown);
            // A WWID without matching host inventory is not a validated storage relation.
            continue;
        }
        state.edges.push(edge(
            dm,
            map_id,
            RelationKind::MapsTo,
            mapping_evidence(device.confidence),
        ));
    }
    state.reasons = devices
        .iter()
        .filter_map(|device| device.unresolved_reason.clone())
        .collect();
    state
}

fn node(
    id: String,
    label: String,
    kind: NodeKind,
    domain_id: Option<u32>,
    status: &LayerStatus,
) -> TopologyNode {
    TopologyNode {
        id,
        label,
        kind,
        domain_id,
        health: None,
        redundancy: None,
        freshness: status.freshness,
        availability: Availability::Available,
        confidence: MappingConfidence::Exact,
        reason: None,
    }
}

fn edge(from: String, to: String, relation: RelationKind, evidence: Evidence) -> TopologyEdge {
    TopologyEdge {
        from,
        to,
        relation,
        evidence,
        reason: None,
    }
}

fn mapping_evidence(confidence: MappingConfidence) -> Evidence {
    match confidence {
        MappingConfidence::Exact => Evidence::HostObserved,
        MappingConfidence::Derived => Evidence::Derived,
        MappingConfidence::Fallback => Evidence::Fallback,
        MappingConfidence::Unknown => Evidence::Unavailable,
    }
}

fn host_relation_evidence(confidence: MappingConfidence) -> Evidence {
    match confidence {
        // The explicit host_device field is itself host-observed relation evidence;
        // Unknown here means a later mapping stage failed, not that this edge was guessed.
        MappingConfidence::Unknown => Evidence::HostObserved,
        _ => mapping_evidence(confidence),
    }
}

fn push_node(nodes: &mut Vec<TopologyNode>, seen: &mut HashSet<String>, node: TopologyNode) {
    if seen.insert(node.id.clone()) {
        nodes.push(node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{network::NetworkChain, snapshot::MappingStage};

    #[test]
    fn unresolved_storage_does_not_create_guessed_mapping_edges() {
        let devices = vec![DomainBlockDevice {
            domid: 1858,
            frontend: "51712".into(),
            unresolved_stage: Some(MappingStage::HostBlock),
            unresolved_reason: Some("no host block inventory match".into()),
            ..Default::default()
        }];
        let state = build_storage_ui(
            &devices,
            &[],
            &LayerStatus::partial("mapping incomplete"),
            &LayerStatus::empty("no multipath maps"),
        );

        assert_eq!(state.edges.len(), 1);
        assert_eq!(state.edges[0].relation, RelationKind::Owns);
        assert!(state.nodes.iter().any(|node| {
            node.kind == NodeKind::BlockDevice
                && node.availability == Availability::Partial
                && node.reason.is_some()
        }));
    }

    #[test]
    fn network_view_uses_validated_chains_and_marks_auxiliary_relation() {
        let topology = NetworkTopology {
            status: NetworkTopologyStatus::Complete,
            primary_vif_count: 1,
            auxiliary_vif_count: 1,
            chains: vec![NetworkChain {
                domid: 7,
                vif_index: Some(0),
                vif: "vif7.0".into(),
                bridge: Some("br0".into()),
                uplinks: vec!["bond0".into()],
                physical_nics: vec!["eth0".into()],
                uplink_members: vec![("bond0".into(), "eth0".into())],
                auxiliary_interfaces: vec!["vif7.0-emu".into()],
                diagnostics: Vec::new(),
            }],
            diagnostics: Vec::new(),
        };

        let state = build_network_ui(&topology);

        assert!(state.edges.iter().any(|edge| {
            edge.relation == RelationKind::AuxiliaryOf && edge.evidence == Evidence::Derived
        }));
        assert!(
            state
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::PhysicalNic)
        );
    }

    #[test]
    fn network_view_does_not_cross_join_uplinks_and_physical_nics() {
        let topology = NetworkTopology {
            status: NetworkTopologyStatus::Complete,
            chains: vec![NetworkChain {
                domid: 7,
                vif: "vif7.0".into(),
                bridge: Some("br0".into()),
                uplinks: vec!["bond0".into(), "bond1".into()],
                physical_nics: vec!["em1".into(), "em2".into()],
                uplink_members: vec![("bond0".into(), "em1".into())],
                ..Default::default()
            }],
            ..Default::default()
        };

        let state = build_network_ui(&topology);
        let member_edges = state
            .edges
            .iter()
            .filter(|edge| edge.relation == RelationKind::MemberOf)
            .collect::<Vec<_>>();

        assert_eq!(member_edges.len(), 1);
        assert_eq!(member_edges[0].from, "net:em1");
        assert_eq!(member_edges[0].to, "net:bond0");
    }

    #[test]
    fn network_view_does_not_attach_vif_to_unvalidated_bridge() {
        let topology = NetworkTopology {
            status: NetworkTopologyStatus::Partial,
            chains: vec![NetworkChain {
                domid: 7,
                vif: "vif7.0".into(),
                diagnostics: vec!["bridge br0 is unavailable".into()],
                ..Default::default()
            }],
            ..Default::default()
        };

        let state = build_network_ui(&topology);

        assert!(
            !state
                .edges
                .iter()
                .any(|edge| edge.relation == RelationKind::AttachedTo)
        );
    }

    #[test]
    fn empty_guest_network_is_distinct_from_unavailable_inventory() {
        let state = build_network_ui(&NetworkTopology {
            status: NetworkTopologyStatus::Empty,
            ..Default::default()
        });

        assert_eq!(state.status.availability, Availability::Empty);
        assert_eq!(state.status.freshness, Freshness::Live);
        assert!(state.nodes.is_empty());
    }

    #[test]
    fn multipath_inventory_remains_visible_when_guest_vbd_topology_is_empty() {
        let storage = vec![StorageIdentity {
            wwid: "3600abc".into(),
            mapper: "dm-3".into(),
            health: StorageHealth::Healthy,
            redundancy: StorageRedundancy::Single,
            ..Default::default()
        }];
        let state = build_storage_ui(
            &[],
            &storage,
            &LayerStatus::empty("Domain-0-only"),
            &LayerStatus::live(),
        );

        assert_eq!(state.status.availability, Availability::Available);
        assert_eq!(state.nodes.len(), 1);
        assert_eq!(state.nodes[0].redundancy, Some(StorageRedundancy::Single));
    }

    #[test]
    fn unresolved_host_and_dm_nodes_are_not_presented_as_exact_available() {
        let devices = vec![DomainBlockDevice {
            domid: 7,
            frontend: "51712".into(),
            host_device: Some("dm-3".into()),
            dm_name: Some("mpatha".into()),
            confidence: MappingConfidence::Unknown,
            unresolved_stage: Some(MappingStage::Wwid),
            unresolved_reason: Some("no host-side WWID evidence".into()),
            ..Default::default()
        }];
        let state = build_storage_ui(
            &devices,
            &[],
            &LayerStatus::partial("mapping incomplete"),
            &LayerStatus::empty("no multipath maps"),
        );

        for id in ["block:dm-3", "dm:mpatha"] {
            let node = state.nodes.iter().find(|node| node.id == id).unwrap();
            assert_eq!(node.confidence, MappingConfidence::Unknown);
            assert_eq!(node.availability, Availability::Partial);
            assert!(node.reason.is_some());
        }
    }

    #[test]
    fn file_backed_loop_unresolved_reason_is_visible_without_storage_mapping() {
        let reason = "host block/loop loop0 and backing file /OVS/Repositories/disk.img are observed, but no device-mapper/WWID evidence is available";
        let devices = vec![DomainBlockDevice {
            domid: 7,
            frontend: "51712".into(),
            physical_device: Some("7:0".into()),
            major_minor: Some("7:0".into()),
            host_device: Some("loop0".into()),
            unresolved_stage: Some(MappingStage::DeviceMapper),
            unresolved_reason: Some(reason.into()),
            ..Default::default()
        }];

        let state = build_storage_ui(
            &devices,
            &[],
            &LayerStatus::partial("mapping incomplete"),
            &LayerStatus::empty("no multipath maps"),
        );

        assert!(state.edges.iter().any(|edge| {
            edge.from == "vbd:7:51712"
                && edge.to == "block:loop0"
                && edge.evidence == Evidence::HostObserved
        }));
        assert!(
            !state
                .edges
                .iter()
                .any(|edge| edge.relation == RelationKind::MapsTo
                    && edge.to.starts_with("storage:wwid:"))
        );
        assert!(
            state
                .reasons
                .iter()
                .any(|value| value.contains("backing file"))
        );
    }

    #[test]
    fn unmatched_wwid_does_not_create_validated_storage_edge() {
        let devices = vec![DomainBlockDevice {
            domid: 7,
            frontend: "51712".into(),
            host_device: Some("dm-3".into()),
            dm_name: Some("mpatha".into()),
            wwid: Some("3600unknown".into()),
            confidence: MappingConfidence::Fallback,
            ..Default::default()
        }];
        let state = build_storage_ui(
            &devices,
            &[],
            &LayerStatus::partial("mapping from sysfs fallback"),
            &LayerStatus::live(),
        );

        assert!(!state.edges.iter().any(|edge| {
            edge.relation == RelationKind::MapsTo && edge.to == "storage:wwid:3600unknown"
        }));
        assert_eq!(
            state
                .nodes
                .iter()
                .find(|node| node.id == "storage:wwid:3600unknown")
                .unwrap()
                .availability,
            Availability::Unavailable
        );
    }
}
