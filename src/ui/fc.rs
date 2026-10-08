use super::{
    layout::{compact, master_detail, range, viewport},
    widgets::{block, bytes, compact_storage_status, layer_status},
};
use crate::{
    app::{App, DetailTarget, FcPanelMode, FcView, ScrollState},
    collector::fc::{FcHost, FcTarget, MultipathMap, MultipathPath},
    topology::snapshot::StorageHealth,
};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Style},
    widgets::{Paragraph, Row, Table},
};

pub(super) fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let snapshot = app.fc.lock().map(|item| item.clone()).unwrap_or_default();
    if app.fc_config.view == FcView::Overview {
        let online = snapshot
            .hosts
            .iter()
            .filter(|item| item.port_state.as_deref() == Some("Online"))
            .count();
        let healthy = snapshot
            .maps
            .iter()
            .filter(|item| item.health == StorageHealth::Healthy)
            .count();
        let degraded = snapshot
            .maps
            .iter()
            .filter(|item| item.health == StorageHealth::Degraded)
            .count();
        let failed = snapshot
            .maps
            .iter()
            .filter(|item| item.health == StorageHealth::Failed)
            .count();
        let single = snapshot
            .maps
            .iter()
            .filter(|item| item.redundancy == crate::topology::snapshot::StorageRedundancy::Single)
            .count();
        let redundant = snapshot
            .maps
            .iter()
            .filter(|item| {
                item.redundancy == crate::topology::snapshot::StorageRedundancy::Redundant
            })
            .count();
        let reduced = snapshot
            .maps
            .iter()
            .filter(|item| item.redundancy == crate::topology::snapshot::StorageRedundancy::Reduced)
            .count();
        frame.render_widget(Paragraph::new(format!("HBA ports        {} ({})\nOnline ports     {online}\nRemote ports     {}\nTargets          {} ({})\nMultipath maps   {} ({})\nHealthy          {healthy}\nDegraded         {degraded}\nFailed           {failed}\nSingle           {single}\nRedundant        {redundant}\nReduced          {reduced}\nDiagnostics      {}\n\nScope: host-side FC and multipath only", snapshot.hosts.len(), layer_status(&snapshot.layers.fc_hba), snapshot.rports.len(), snapshot.targets.len(), layer_status(&snapshot.layers.fc_transport), snapshot.maps.len(), layer_status(&snapshot.layers.multipath), snapshot.diagnostics.len())).block(block("FC/SAN Overview")), area);
        return;
    }
    if app.fc_config.view == FcView::Ports {
        let visible = viewport(area);
        app.fc_scroll.total = snapshot.hosts.len();
        app.fc_scroll.viewport = visible;
        let rows = snapshot
            .hosts
            .iter()
            .skip(app.fc_scroll.offset)
            .map(|item| {
                Row::new(vec![
                    item.name.clone(),
                    item.port_state.clone().unwrap_or_else(|| "Unknown".into()),
                    item.speed.clone().unwrap_or_else(|| "Unknown".into()),
                    item.port_wwn.clone().unwrap_or_else(|| "Unknown".into()),
                    item.fabric_name.clone().unwrap_or_else(|| "Unknown".into()),
                ])
            });
        frame.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Length(10),
                    Constraint::Length(12),
                    Constraint::Length(12),
                    Constraint::Min(20),
                    Constraint::Min(20),
                ],
            )
            .header(Row::new(["HBA", "STATE", "SPEED", "PORT WWPN", "FABRIC"]))
            .block(block(format!(
                "FC Ports [{}]",
                range(app.fc_scroll.offset, snapshot.hosts.len(), visible)
            ))),
            area,
        );
        return;
    }
    if app.fc_config.view == FcView::Targets {
        let visible = viewport(area);
        app.fc_scroll.total = snapshot.targets.len();
        app.fc_scroll.viewport = visible;
        let rows = snapshot
            .targets
            .iter()
            .skip(app.fc_scroll.offset)
            .map(|item| {
                Row::new(vec![
                    item.name.clone(),
                    item.port_wwn.clone().unwrap_or_else(|| "Unknown".into()),
                    item.node_wwn.clone().unwrap_or_else(|| "Unknown".into()),
                    item.port_id.clone().unwrap_or_else(|| "Unknown".into()),
                ])
            });
        frame.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Length(18),
                    Constraint::Min(22),
                    Constraint::Min(22),
                    Constraint::Length(12),
                ],
            )
            .header(Row::new(["TARGET", "PORT WWPN", "NODE WWPN", "PORT ID"]))
            .block(block(format!(
                "FC Targets [{}]",
                range(app.fc_scroll.offset, snapshot.targets.len(), visible)
            ))),
            area,
        );
        return;
    }
    let detail = app.fc_panel_mode == FcPanelMode::Detail;
    let layout = master_detail(area, detail);
    if let Some(summary) = layout.master {
        let visible = viewport(summary);
        let narrow = compact(summary);
        app.fc_scroll.total = snapshot.maps.len();
        app.fc_scroll.viewport = visible;
        let rows = snapshot
            .maps
            .iter()
            .enumerate()
            .skip(app.fc_scroll.offset)
            .map(|(index, item)| {
                let compact = compact_storage_status(item.health, item.redundancy);
                let row = if narrow {
                    Row::new(vec![
                        item.wwid.clone(),
                        item.mapper.clone(),
                        format!("{}/{}", item.active_paths, item.total_paths),
                        compact.text().to_string(),
                    ])
                } else {
                    Row::new(vec![
                        item.wwid.clone(),
                        item.mapper.clone(),
                        item.size.clone(),
                        format!("{}/{}", item.active_paths, item.total_paths),
                        compact.text().to_string(),
                        item.io
                            .as_ref()
                            .map(|io| bytes(io.read_bytes_per_sec))
                            .unwrap_or_else(|| "NO DATA".into()),
                        item.io
                            .as_ref()
                            .map(|io| bytes(io.write_bytes_per_sec))
                            .unwrap_or_else(|| "NO DATA".into()),
                    ])
                }
                .style(compact.style());
                if app.fc_selected == Some(index) {
                    row.style(Style::default().bg(Color::DarkGray).fg(Color::Yellow))
                } else {
                    row
                }
            });
        frame.render_widget(
            if narrow {
                Table::new(
                    rows,
                    [
                        Constraint::Min(18),
                        Constraint::Length(10),
                        Constraint::Length(8),
                        Constraint::Length(10),
                    ],
                )
                .header(Row::new(["WWID", "MAP", "PATHS", "STATE"]))
            } else {
                Table::new(
                    rows,
                    [
                        Constraint::Min(24),
                        Constraint::Length(10),
                        Constraint::Length(8),
                        Constraint::Length(8),
                        Constraint::Length(10),
                        Constraint::Length(13),
                        Constraint::Length(13),
                    ],
                )
                .header(Row::new([
                    "WWID", "MAP", "SIZE", "PATHS", "STATE", "READ", "WRITE",
                ]))
            }
            .block(block(format!(
                "Multipath [{}]",
                range(app.fc_scroll.offset, snapshot.maps.len(), visible)
            ))),
            summary,
        );
    }
    if detail {
        let selected = match &app.detail_target {
            DetailTarget::Multipath(wwid) => snapshot
                .maps
                .iter()
                .find(|map| map.wwid == *wwid)
                .map(|map| map.wwid.as_str()),
            _ => None,
        };
        let Some(wwid) = selected else {
            return;
        };
        let Some(map) = snapshot.maps.iter().find(|map| map.wwid == wwid) else {
            return;
        };
        render_path_detail(
            frame,
            layout.detail,
            map,
            &snapshot.hosts,
            &snapshot.targets,
            &mut app.fc_detail_scroll,
        );
    }
}

