use super::{
    layout::{compact, range, viewport},
    widgets::{availability, block, compact_status, empty, error, freshness, health, redundancy},
};
use crate::{
    app::ScrollState,
    topology::{
        status::Availability,
        view::{Evidence, RelationKind, TopologyNode, TopologyUiState},
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    widgets::{Row, Table},
};
use std::collections::{HashMap, HashSet};

pub(super) fn render(
    frame: &mut Frame,
    state: &TopologyUiState,
    area: Rect,
    scroll: &mut ScrollState,
    title: &str,
    selected: Option<&str>,
) {
    if state.status.availability == Availability::Empty {
        frame.render_widget(
            empty(
                format!("{title} [EMPTY]"),
                state.status.reason.as_deref().unwrap_or("No topology data"),
            ),
            area,
        );
        return;
    }
    if matches!(
        state.status.availability,
        Availability::Unavailable | Availability::Error
    ) {
        frame.render_widget(
            error(
                format!("{title} [{}]", availability(state.status.availability)),
                state
                    .status
                    .reason
                    .as_deref()
                    .unwrap_or("Topology evidence unavailable"),
            ),
            area,
        );
        return;
    }

    let selected_ids = selected.map(|selected| related_ids(state, selected));
    let included = |id: &str| selected_ids.as_ref().is_none_or(|ids| ids.contains(id));
    let mut rows = hierarchy_rows(state, &included, selected.is_some());
    let compact_view = compact(area);
    for reason in state
        .reasons
        .iter()
        .filter(|reason| selected.is_none_or(|selected| reason.contains(selected)))
    {
        rows.push(Row::new(vec![
            "DIAGNOSTIC".to_string(),
            "REASON".to_string(),
            diagnostic_reason(reason, compact_view),
        ]));
    }
    if rows.is_empty() {
        rows.push(Row::new(vec![
            "NO MATCHING TOPOLOGY".to_string(),
            "-".to_string(),
            compact_status(
                crate::topology::status::Freshness::NoData,
                Availability::Unavailable,
                None,
                None,
                None,
                Some(Evidence::Unavailable),
            )
            .text()
            .to_string(),
        ]));
    }
    let visible = viewport(area);
    scroll.total = rows.len();
    scroll.viewport = visible;
    scroll.offset = scroll.offset.min(rows.len().saturating_sub(visible));
    let table = if compact(area) {
        Table::new(
            rows.into_iter().skip(scroll.offset),
            [
                Constraint::Percentage(48),
                Constraint::Percentage(22),
                Constraint::Percentage(30),
            ],
        )
        .header(Row::new(["TOPOLOGY", "RELATION/EVIDENCE", "STATUS"]))
    } else {
        Table::new(
            rows.into_iter().skip(scroll.offset),
            [
                Constraint::Min(35),
                Constraint::Length(22),
                Constraint::Length(30),
            ],
        )
        .header(Row::new(["TOPOLOGY", "RELATION/EVIDENCE", "STATUS"]))
    };
    frame.render_widget(
        table.block(block(format!(
            "{title} [{}/{}] [{}]",
            freshness(state.status.freshness),
            availability(state.status.availability),
            range(scroll.offset, scroll.total, visible)
        ))),
        area,
    );
}

fn related_ids(state: &TopologyUiState, selected: &str) -> HashSet<String> {
    let mut ids = state
        .nodes
        .iter()
        .filter(|node| node.id.contains(selected) || node.label.contains(selected))
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();
    loop {
        let previous = ids.len();
        for edge in &state.edges {
            if ids.contains(&edge.from) || ids.contains(&edge.to) {
                ids.insert(edge.from.clone());
                ids.insert(edge.to.clone());
            }
        }
        if ids.len() == previous {
            break;
        }
    }
    ids
}

fn hierarchy_rows(
    state: &TopologyUiState,
    included: &impl Fn(&str) -> bool,
    detailed: bool,
) -> Vec<Row<'static>> {
    let ids = state
        .nodes
        .iter()
        .filter(|node| included(&node.id))
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let mut children: HashMap<&str, Vec<&crate::topology::view::TopologyEdge>> = HashMap::new();
    let mut has_parent = HashSet::new();
    for edge in &state.edges {
        if !ids.contains(edge.from.as_str()) || !ids.contains(edge.to.as_str()) {
            continue;
        }
        let (parent, child) = match edge.relation {
            RelationKind::MemberOf | RelationKind::AuxiliaryOf => (&edge.to, &edge.from),
            _ => (&edge.from, &edge.to),
        };
        children.entry(parent).or_default().push(edge);
        has_parent.insert(child.as_str());
    }
    for edges in children.values_mut() {
        edges.sort_by_key(|edge| edge_key(state, edge));
    }
    // Guest topology is the primary tree.  Standalone multipath inventory is
    // intentionally left to the FC/SAN workspace instead of being repeated
    // as unrelated roots beneath the guest chains.
    let has_domain_root = state.nodes.iter().any(|node| {
        ids.contains(node.id.as_str()) && node.kind == crate::topology::view::NodeKind::Domain
    });
    let mut roots = state
        .nodes
        .iter()
        .filter(|node| {
            ids.contains(node.id.as_str())
                && !has_parent.contains(node.id.as_str())
                && (!has_domain_root || node.kind == crate::topology::view::NodeKind::Domain)
        })
        .collect::<Vec<_>>();
    roots.sort_by_key(|node| node_key(node));

    let mut rows = Vec::new();
    let mut displayed = HashSet::new();
    let mut shared_references = HashSet::new();
    for node in roots {
        append_node_rows(
            state,
            node,
            "",
            None,
            &children,
            &mut displayed,
            &mut shared_references,
            detailed,
            &mut rows,
        );
    }
    rows
}

