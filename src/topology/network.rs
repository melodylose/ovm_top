use crate::topology::snapshot::{DomainIdentity, NetInterface, VifMapping};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceAvailability {
    Available,
    Partial,
    Unavailable,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkEvidence {
    pub vifs: EvidenceAvailability,
    pub interfaces: EvidenceAvailability,
}

impl Default for NetworkEvidence {
    fn default() -> Self {
        Self {
            vifs: EvidenceAvailability::Unavailable,
            interfaces: EvidenceAvailability::Unavailable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkTopologyStatus {
    Complete,
    Empty,
    Unavailable,
    Partial,
    Error,
}

impl Default for NetworkTopologyStatus {
    fn default() -> Self {
        Self::Unavailable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkAnomaly {
    VifWithoutBridge,
    BridgeWithoutUplink,
    BondWithoutSlave,
    PhysicalNicDown,
    OrphanEmulatedVif,
    DuplicateEmulatedVif,
    UnknownParent,
    MtuMismatch,
    OperstateMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkDiagnostic {
    pub anomaly: NetworkAnomaly,
    pub domid: Option<u32>,
    pub interface: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkChain {
    pub domid: u32,
    pub vif_index: Option<u32>,
    pub vif: String,
    pub bridge: Option<String>,
    pub uplinks: Vec<String>,
    pub physical_nics: Vec<String>,
    /// Validated bond/member relations. `physical_nics` is intentionally kept
    /// as a summary, but must not be used to infer these edges.
    pub uplink_members: Vec<(String, String)>,
    pub auxiliary_interfaces: Vec<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkTopology {
    pub status: NetworkTopologyStatus,
    pub primary_vif_count: usize,
    pub auxiliary_vif_count: usize,
    pub chains: Vec<NetworkChain>,
    pub diagnostics: Vec<NetworkDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VifRole {
    Primary,
    Auxiliary,
}

fn parse_vif_name(name: &str) -> Option<(u32, u32, VifRole)> {
    let value = name.strip_prefix("vif")?;
    let (identity, role) = value
        .strip_suffix("-emu")
        .map(|value| (value, VifRole::Auxiliary))
        .unwrap_or((value, VifRole::Primary));
    let (domid, index) = identity.split_once('.')?;
    Some((domid.parse().ok()?, index.parse().ok()?, role))
}

pub fn validate_network_topology(
    domains: &[DomainIdentity],
    vifs: &[VifMapping],
    interfaces: &[NetInterface],
    evidence: NetworkEvidence,
) -> NetworkTopology {
    let mut topology = NetworkTopology::default();
    let interface_by_name: HashMap<&str, &NetInterface> = interfaces
        .iter()
        .map(|interface| (interface.name.as_str(), interface))
        .collect();

    let primary = vifs
        .iter()
        .filter(|vif| !matches!(parse_vif_name(&vif.vif), Some((_, _, VifRole::Auxiliary))))
        .collect::<Vec<_>>();
    topology.primary_vif_count = primary.len();
    topology.chains = primary
        .iter()
        .map(|vif| {
            build_chain(
                vif,
                interfaces,
                &interface_by_name,
                &mut topology.diagnostics,
            )
        })
        .collect();

    associate_auxiliary_interfaces(interfaces, &mut topology);
    validate_interface_inventory(interfaces, &interface_by_name, &mut topology.diagnostics);

    let guest_count = domains.iter().filter(|domain| domain.domid != 0).count();
    topology.status = if evidence.vifs == EvidenceAvailability::Error
        || evidence.interfaces == EvidenceAvailability::Error
    {
        NetworkTopologyStatus::Error
    } else if (guest_count > 0 && evidence.vifs == EvidenceAvailability::Unavailable)
        || evidence.interfaces == EvidenceAvailability::Unavailable
    {
        NetworkTopologyStatus::Unavailable
    } else if primary.is_empty() && guest_count == 0 {
        NetworkTopologyStatus::Empty
    } else if evidence.vifs == EvidenceAvailability::Partial
        || evidence.interfaces == EvidenceAvailability::Partial
        || !topology.diagnostics.is_empty()
    {
        NetworkTopologyStatus::Partial
    } else if primary.is_empty() {
        NetworkTopologyStatus::Empty
    } else {
        NetworkTopologyStatus::Complete
    };
    topology
}

fn build_chain(
    vif: &VifMapping,
    interfaces: &[NetInterface],
    interface_by_name: &HashMap<&str, &NetInterface>,
    diagnostics: &mut Vec<NetworkDiagnostic>,
) -> NetworkChain {
    let mut chain = NetworkChain {
        domid: vif.domid,
        vif_index: parse_vif_name(&vif.vif).map(|(_, index, _)| index),
        vif: vif.vif.clone(),
        ..Default::default()
    };
    let Some(bridge_name) = vif.bridge.as_deref() else {
        add_diagnostic(
            diagnostics,
            &mut chain,
            NetworkDiagnostic {
                anomaly: NetworkAnomaly::VifWithoutBridge,
                domid: Some(vif.domid),
                interface: Some(vif.vif.clone()),
                reason: format!("{} has no host-observed bridge", vif.vif),
            },
        );
        return chain;
    };
    let Some(bridge) = interface_by_name
        .get(bridge_name)
        .filter(|interface| interface.kind == "bridge")
    else {
        add_diagnostic(
            diagnostics,
            &mut chain,
            NetworkDiagnostic {
                anomaly: NetworkAnomaly::UnknownParent,
                domid: Some(vif.domid),
                interface: Some(vif.vif.clone()),
                reason: format!("bridge {bridge_name} is absent from host interface inventory"),
            },
        );
        return chain;
    };
    chain.bridge = Some(bridge_name.to_string());

    let bridge_children = interfaces
        .iter()
        .filter(|interface| {
            interface.master.as_deref() == Some(bridge_name) && interface.kind != "vif"
        })
        .collect::<Vec<_>>();
    let uplinks = bridge_children
        .iter()
        .copied()
        .filter(|interface| matches!(interface.kind.as_str(), "bond" | "physical"))
        .collect::<Vec<_>>();
    if uplinks.is_empty() {
        if bridge_children.is_empty() {
            add_diagnostic(
                diagnostics,
                &mut chain,
                NetworkDiagnostic {
                    anomaly: NetworkAnomaly::BridgeWithoutUplink,
                    domid: Some(vif.domid),
                    interface: Some(bridge_name.to_string()),
                    reason: format!("bridge {bridge_name} has no host-observed non-VIF uplink"),
                },
            );
        } else {
            add_diagnostic(
                diagnostics,
                &mut chain,
                NetworkDiagnostic {
                    anomaly: NetworkAnomaly::UnknownParent,
                    domid: Some(vif.domid),
                    interface: Some(bridge_name.to_string()),
                    reason: format!(
                        "bridge {bridge_name} has only unsupported uplink type(s): {}",
                        bridge_children
                            .iter()
                            .map(|interface| interface.kind.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                },
            );
        }
        return chain;
    }

    for uplink in uplinks {
        chain.uplinks.push(uplink.name.clone());
        compare_link_attributes(bridge, uplink, vif.domid, &mut chain, diagnostics);
        if uplink.kind == "physical" {
            chain.physical_nics.push(uplink.name.clone());
            validate_physical_state(uplink, vif.domid, &mut chain, diagnostics);
            continue;
        }

        let mut members = uplink.members.clone();
        members.extend(
            interfaces
                .iter()
                .filter(|interface| interface.master.as_deref() == Some(uplink.name.as_str()))
                .map(|interface| interface.name.clone()),
        );
        members.sort();
        members.dedup();
        if uplink.members_available && members.is_empty() {
            add_diagnostic(
                diagnostics,
                &mut chain,
                NetworkDiagnostic {
                    anomaly: NetworkAnomaly::BondWithoutSlave,
                    domid: Some(vif.domid),
                    interface: Some(uplink.name.clone()),
                    reason: format!("bond {} has an observed empty slave set", uplink.name),
                },
            );
        }
        for member_name in members {
            let Some(member) = interface_by_name.get(member_name.as_str()) else {
                add_diagnostic(
                    diagnostics,
                    &mut chain,
                    NetworkDiagnostic {
                        anomaly: NetworkAnomaly::UnknownParent,
                        domid: Some(vif.domid),
                        interface: Some(member_name.clone()),
                        reason: format!(
                            "bond {} references missing member {member_name}",
                            uplink.name
                        ),
                    },
                );
                continue;
            };
            if member.kind == "physical" {
                chain.physical_nics.push(member.name.clone());
                chain
                    .uplink_members
                    .push((uplink.name.clone(), member.name.clone()));
                validate_physical_state(member, vif.domid, &mut chain, diagnostics);
                compare_link_attributes(uplink, member, vif.domid, &mut chain, diagnostics);
            }
        }
    }
    chain.uplinks.sort();
    chain.uplinks.dedup();
    chain.physical_nics.sort();
    chain.physical_nics.dedup();
    chain.uplink_members.sort();
    chain.uplink_members.dedup();
    chain
}

fn associate_auxiliary_interfaces(interfaces: &[NetInterface], topology: &mut NetworkTopology) {
    let mut auxiliary: HashMap<(u32, u32), Vec<String>> = HashMap::new();
    for interface in interfaces {
        if let Some((domid, index, VifRole::Auxiliary)) = parse_vif_name(&interface.name) {
            topology.auxiliary_vif_count += 1;
            auxiliary
                .entry((domid, index))
                .or_default()
                .push(interface.name.clone());
        }
    }
    for ((domid, index), mut names) in auxiliary {
        names.sort();
        if let Some(chain) = topology
            .chains
            .iter_mut()
            .find(|chain| chain.domid == domid && chain.vif_index == Some(index))
        {
            chain.auxiliary_interfaces = names.clone();
            if names.len() > 1 {
                add_diagnostic(
                    &mut topology.diagnostics,
                    chain,
                    NetworkDiagnostic {
                        anomaly: NetworkAnomaly::DuplicateEmulatedVif,
                        domid: Some(domid),
                        interface: Some(names.join(",")),
                        reason: format!(
                            "DomID {domid} VIF {index} has {} emulated interfaces",
                            names.len()
                        ),
                    },
                );
            }
        } else {
            topology.diagnostics.push(NetworkDiagnostic {
                anomaly: NetworkAnomaly::OrphanEmulatedVif,
                domid: Some(domid),
                interface: Some(names.join(",")),
                reason: format!(
                    "emulated interface for DomID {domid} VIF {index} has no primary VIF evidence"
                ),
            });
        }
    }
}

fn validate_interface_inventory(
    interfaces: &[NetInterface],
    interface_by_name: &HashMap<&str, &NetInterface>,
    diagnostics: &mut Vec<NetworkDiagnostic>,
) {
    let mut seen = HashSet::new();
    for interface in interfaces {
        if !seen.insert(interface.name.as_str())
            && matches!(
                parse_vif_name(&interface.name),
                Some((_, _, VifRole::Auxiliary))
            )
        {
            diagnostics.push(NetworkDiagnostic {
                anomaly: NetworkAnomaly::DuplicateEmulatedVif,
                domid: parse_vif_name(&interface.name).map(|(domid, _, _)| domid),
                interface: Some(interface.name.clone()),
                reason: format!(
                    "auxiliary interface {} occurs more than once in inventory",
                    interface.name
                ),
            });
        }
        if let Some(master) = interface.master.as_deref()
            && !interface_by_name.contains_key(master)
        {
            diagnostics.push(NetworkDiagnostic {
                anomaly: NetworkAnomaly::UnknownParent,
                domid: parse_vif_name(&interface.name).map(|(domid, _, _)| domid),
                interface: Some(interface.name.clone()),
                reason: format!("{} references unknown parent {master}", interface.name),
            });
        }
    }
}

fn validate_physical_state(
    interface: &NetInterface,
    domid: u32,
    chain: &mut NetworkChain,
    diagnostics: &mut Vec<NetworkDiagnostic>,
) {
    if interface
        .operstate
        .as_deref()
        .is_some_and(|state| !matches!(state, "up" | "unknown"))
    {
        add_diagnostic(
            diagnostics,
            chain,
            NetworkDiagnostic {
                anomaly: NetworkAnomaly::PhysicalNicDown,
                domid: Some(domid),
                interface: Some(interface.name.clone()),
                reason: format!(
                    "physical NIC {} operstate is {}",
                    interface.name,
                    interface.operstate.as_deref().unwrap_or("unknown")
                ),
            },
        );
    }
}

fn compare_link_attributes(
    parent: &NetInterface,
    child: &NetInterface,
    domid: u32,
    chain: &mut NetworkChain,
    diagnostics: &mut Vec<NetworkDiagnostic>,
) {
    if let (Some(parent_mtu), Some(child_mtu)) = (parent.mtu, child.mtu)
        && parent_mtu != child_mtu
    {
        add_diagnostic(
            diagnostics,
            chain,
            NetworkDiagnostic {
                anomaly: NetworkAnomaly::MtuMismatch,
                domid: Some(domid),
                interface: Some(child.name.clone()),
                reason: format!(
                    "MTU mismatch: {}={parent_mtu}, {}={child_mtu}",
                    parent.name, child.name
                ),
            },
        );
    }
    if let (Some(parent_state), Some(child_state)) =
        (known_operstate(parent), known_operstate(child))
        && parent_state != child_state
    {
        add_diagnostic(
            diagnostics,
            chain,
            NetworkDiagnostic {
                anomaly: NetworkAnomaly::OperstateMismatch,
                domid: Some(domid),
                interface: Some(child.name.clone()),
                reason: format!(
                    "operstate mismatch: {}={parent_state}, {}={child_state}",
                    parent.name, child.name
                ),
            },
        );
    }
}

fn known_operstate(interface: &NetInterface) -> Option<&str> {
    interface
        .operstate
        .as_deref()
        .filter(|state| *state != "unknown")
}

fn add_diagnostic(
    diagnostics: &mut Vec<NetworkDiagnostic>,
    chain: &mut NetworkChain,
    diagnostic: NetworkDiagnostic,
) {
    chain.diagnostics.push(diagnostic.reason.clone());
    diagnostics.push(diagnostic);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface(name: &str, kind: &str, master: Option<&str>, mtu: u32) -> NetInterface {
        NetInterface {
            name: name.into(),
            kind: kind.into(),
            master: master.map(str::to_string),
            mtu: Some(mtu),
            operstate: Some("up".into()),
            members_available: kind == "bond",
            ..Default::default()
        }
    }

    fn available() -> NetworkEvidence {
        NetworkEvidence {
            vifs: EvidenceAvailability::Available,
            interfaces: EvidenceAvailability::Available,
        }
    }

    #[test]
    fn ap7_domain_zero_only_is_empty_not_failure() {
        let topology = validate_network_topology(
            &[DomainIdentity {
                domid: 0,
                name: "Domain-0".into(),
            }],
            &[],
            &[],
            available(),
        );

        assert_eq!(topology.status, NetworkTopologyStatus::Empty);
        assert!(topology.chains.is_empty());
        assert!(topology.diagnostics.is_empty());
    }

    #[test]
    fn ap8_fixture_builds_eight_primary_chains_without_counting_emu() {
        let guest_ids = [1858, 1859, 1860, 1862];
        let domains = std::iter::once(DomainIdentity {
            domid: 0,
            name: "Domain-0".into(),
        })
        .chain(guest_ids.map(|domid| DomainIdentity {
            domid,
            name: format!("guest-{domid}"),
        }))
        .collect::<Vec<_>>();
        let vifs = guest_ids
            .into_iter()
            .flat_map(|domid| {
                [
                    VifMapping {
                        domid,
                        vif: format!("vif{domid}.0"),
                        bridge: Some("ac121100".into()),
                        ..Default::default()
                    },
                    VifMapping {
                        domid,
                        vif: format!("vif{domid}.1"),
                        bridge: Some("10c61b749d".into()),
                        ..Default::default()
                    },
                ]
            })
            .collect::<Vec<_>>();
        let mut interfaces = vec![
            interface("ac121100", "bridge", None, 1500),
            interface("bond0", "bond", Some("ac121100"), 1500),
            interface("em1", "physical", Some("bond0"), 1500),
            interface("10c61b749d", "bridge", None, 1600),
            interface("em2", "physical", Some("10c61b749d"), 1600),
            interface("vif1862.0-emu", "vif", Some("ac121100"), 1500),
            interface("vif1862.1-emu", "vif", Some("10c61b749d"), 1600),
        ];
        interfaces[1].members = vec!["em1".into()];

        let topology = validate_network_topology(&domains, &vifs, &interfaces, available());

        assert_eq!(domains.iter().filter(|domain| domain.domid != 0).count(), 4);
        assert_eq!(topology.status, NetworkTopologyStatus::Complete);
        assert_eq!(topology.primary_vif_count, 8);
        assert_eq!(topology.auxiliary_vif_count, 2);
        assert_eq!(topology.chains.len(), 8);
        assert!(topology.chains.iter().all(|chain| {
            if chain.vif_index == Some(0) {
                chain.uplinks == ["bond0"]
                    && chain.physical_nics == ["em1"]
                    && chain.uplink_members == [("bond0".into(), "em1".into())]
            } else {
                chain.uplinks == ["em2"] && chain.physical_nics == ["em2"]
            }
        }));
        assert!(topology.diagnostics.is_empty());
    }

    #[test]
    fn reports_only_anomalies_supported_by_fixture_evidence() {
        let domains = vec![DomainIdentity {
            domid: 7,
            name: "guest".into(),
        }];
        let vifs = vec![VifMapping {
            domid: 7,
            vif: "vif7.0".into(),
            bridge: Some("br0".into()),
            ..Default::default()
        }];
        let mut nic = interface("eth0", "physical", Some("bond0"), 1400);
        nic.operstate = Some("down".into());
        let mut bond = interface("bond0", "bond", Some("br0"), 1500);
        bond.members = vec!["eth0".into()];
        let interfaces = vec![
            interface("br0", "bridge", None, 1500),
            bond,
            nic,
            interface("vif9.0-emu", "vif", Some("missing-br"), 1500),
        ];

        let topology = validate_network_topology(&domains, &vifs, &interfaces, available());
        let anomalies = topology
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.anomaly)
            .collect::<HashSet<_>>();

        assert_eq!(topology.status, NetworkTopologyStatus::Partial);
        assert!(anomalies.contains(&NetworkAnomaly::PhysicalNicDown));
        assert!(anomalies.contains(&NetworkAnomaly::MtuMismatch));
        assert!(anomalies.contains(&NetworkAnomaly::OperstateMismatch));
        assert!(anomalies.contains(&NetworkAnomaly::OrphanEmulatedVif));
        assert!(anomalies.contains(&NetworkAnomaly::UnknownParent));
    }

    #[test]
    fn unavailable_evidence_is_not_reported_as_empty() {
        let topology = validate_network_topology(
            &[DomainIdentity {
                domid: 7,
                name: "guest".into(),
            }],
            &[],
            &[],
            NetworkEvidence::default(),
        );

        assert_eq!(topology.status, NetworkTopologyStatus::Unavailable);
    }

    #[test]
    fn collector_error_is_distinct_from_unavailable_evidence() {
        let topology = validate_network_topology(
            &[DomainIdentity {
                domid: 7,
                name: "guest".into(),
            }],
            &[],
            &[],
            NetworkEvidence {
                vifs: EvidenceAvailability::Available,
                interfaces: EvidenceAvailability::Error,
            },
        );

        assert_eq!(topology.status, NetworkTopologyStatus::Error);
    }

    #[test]
    fn reports_missing_bridge_uplink_empty_bond_and_duplicate_auxiliary() {
        let domains = vec![DomainIdentity {
            domid: 7,
            name: "synthetic-guest".into(),
        }];
        let vifs = vec![
            VifMapping {
                domid: 7,
                vif: "vif7.0".into(),
                bridge: None,
                ..Default::default()
            },
            VifMapping {
                domid: 7,
                vif: "vif7.1".into(),
                bridge: Some("br-empty".into()),
                ..Default::default()
            },
            VifMapping {
                domid: 7,
                vif: "vif7.2".into(),
                bridge: Some("br-bond".into()),
                ..Default::default()
            },
        ];
        let interfaces = vec![
            interface("br-empty", "bridge", None, 1500),
            interface("br-bond", "bridge", None, 1500),
            interface("bond0", "bond", Some("br-bond"), 1500),
            interface("vif7.2-emu", "vif", Some("br-bond"), 1500),
            interface("vif7.2-emu", "vif", Some("br-bond"), 1500),
        ];

        let topology = validate_network_topology(&domains, &vifs, &interfaces, available());
        let anomalies = topology
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.anomaly)
            .collect::<HashSet<_>>();

        assert!(anomalies.contains(&NetworkAnomaly::VifWithoutBridge));
        assert!(anomalies.contains(&NetworkAnomaly::BridgeWithoutUplink));
        assert!(anomalies.contains(&NetworkAnomaly::BondWithoutSlave));
        assert!(anomalies.contains(&NetworkAnomaly::DuplicateEmulatedVif));
    }
}
