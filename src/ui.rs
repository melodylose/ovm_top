use crate::{
    app::{
        App, DetailTarget, DiskView, FcPanelMode, FcView, InputMode, LogTimeRange, NetworkView,
        Workspace,
    },
    log,
    topology::snapshot::{MappingConfidence, SnapshotStatus, StorageHealth},
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};

fn bytes(value: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    match value as f64 {
        value if value >= GIB => format!("{:.1} GiB/s", value / GIB),
        value if value >= MIB => format!("{:.1} MiB/s", value / MIB),
        value if value >= KIB => format!("{:.1} KiB/s", value / KIB),
        _ => format!("{value} B/s"),
    }
}

fn viewport(area: Rect) -> usize {
    area.height.saturating_sub(3) as usize
}

fn range(offset: usize, total: usize, visible: usize) -> String {
    if total == 0 {
        return "0/0".into();
    }
    let start = offset.min(total - 1) + 1;
    let end = offset.saturating_add(visible).min(total);
    if start == end {
        format!("{start}/{total}")
    } else {
        format!("{start}-{end}/{total}")
    }
}

fn block(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .title(title.into())
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
}

fn state(value: &str) -> &'static str {
    if value.contains('r') {
        "Running"
    } else if value.contains('b') {
        "Blocked"
    } else if value.contains('p') {
        "Paused"
    } else if value.contains('c') {
        "Crashed"
    } else if value.contains('d') {
        "Dying"
    } else {
        "Unknown"
    }
}

fn health(value: StorageHealth) -> &'static str {
    match value {
        StorageHealth::Healthy => "HEALTHY",
        StorageHealth::Degraded => "DEGRADED",
        StorageHealth::Failed => "FAILED",
        StorageHealth::Unknown => "UNKNOWN",
    }
}

fn status(value: SnapshotStatus) -> &'static str {
    match value {
        SnapshotStatus::Live => "LIVE",
        SnapshotStatus::Stale => "STALE",
        SnapshotStatus::MergeFallback => "FALLBACK",
        SnapshotStatus::NoData => "NO DATA",
        SnapshotStatus::MergeError => "MERGE ERROR",
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(frame.area());
    draw_header(frame, app, outer[0]);
    match app.workspace {
        Workspace::Overview => draw_overview(frame, app, outer[1]),
        Workspace::Domains => draw_domains(frame, app, outer[1]),
        Workspace::Network => draw_network(frame, app, outer[1]),
        Workspace::Disk => draw_disk(frame, app, outer[1]),
        Workspace::Logs => draw_logs(frame, app, outer[1]),
        Workspace::FcSan => draw_fc(frame, app, outer[1]),
    }
    draw_footer(frame, app, outer[2]);
    if app.input_mode == InputMode::Help {
        draw_help(frame);
    }
    if app.fc_config_open {
        draw_fc_menu(frame, app);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let current = match app.workspace {
        Workspace::Overview => "Overview",
        Workspace::Domains => "Domains",
        Workspace::Network => "Network",
        Workspace::Disk => "Disk",
        Workspace::Logs => "Logs",
        Workspace::FcSan => "FC/SAN",
    };
    let size = if frame.area().width >= 100 && frame.area().height >= 30 {
        "LARGE"
    } else if frame.area().width >= 70 && frame.area().height >= 20 {
        "MEDIUM"
    } else {
        "SMALL"
    };
    frame.render_widget(
        Paragraph::new(format!(
            "ovm-top | {current} | {size}   0 Overview  1 Domains  2 Network  3 Disk  4 Logs  5 FC/SAN"
        ))
        .block(block("Workspace")),
        area,
    );
}

fn draw_overview(frame: &mut Frame, app: &App, area: Rect) {
    let domains = app
        .domains
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    let topology = app
        .topology
        .lock()
        .map(|item| item.clone())
        .unwrap_or_default();
    let fc = app.fc.lock().map(|item| item.clone()).unwrap_or_default();
    let running = domains
        .iter()
        .filter(|item| state(&item.state) == "Running")
        .count();
    let online = fc
        .hosts
        .iter()
        .filter(|host| host.port_state.as_deref() == Some("Online"))
        .count();
    let healthy = fc
        .maps
        .iter()
        .filter(|map| map.health == StorageHealth::Healthy)
        .count();
    let degraded = fc
        .maps
        .iter()
        .filter(|map| map.health == StorageHealth::Degraded)
        .count();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Min(8),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(format!(
            "Host: {}   Xen: {}   CPU: {}   RAM: {} / {} MiB free   NUMA: {}   Scheduler: {}",
            app.xm_info.host,
            app.xm_info.xen_version,
            app.xm_info.nr_cpus,
            app.xm_info.free_memory_mb,
            app.xm_info.total_memory_mb,
            app.xm_info.nr_nodes,
            app.xm_info.scheduler
        ))
        .wrap(Wrap { trim: true })
        .block(block("Xen Host")),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(format!(
            "VM: {}   Running: {running}   VBD: {}   VIF: {}   Snapshot: {}",
            domains.len(),
            topology.block_devices.len(),
            topology.vifs.len(),
            status(topology.meta.status)
        ))
        .block(block("Domains Summary")),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(format!(
            "Network interfaces: {}\nHost disks: {}\nFC ports: {online}/{} online\nTargets: {}\nMultipath: {} healthy, {degraded} degraded, {} total\nStorage metadata: host-side only; array-side unavailable",
            topology.interfaces.len(),
            app.disk.lock().map(|items| items.len()).unwrap_or_default(),
            fc.hosts.len(),
            fc.targets.len(),
            healthy,
            fc.maps.len()
        ))
        .block(block("Infrastructure")),
        chunks[2],
    );
}