#[derive(Debug, PartialEq, Eq)]
struct PathDetail {
    hctl: String,
    device: String,
    lun: String,
    hba: String,
    hba_wwpn: String,
    target: String,
    port_id: String,
    state: String,
}

fn known(value: &str) -> String {
    if value.is_empty() {
        "Unknown".into()
    } else {
        value.into()
    }
}

fn optional(value: Option<&str>) -> String {
    value.map(known).unwrap_or_else(|| "Unknown".into())
}

fn path_detail(path: &MultipathPath, hosts: &[FcHost], targets: &[FcTarget]) -> PathDetail {
    let host = hosts
        .iter()
        .find(|host| host.name == format!("host{}", path.host));
    let target = targets.iter().find(|target| {
        target.host == path.host && target.channel == path.channel && target.target == path.target
    });
    PathDetail {
        hctl: known(&path.hctl),
        device: known(&path.device),
        lun: path.lun.to_string(),
        hba: host
            .map(|host| known(&host.name))
            .unwrap_or_else(|| "Unavailable".into()),
        hba_wwpn: host
            .and_then(|host| host.port_wwn.as_deref())
            .map(known)
            .unwrap_or_else(|| "Unknown".into()),
        target: target
            .map(|target| {
                format!(
                    "{} / {}",
                    known(&target.name),
                    optional(target.port_wwn.as_deref())
                )
            })
            .unwrap_or_else(|| "Unavailable".into()),
        port_id: target
            .and_then(|target| target.port_id.as_deref())
            .map(known)
            .unwrap_or_else(|| "Unknown".into()),
        state: known(&path.state),
    }
}