fn append_node_rows(
    state: &TopologyUiState,
    node: &TopologyNode,
    prefix: &str,
    incoming: Option<&crate::topology::view::TopologyEdge>,
    children: &HashMap<&str, Vec<&crate::topology::view::TopologyEdge>>,
    displayed: &mut HashSet<String>,
    shared_references: &mut HashSet<String>,
    detailed: bool,
    rows: &mut Vec<Row<'static>>,
) {
    let shared = !displayed.insert(node.id.clone());
    if shared && !shared_references.insert(node.id.clone()) {
        return;
    }
    let label = if shared {
        format!("{prefix}↳ shared {}", node_label(node))
    } else {
        format!("{prefix}{}", node_label(node))
    };
    let (relation, status) = incoming
        .map(|edge| {
            let mut status = if detailed {
                node_detail_status(node)
            } else {
                node_compact_status_with_evidence(node, Some(edge.evidence))
            };
            if detailed {
                if let Some(reason) = edge.reason.as_deref() {
                    status.push_str(" / ");
                    status.push_str(reason);
                }
            } else if let Some(reason) = edge.reason.as_deref().or(node.reason.as_deref()) {
                status.push_str(" / ");
                status.push_str(reason_indication(reason));
            }
            (
                format!(
                    "{} / {}",
                    relation_label(edge.relation),
                    evidence_label(edge.evidence)
                ),
                status,
            )
        })
        .unwrap_or_else(|| {
            (
                "ROOT".into(),
                if detailed {
                    node_detail_status(node)
                } else {
                    node_compact_status(node)
                },
            )
        });
    rows.push(Row::new(vec![label, relation, status]));
    if shared {
        return;
    }
    if let Some(edges) = children.get(node.id.as_str()) {
        let last = edges.len().saturating_sub(1);
        for (index, edge) in edges.iter().enumerate() {
            let Some(child) = node_by_id(state, child_id(edge)) else {
                continue;
            };
            let branch = if index == last { "└─ " } else { "├─ " };
            append_node_rows(
                state,
                child,
                &format!("{prefix}{branch}"),
                Some(edge),
                children,
                displayed,
                shared_references,
                detailed,
                rows,
            );
        }
    }
}

fn child_id(edge: &crate::topology::view::TopologyEdge) -> &str {
    match edge.relation {
        RelationKind::MemberOf | RelationKind::AuxiliaryOf => &edge.from,
        _ => &edge.to,
    }
}

fn node_key(node: &TopologyNode) -> (u8, String, String) {
    (
        kind_rank(node.kind),
        node.label.to_lowercase(),
        node.id.clone(),
    )
}

fn edge_key(
    state: &TopologyUiState,
    edge: &crate::topology::view::TopologyEdge,
) -> (u8, u8, String, String) {
    let child = node_by_id(state, child_id(edge));
    (
        relation_rank(edge.relation),
        child.map_or(u8::MAX, |node| kind_rank(node.kind)),
        child.map_or_else(
            || child_id(edge).to_string(),
            |node| node.label.to_lowercase(),
        ),
        child_id(edge).to_string(),
    )
}