fn detail_areas(area: Rect, detail: bool) -> (Option<Rect>, Rect) {
    if !detail {
        return (Some(area), area);
    }
    if area.width >= 100 && area.height >= 24 {
        let parts = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        (Some(parts[0]), parts[1])
    } else if area.width >= 70 && area.height >= 18 {
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        (Some(parts[0]), parts[1])
    } else {
        (None, area)
    }
}

fn draw_domains(frame: &mut Frame, app: &mut App, area: Rect) {
    let detail = matches!(app.detail_target, DetailTarget::Domain(_));
    let (summary, detail_area) = detail_areas(area, detail);
    if let Some(summary) = summary {
        let visible = viewport(summary);
        let domains = app
            .domains
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default();
        app.domain_scroll.total = domains.len();
        app.domain_scroll.viewport = visible;
        app.domain_scroll.offset = app
            .domain_scroll
            .offset
            .min(domains.len().saturating_sub(visible));
        let rows = domains
            .iter()
            .enumerate()
            .skip(app.domain_scroll.offset)
            .map(|(index, item)| {
                let row = Row::new(vec![
                    item.id.to_string(),
                    item.name.clone(),
                    state(&item.state).into(),
                    format!("{:.1}", item.cpu_percent),
                    format!("{:.1}", item.memory_percent),
                    item.vcpus.to_string(),
                    item.memory_mb.to_string(),
                ]);
                if app.domain_selected == Some(index) {
                    row.style(Style::default().bg(Color::DarkGray).fg(Color::Yellow))
                } else {
                    row
                }
            });
        let freshness = app
            .domain_meta
            .lock()
            .map(|meta| {
                if meta
                    .collected_at
                    .is_some_and(|time| time.elapsed().as_secs() > 4)
                {
                    "STALE"
                } else {
                    status(meta.status)
                }
            })
            .unwrap_or("NO DATA");
        frame.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Length(7),
                    Constraint::Min(18),
                    Constraint::Length(10),
                    Constraint::Length(8),
                    Constraint::Length(8),
                    Constraint::Length(6),
                    Constraint::Length(10),
                ],
            )
            .header(Row::new([
                "DomID", "NAME", "STATE", "CPU%", "MEM%", "VCPU", "MEM MiB",
            ]))
            .block(block(format!(
                "Domains [{}] [{freshness}]",
                range(app.domain_scroll.offset, domains.len(), visible),
            ))),
            summary,
        );
    }
    if detail {
        draw_domain_detail(frame, app, detail_area);
    }
}