fn render_path_detail(
    frame: &mut Frame,
    area: Rect,
    map: &MultipathMap,
    hosts: &[FcHost],
    targets: &[FcTarget],
    scroll: &mut ScrollState,
) {
    let state = compact_storage_status(map.health, map.redundancy);
    let title = format!(
        "FC/SAN Paths — WWID {} MAP {} PATHS {}/{} STATE {}",
        map.wwid,
        map.mapper,
        map.active_paths,
        map.total_paths,
        state.text()
    );
    let narrow = compact(area);
    let visible = viewport(area);
    scroll.total = map.paths.len();
    scroll.viewport = visible;
    scroll.offset = scroll
        .offset
        .min(scroll.total.saturating_sub(scroll.viewport));
    let rows = map.paths.iter().skip(scroll.offset).map(|path| {
        let detail = path_detail(path, hosts, targets);
        if narrow {
            Row::new(vec![detail.hctl, detail.device, detail.lun, detail.state])
        } else {
            Row::new(vec![
                detail.hctl,
                detail.device,
                detail.lun,
                detail.hba,
                detail.hba_wwpn,
                detail.target,
                detail.port_id,
                detail.state,
            ])
        }
    });
    let table = if narrow {
        Table::new(
            rows,
            [
                Constraint::Length(14),
                Constraint::Min(12),
                Constraint::Length(6),
                Constraint::Length(12),
            ],
        )
        .header(Row::new(["HCTL", "DEVICE", "LUN", "STATE"]))
    } else {
        Table::new(
            rows,
            [
                Constraint::Length(14),
                Constraint::Length(14),
                Constraint::Length(6),
                Constraint::Length(10),
                Constraint::Min(18),
                Constraint::Min(28),
                Constraint::Length(14),
                Constraint::Length(14),
            ],
        )
        .header(Row::new([
            "HCTL",
            "DEVICE",
            "LUN",
            "HBA",
            "HBA WWPN",
            "TARGET/WWPN",
            "PORT ID",
            "PATH STATE",
        ]))
    };
    frame.render_widget(table.block(block(title)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_detail_joins_hba_and_target_metadata() {
        let path = MultipathPath {
            hctl: "host7:0:3:9".into(),
            device: "sda".into(),
            host: 7,
            channel: 0,
            target: 3,
            lun: 9,
            state: "active".into(),
            ..Default::default()
        };
        let detail = path_detail(
            &path,
            &[FcHost {
                name: "host7".into(),
                port_wwn: Some("0xabc".into()),
                ..Default::default()
            }],
            &[FcTarget {
                name: "target7:0:3".into(),
                host: 7,
                channel: 0,
                target: 3,
                port_wwn: Some("0xdef".into()),
                port_id: Some("0x12".into()),
                ..Default::default()
            }],
        );
        assert_eq!(detail.hba, "host7");
        assert_eq!(detail.hba_wwpn, "0xabc");
        assert_eq!(detail.target, "target7:0:3 / 0xdef");
        assert_eq!(detail.port_id, "0x12");
    }

    #[test]
    fn path_detail_does_not_guess_missing_metadata() {
        let detail = path_detail(&MultipathPath::default(), &[], &[]);
        assert_eq!(detail.hba, "Unavailable");
        assert_eq!(detail.hba_wwpn, "Unknown");
        assert_eq!(detail.target, "Unavailable");
        assert_eq!(detail.port_id, "Unknown");
        assert_eq!(detail.state, "Unknown");
    }
}