fn kind_rank(kind: crate::topology::view::NodeKind) -> u8 {
    use crate::topology::view::NodeKind::*;
    match kind {
        Domain => 0,
        Vif | AuxiliaryVif => 1,
        BlockDevice => 2,
        Bridge | Bond | PhysicalNic => 3,
        DeviceMapper => 4,
        MultipathMap => 5,
        FcHost | FcTarget | FcPath => 6,
        Unknown => 7,
    }
}

fn relation_rank(relation: RelationKind) -> u8 {
    match relation {
        RelationKind::Owns => 0,
        RelationKind::AttachedTo => 1,
        RelationKind::Uplink => 2,
        RelationKind::MemberOf => 3,
        RelationKind::AuxiliaryOf => 4,
        RelationKind::MapsTo => 5,
        RelationKind::PathVia => 6,
    }
}

fn node_by_id<'a>(state: &'a TopologyUiState, id: &str) -> Option<&'a TopologyNode> {
    state.nodes.iter().find(|node| node.id == id)
}

fn node_label(node: &TopologyNode) -> String {
    format!("{:?}: {}", node.kind, node.label)
}

fn node_compact_status(node: &TopologyNode) -> String {
    let mut status = node_compact_status_with_evidence(node, None);
    if let Some(reason) = node.reason.as_deref() {
        status.push_str(" / ");
        status.push_str(reason_indication(reason));
    }
    status
}

fn node_detail_status(node: &TopologyNode) -> String {
    let mut values = vec![
        freshness(node.freshness).to_string(),
        availability(node.availability).to_string(),
        format!("{:?}", node.confidence),
    ];
    if let Some(value) = node.health {
        values.push(health(value).into());
    }
    if let Some(value) = node.redundancy {
        values.push(redundancy(value).into());
    }
    if let Some(reason) = node.reason.as_deref() {
        values.push(reason.into());
    }
    values.join(" / ")
}

fn node_compact_status_with_evidence(node: &TopologyNode, evidence: Option<Evidence>) -> String {
    compact_status(
        node.freshness,
        node.availability,
        node.health,
        node.redundancy,
        Some(node.confidence),
        evidence,
    )
    .text()
    .to_string()
}

fn reason_indication(reason: &str) -> &'static str {
    let reason = reason.to_ascii_lowercase();
    if reason.contains("unknown") || reason.contains("unavailable") || reason.contains("missing") {
        "?"
    } else if reason.contains("match")
        || reason.contains("mapping")
        || reason.contains("wwid")
        || reason.contains("parent")
    {
        "M"
    } else {
        "!"
    }
}

fn diagnostic_reason(reason: &str, compact_view: bool) -> String {
    if compact_view {
        reason_indication(reason).to_string()
    } else {
        reason.to_string()
    }
}

fn relation_label(relation: RelationKind) -> &'static str {
    match relation {
        RelationKind::Owns => "OWNS",
        RelationKind::AttachedTo => "ATTACHED TO",
        RelationKind::Uplink => "UPLINK",
        RelationKind::MemberOf => "MEMBER",
        RelationKind::AuxiliaryOf => "AUXILIARY",
        RelationKind::MapsTo => "MAPS TO",
        RelationKind::PathVia => "PATH VIA",
    }
}