fn draw_domain_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let domid = match app.detail_target {
        DetailTarget::Domain(id) => id,
        _ => return,
    };
    let domain = app
        .domains
        .lock()
        .ok()
        .and_then(|items| items.iter().find(|item| item.id == domid).cloned());
    let topology = app
        .topology
        .lock()
        .map(|item| item.clone())
        .unwrap_or_default();
    let mut rows = Vec::new();
    if let Some(domain) = domain {
        rows.push(Row::new(vec![
            Cell::from("Domain"),
            Cell::from(format!("DomID {domid}")),
        ]));
        rows.push(Row::new(vec![
            Cell::from("├─ Name"),
            Cell::from(format!("{} (display metadata)", domain.name)),
        ]));
        rows.push(Row::new(vec![
            Cell::from("├─ State"),
            Cell::from(state(&domain.state)),
        ]));
        let identity = match domain.identity_source {
            crate::topology::snapshot::IdentitySource::FullName => "FullName",
            crate::topology::snapshot::IdentitySource::StreamOrder => "StreamOrder fallback",
            crate::topology::snapshot::IdentitySource::Unknown => "Unknown",
        };
        rows.push(Row::new(vec![
            Cell::from("├─ Identity source"),
            Cell::from(identity),
        ]));
        rows.push(Row::new(vec![
            Cell::from("├─ Resources"),
            Cell::from(format!(
                "CPU {:.1}% MEM {:.1}% VCPU {}",
                domain.cpu_percent, domain.memory_percent, domain.vcpus
            )),
        ]));
        for vif in topology.vifs.iter().filter(|item| item.domid == domid) {
            rows.push(Row::new(vec![
                Cell::from(format!("├─ VIF {}", vif.vif)),
                Cell::from(format!(
                    "MAC={} bridge={}",
                    vif.mac.as_deref().unwrap_or("Unknown"),
                    vif.bridge.as_deref().unwrap_or("Unknown")
                )),
            ]));
        }
        for device in topology
            .block_devices
            .iter()
            .filter(|item| item.domid == domid)
        {
            let confidence = match device.confidence {
                MappingConfidence::Exact => "Exact",
                MappingConfidence::Derived => "Derived",
                MappingConfidence::Fallback => "Fallback",
                MappingConfidence::Unknown => "Unknown",
            };
            rows.push(Row::new(vec![
                Cell::from(format!("├─ VBD {}", device.frontend)),
                Cell::from(format!(
                    "device={} WWID={} confidence={confidence}",
                    device.device_name.as_deref().unwrap_or("Unknown"),
                    device.wwid.as_deref().unwrap_or("Unknown")
                )),
            ]));
            if let Some(storage) = device
                .wwid
                .as_ref()
                .and_then(|wwid| topology.storage.iter().find(|item| &item.wwid == wwid))
            {
                rows.push(Row::new(vec![
                    Cell::from("│  └─ Multipath"),
                    Cell::from(format!(
                        "{} {} host-side",
                        storage.mapper,
                        health(storage.health)
                    )),
                ]));
            }
        }
    } else {
        rows.push(Row::new(vec![
            Cell::from("NO DATA"),
            Cell::from("Domain no longer exists"),
        ]));
    }
    let visible = viewport(area);
    app.domain_detail_scroll.total = rows.len();
    app.domain_detail_scroll.viewport = visible;
    frame.render_widget(
        Table::new(
            rows.into_iter().skip(app.domain_detail_scroll.offset),
            [Constraint::Length(24), Constraint::Min(20)],
        )
        .header(Row::new(["VM-CENTRIC TOPOLOGY", "VALUE"]))
        .block(block(format!(
            "Domain Detail: DomID {domid} [{}]",
            range(
                app.domain_detail_scroll.offset,
                app.domain_detail_scroll.total,
                visible
            )
        ))),
        area,
    );
}

fn draw_network(frame: &mut Frame, app: &mut App, area: Rect) {
    let detail = matches!(app.detail_target, DetailTarget::Network(_));
    let (summary, detail_area) = detail_areas(area, detail);
    if let Some(summary) = summary {
        match app.network_view {
            NetworkView::Performance => {
                let items = app
                    .network
                    .lock()
                    .map(|items| items.clone())
                    .unwrap_or_default();
                let visible = viewport(summary);
                app.network_scroll.total = items.len();
                app.network_scroll.viewport = visible;
                let rows = items
                    .iter()
                    .enumerate()
                    .skip(app.network_scroll.offset)
                    .map(|(index, item)| {
                        let row = Row::new(vec![
                            item.name.clone(),
                            bytes(item.rx_bytes_per_sec),
                            bytes(item.tx_bytes_per_sec),
                            item.rx_packets_per_sec.to_string(),
                            item.tx_packets_per_sec.to_string(),
                            item.rx_drops_per_sec.to_string(),
                            item.tx_drops_per_sec.to_string(),
                        ]);
                        if app.network_selected == Some(index) {
                            row.style(Style::default().bg(Color::DarkGray).fg(Color::Yellow))
                        } else {
                            row
                        }
                    });
                frame.render_widget(
                    Table::new(
                        rows,
                        [
                            Constraint::Min(14),
                            Constraint::Length(14),
                            Constraint::Length(14),
                            Constraint::Length(10),
                            Constraint::Length(10),
                            Constraint::Length(8),
                            Constraint::Length(8),
                        ],
                    )
                    .header(Row::new([
                        "IFACE", "RX", "TX", "RX PPS", "TX PPS", "RX DROP", "TX DROP",
                    ]))
                    .block(block(format!(
                        "Network Performance [{}]",
                        range(app.network_scroll.offset, items.len(), visible)
                    ))),
                    summary,
                );
            }
            NetworkView::Topology => draw_network_topology(frame, app, summary, false),
        }
    }
    if detail {
        draw_network_topology(frame, app, detail_area, true);
    }
}

fn draw_network_topology(frame: &mut Frame, app: &mut App, area: Rect, selected_only: bool) {
    let snapshot = app
        .topology
        .lock()
        .map(|item| item.clone())
        .unwrap_or_default();
    let mut rows = Vec::new();
    let selected = match &app.detail_target {
        DetailTarget::Network(name) if selected_only => Some(name.as_str()),
        _ => None,
    };
    for item in snapshot
        .interfaces
        .iter()
        .filter(|item| selected.is_none_or(|name| item.name == name))
    {
        rows.push(Row::new(vec![
            item.name.clone(),
            format!(
                "type={} master={}",
                item.kind,
                item.master.as_deref().unwrap_or("Unknown")
            ),
        ]));
        for member in &item.members {
            rows.push(Row::new(vec![
                format!("  └─ {member}"),
                "bond member".into(),
            ]));
        }
        for vif in snapshot
            .vifs
            .iter()
            .filter(|vif| vif.bridge.as_deref() == Some(item.name.as_str()))
        {
            rows.push(Row::new(vec![
                format!("  └─ {} (DomID {})", vif.vif, vif.domid),
                format!("MAC={}", vif.mac.as_deref().unwrap_or("Unknown")),
            ]));
        }
    }
    let visible = viewport(area);
    app.network_detail_scroll.total = rows.len();
    app.network_detail_scroll.viewport = visible;
    frame.render_widget(
        Table::new(
            rows.into_iter().skip(app.network_detail_scroll.offset),
            [Constraint::Length(28), Constraint::Min(20)],
        )
        .header(Row::new(["HOST INTERFACE", "HOST-SIDE RELATIONSHIP"]))
        .block(block(format!(
            "Network Topology [{}] [{}]",
            status(snapshot.meta.status),
            range(
                app.network_detail_scroll.offset,
                app.network_detail_scroll.total,
                visible
            )
        ))),
        area,
    );
}