fn evidence_label(evidence: Evidence) -> &'static str {
    match evidence {
        Evidence::HostObserved => "HOST",
        Evidence::Derived => "DERIVED",
        Evidence::Fallback => "FALLBACK",
        Evidence::Unavailable => "UNAVAILABLE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{snapshot::MappingConfidence, status::Freshness, view::NodeKind};

    fn test_node(id: &str, label: &str, kind: NodeKind) -> TopologyNode {
        TopologyNode {
            id: id.into(),
            label: label.into(),
            kind,
            domain_id: None,
            health: None,
            redundancy: None,
            freshness: Freshness::Live,
            availability: Availability::Available,
            confidence: MappingConfidence::Exact,
            reason: None,
        }
    }

    fn test_edge(
        from: &str,
        to: &str,
        relation: RelationKind,
    ) -> crate::topology::view::TopologyEdge {
        crate::topology::view::TopologyEdge {
            from: from.into(),
            to: to.into(),
            relation,
            evidence: Evidence::HostObserved,
            reason: None,
        }
    }

    #[test]
    fn hierarchy_is_domain_first_and_deterministically_ordered() {
        let state = TopologyUiState {
            nodes: vec![
                test_node("vif-b", "vif-b", NodeKind::Vif),
                test_node("domain-2", "DomID 2", NodeKind::Domain),
                test_node("domain-1", "DomID 1", NodeKind::Domain),
                test_node("vif-a", "vif-a", NodeKind::Vif),
            ],
            edges: vec![
                test_edge("domain-2", "vif-b", RelationKind::Owns),
                test_edge("domain-1", "vif-a", RelationKind::Owns),
            ],
            ..Default::default()
        };
        let rows = hierarchy_rows(&state, &|_| true, false);
        let rendered = rows
            .iter()
            .map(|row| format!("{row:?}"))
            .collect::<Vec<_>>();
        assert!(rendered[0].contains("DomID 1"));
        assert!(rendered[1].contains("vif-a"));
        assert!(rendered[2].contains("DomID 2"));
    }

    #[test]
    fn shared_node_is_a_reference_and_not_recursed() {
        let state = TopologyUiState {
            nodes: vec![
                test_node("domain-a", "DomID A", NodeKind::Domain),
                test_node("domain-b", "DomID B", NodeKind::Domain),
                test_node("bridge", "br0", NodeKind::Bridge),
                test_node("bond", "bond0", NodeKind::Bond),
            ],
            edges: vec![
                test_edge("domain-a", "bridge", RelationKind::Owns),
                test_edge("domain-b", "bridge", RelationKind::Owns),
                test_edge("bridge", "bond", RelationKind::Uplink),
            ],
            ..Default::default()
        };
        let rows = hierarchy_rows(&state, &|_| true, false);
        let rendered = rows
            .iter()
            .map(|row| format!("{row:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("↳ shared"));
        assert_eq!(rendered.matches("bond0").count(), 1);
    }

    #[test]
    fn shared_node_has_one_reference_even_with_multiple_parents() {
        let state = TopologyUiState {
            nodes: vec![
                test_node("domain-a", "DomID A", NodeKind::Domain),
                test_node("domain-b", "DomID B", NodeKind::Domain),
                test_node("domain-c", "DomID C", NodeKind::Domain),
                test_node("bridge", "br0", NodeKind::Bridge),
            ],
            edges: vec![
                test_edge("domain-a", "bridge", RelationKind::Owns),
                test_edge("domain-b", "bridge", RelationKind::Owns),
                test_edge("domain-c", "bridge", RelationKind::Owns),
            ],
            ..Default::default()
        };
        let rendered = hierarchy_rows(&state, &|_| true, false)
            .iter()
            .map(|row| format!("{row:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(rendered.matches("br0").count(), 2);
        assert_eq!(rendered.matches("↳ shared").count(), 1);
    }

    #[test]
    fn hierarchy_does_not_invent_unresolved_edges() {
        let state = TopologyUiState {
            nodes: vec![
                test_node("domain-1", "DomID 1", NodeKind::Domain),
                test_node("vbd", "VBD 51712", NodeKind::BlockDevice),
            ],
            edges: vec![test_edge("domain-1", "vbd", RelationKind::Owns)],
            ..Default::default()
        };
        let rows = hierarchy_rows(&state, &|_| true, false);
        let rendered = rows
            .iter()
            .map(|row| format!("{row:?}"))
            .collect::<String>();
        assert!(rendered.contains("VBD 51712"));
        assert!(!rendered.contains("MultipathMap"));
        assert_eq!(state.edges.len(), 1);
    }

    #[test]
    fn compact_reason_is_a_short_indication() {
        assert_eq!(reason_indication("no host-side WWID evidence"), "M");
        assert_eq!(reason_indication("bridge is unavailable"), "?");
        assert_eq!(reason_indication("unexpected topology issue"), "!");
    }

    #[test]
    fn compact_diagnostic_reason_does_not_leak_long_reason() {
        let reason = "backend path is unavailable: /very/long/xenstore/identity";
        assert_eq!(diagnostic_reason(reason, true), "?");
        assert!(!diagnostic_reason(reason, true).contains(reason));
        assert_eq!(diagnostic_reason(reason, false), reason);
    }
}