fn draw_disk(frame: &mut Frame, app: &mut App, area: Rect) {
    let detail = matches!(app.detail_target, DetailTarget::Disk(_));
    let (summary, detail_area) = detail_areas(area, detail);
    if let Some(summary) = summary {
        let disks = app
            .disk
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default();
        let visible = viewport(summary);
        app.disk_scroll.total = disks.len();
        app.disk_scroll.viewport = visible;
        match app.disk_view {
            DiskView::Performance => {
                let rows =
                    disks
                        .iter()
                        .enumerate()
                        .skip(app.disk_scroll.offset)
                        .map(|(index, item)| {
                            let row = Row::new(vec![
                                item.name.clone(),
                                bytes(item.read_bytes_per_sec),
                                bytes(item.write_bytes_per_sec),
                                item.read_iops.to_string(),
                                item.write_iops.to_string(),
                                format!("{:.1}%", item.utilization_percent),
                            ]);
                            if app.disk_selected == Some(index) {
                                row.style(Style::default().bg(Color::DarkGray).fg(Color::Yellow))
                            } else {
                                row
                            }
                        });
                frame.render_widget(
                    Table::new(
                        rows,
                        [
                            Constraint::Min(12),
                            Constraint::Length(14),
                            Constraint::Length(14),
                            Constraint::Length(9),
                            Constraint::Length(9),
                            Constraint::Length(8),
                        ],
                    )
                    .header(Row::new([
                        "DEVICE", "READ", "WRITE", "RIOPS", "WIOPS", "UTIL",
                    ]))
                    .block(block(format!(
                        "Disk Performance [{}]",
                        range(app.disk_scroll.offset, disks.len(), visible)
                    ))),
                    summary,
                );
            }
            DiskView::Topology => {
                let storage = app
                    .topology
                    .lock()
                    .map(|item| item.storage.clone())
                    .unwrap_or_default();
                let rows =
                    disks
                        .iter()
                        .enumerate()
                        .skip(app.disk_scroll.offset)
                        .map(|(index, item)| {
                            let mapping = storage.iter().find(|map| map.mapper == item.name);
                            let row = Row::new(vec![
                                item.name.clone(),
                                if item.name.starts_with("dm-") {
                                    "multipath"
                                } else {
                                    "block device"
                                }
                                .into(),
                                mapping
                                    .map(|map| map.wwid.clone())
                                    .unwrap_or_else(|| "Unknown".into()),
                                if mapping.is_some() {
                                    "Derived"
                                } else {
                                    "Host-observed"
                                }
                                .into(),
                                "host-side".into(),
                            ]);
                            if app.disk_selected == Some(index) {
                                row.style(Style::default().bg(Color::DarkGray).fg(Color::Yellow))
                            } else {
                                row
                            }
                        });
                frame.render_widget(
                    Table::new(
                        rows,
                        [
                            Constraint::Min(14),
                            Constraint::Length(16),
                            Constraint::Min(24),
                            Constraint::Length(16),
                            Constraint::Length(12),
                        ],
                    )
                    .header(Row::new(["DEVICE", "TYPE", "WWID", "CONFIDENCE", "SCOPE"]))
                    .block(block(format!(
                        "Disk Host Topology [{}]",
                        range(app.disk_scroll.offset, disks.len(), visible)
                    ))),
                    summary,
                );
            }
        }
    }
    if detail {
        let name = match &app.detail_target {
            DetailTarget::Disk(name) => name.clone(),
            _ => return,
        };
        let disk = app
            .disk
            .lock()
            .ok()
            .and_then(|items| items.iter().find(|item| item.name == name).cloned());
        let mut rows = vec![
            Row::new(vec![Cell::from("Device"), Cell::from(name.clone())]),
            Row::new(vec![
                Cell::from("Identity confidence"),
                Cell::from("Host-observed"),
            ]),
            Row::new(vec![
                Cell::from("Array metadata"),
                Cell::from("Unavailable (not queried)"),
            ]),
        ];
        if let Some(item) = disk {
            rows.push(Row::new(vec![
                Cell::from("Performance"),
                Cell::from(format!(
                    "read={} write={} riops={} wiops={} util={:.1}%",
                    bytes(item.read_bytes_per_sec),
                    bytes(item.write_bytes_per_sec),
                    item.read_iops,
                    item.write_iops,
                    item.utilization_percent
                )),
            ]));
        }
        frame.render_widget(
            Table::new(rows, [Constraint::Length(24), Constraint::Min(20)])
                .header(Row::new(["HOST DISK", "VALUE"]))
                .block(block("Disk Detail")),
            detail_area,
        );
    }
}

fn draw_fc(frame: &mut Frame, app: &mut App, area: Rect) {
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
        frame.render_widget(Paragraph::new(format!("HBA ports        {}\nOnline ports     {online}\nTargets          {}\nMultipath maps   {}\nHealthy          {healthy}\nDegraded         {degraded}\nFailed           {failed}\n\nScope: host-side FC and multipath only", snapshot.hosts.len(), snapshot.targets.len(), snapshot.maps.len())).block(block("FC/SAN Overview")), area);
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
    let (summary, detail_area) = detail_areas(area, detail);
    if let Some(summary) = summary {
        let visible = viewport(summary);
        app.fc_scroll.total = snapshot.maps.len();
        app.fc_scroll.viewport = visible;
        let rows = snapshot
            .maps
            .iter()
            .enumerate()
            .skip(app.fc_scroll.offset)
            .map(|(index, item)| {
                let row = Row::new(vec![
                    item.wwid.clone(),
                    item.mapper.clone(),
                    item.size.clone(),
                    format!("{}/{}", item.active_paths, item.total_paths),
                    item.status.clone(),
                    health(item.health).into(),
                    item.io
                        .as_ref()
                        .map(|io| bytes(io.read_bytes_per_sec))
                        .unwrap_or_else(|| "NO DATA".into()),
                    item.io
                        .as_ref()
                        .map(|io| bytes(io.write_bytes_per_sec))
                        .unwrap_or_else(|| "NO DATA".into()),
                ]);
                if app.fc_selected == Some(index) {
                    row.style(Style::default().bg(Color::DarkGray).fg(Color::Yellow))
                } else {
                    row
                }
            });
        frame.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Min(24),
                    Constraint::Length(10),
                    Constraint::Length(8),
                    Constraint::Length(8),
                    Constraint::Length(10),
                    Constraint::Length(10),
                    Constraint::Length(13),
                    Constraint::Length(13),
                ],
            )
            .header(Row::new([
                "WWID", "MAP", "SIZE", "PATHS", "STATUS", "HEALTH", "READ", "WRITE",
            ]))
            .block(block(format!(
                "Multipath [{}]",
                range(app.fc_scroll.offset, snapshot.maps.len(), visible)
            ))),
            summary,
        );
    }
    if detail {
        let map = app.fc_detail_map.and_then(|index| snapshot.maps.get(index));
        let mut rows = Vec::new();
        if let Some(map) = map {
            rows.extend([
                Row::new(vec![Cell::from("WWID"), Cell::from(map.wwid.clone())]),
                Row::new(vec![Cell::from("Mapper"), Cell::from(map.mapper.clone())]),
                Row::new(vec![Cell::from("Health"), Cell::from(health(map.health))]),
                Row::new(vec![
                    Cell::from("Paths"),
                    Cell::from(format!("{}/{}", map.active_paths, map.total_paths)),
                ]),
            ]);
            for path in &map.paths {
                rows.push(Row::new(vec![
                    Cell::from(format!("└─ {} / LUN {}", path.hctl, path.lun)),
                    Cell::from(format!(
                        "{} {} {}",
                        path.device, path.major_minor, path.state
                    )),
                ]));
                let host = snapshot
                    .hosts
                    .iter()
                    .find(|host| host.name == format!("host{}", path.host));
                let target = snapshot.targets.iter().find(|target| {
                    target.host == path.host
                        && target.channel == path.channel
                        && target.target == path.target
                });
                rows.push(Row::new(vec![
                    Cell::from("   ├─ HBA / WWPN"),
                    Cell::from(format!(
                        "host{} / {}",
                        path.host,
                        host.and_then(|item| item.port_wwn.as_deref())
                            .unwrap_or("Unknown")
                    )),
                ]));
                rows.push(Row::new(vec![
                    Cell::from("   └─ Target WWPN / Port ID"),
                    Cell::from(format!(
                        "{} / {}",
                        target
                            .and_then(|item| item.port_wwn.as_deref())
                            .unwrap_or("Unknown"),
                        target
                            .and_then(|item| item.port_id.as_deref())
                            .unwrap_or("Unknown")
                    )),
                ]));
            }
        }
        let visible = viewport(detail_area);
        app.fc_detail_scroll.total = rows.len();
        app.fc_detail_scroll.viewport = visible;
        frame.render_widget(
            Table::new(
                rows.into_iter().skip(app.fc_detail_scroll.offset),
                [Constraint::Length(30), Constraint::Min(20)],
            )
            .header(Row::new(["MULTIPATH MAP", "HOST-SIDE VALUE"]))
            .block(block(format!(
                "FC/SAN Detail [{}]",
                range(
                    app.fc_detail_scroll.offset,
                    app.fc_detail_scroll.total,
                    visible
                )
            ))),
            detail_area,
        );
    }
}

fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);
    let level = app
        .log_filter
        .min_level
        .map(log::level_label)
        .unwrap_or("ALL");
    let source = app
        .log_filter
        .source
        .map(log::source_label)
        .unwrap_or("ALL");
    let time = match app.log_filter.time_range {
        LogTimeRange::All => "ALL",
        LogTimeRange::LastMinute => "1m",
        LogTimeRange::LastFiveMinutes => "5m",
        LogTimeRange::LastFifteenMinutes => "15m",
        LogTimeRange::LastHour => "1h",
    };
    let search = if app.input_mode == InputMode::Search {
        format!("{}█", app.search_input)
    } else if app.log_filter.text.is_empty() {
        "-".into()
    } else {
        app.log_filter.text.clone()
    };
    frame.render_widget(
        Paragraph::new(format!(
            "Time: {time}   Level: {level}   Source: {source}   Text: {search}"
        ))
        .block(block("Memory-only Filters")),
        chunks[0],
    );
    let entries: Vec<_> = app
        .logs
        .iter()
        .filter(|entry| app.log_filter.matches(entry))
        .cloned()
        .collect();
    let visible = viewport(chunks[1]);
    app.log_scroll.total = entries.len();
    app.log_scroll.viewport = visible;
    app.log_scroll.offset = app
        .log_scroll
        .offset
        .min(entries.len().saturating_sub(visible));
    let rows = entries.iter().skip(app.log_scroll.offset).map(|entry| {
        Row::new(vec![
            log::timestamp(entry),
            log::level_label(entry.level).into(),
            log::source_label(entry.source).into(),
            entry.message.clone(),
        ])
    });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(10),
                Constraint::Length(7),
                Constraint::Length(10),
                Constraint::Min(20),
            ],
        )
        .header(Row::new(["TIME", "LEVEL", "SOURCE", "MESSAGE"]))
        .block(block(format!(
            "Logs [{}]",
            range(app.log_scroll.offset, entries.len(), visible)
        ))),
        chunks[1],
    );
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints = match app.workspace {
        Workspace::Overview => "1-5: workspace  ?: help  q: quit",
        Workspace::Domains => "j/k: navigate  l/Enter: detail  h/Esc: back  ?: help",
        Workspace::Network => "j/k: navigate  v: performance/topology  l: detail  h: back",
        Workspace::Disk => "j/k: navigate  v: performance/topology  l: detail  h: back",
        Workspace::Logs => {
            "j/k: scroll  /: search  f: level  F: source  t: time  c: clear filters  4: back"
        }
        Workspace::FcSan => "j/k: select  l: detail  h: back  u/d: preview  o: views  4: logs",
    };
    frame.render_widget(
        Paragraph::new(hints).block(block(format!("Mode: {:?}", app.input_mode))),
        area,
    );
}

fn popup(frame: &Frame, width: u16, height: u16) -> Rect {
    let area = frame.area();
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    )
}

fn draw_help(frame: &mut Frame) {
    let area = popup(frame, 72, 18);
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new("GLOBAL\n  0 Overview | 1 Domains | 2 Network | 3 Disk | 4 Logs | 5 FC/SAN\n  q quit | Esc back | ? close help\n\nNAVIGATION\n  j/k or arrows navigate | l/Enter detail | h/Esc back\n  u/d preview FC map | o FC/SAN subviews | v Network/Disk subview\n\nLOGS (memory-only)\n  / text search | f level | F source | t time range | c clear filters\n\nSTATUS\n  LIVE / STALE / FALLBACK / NO DATA; Unknown means host-side evidence is absent.").wrap(Wrap { trim: false }).block(block("Context Help")), area);
}

fn draw_fc_menu(frame: &mut Frame, app: &App) {
    let area = popup(frame, 45, 12);
    frame.render_widget(Clear, area);
    let options = [
        "Overview",
        "Ports",
        "Targets",
        "Multipath",
        "Toggle path detail",
    ];
    let rows = options.iter().enumerate().map(|(index, label)| {
        let row = Row::new(vec![
            if index == app.fc_config_index {
                ">"
            } else {
                " "
            },
            *label,
        ]);
        if index == app.fc_config_index {
            row.style(Style::default().fg(Color::Yellow))
        } else {
            row
        }
    });
    frame.render_widget(
        Table::new(rows, [Constraint::Length(3), Constraint::Min(20)])
            .block(block("FC/SAN View (Enter apply, Esc cancel)")),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visible_ranges_are_unambiguous() {
        assert_eq!(range(0, 0, 5), "0/0");
        assert_eq!(range(0, 5, 5), "1-5/5");
        assert_eq!(range(3, 23, 4), "4-7/23");
    }
    #[test]
    fn responsive_detail_modes() {
        assert!(detail_areas(Rect::new(0, 0, 60, 15), true).0.is_none());
        assert!(detail_areas(Rect::new(0, 0, 100, 30), true).0.is_some());
    }
}
