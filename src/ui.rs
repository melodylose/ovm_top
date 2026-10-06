use crate::{
    app::{App, FcView, Focus, LogFilter},
    log,
};

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table},
};

fn format_bytes_per_sec(value: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    let value_f = value as f64;

    if value_f >= GIB {
        format!("{:.1} GiB/s", value_f / GIB)
    } else if value_f >= MIB {
        format!("{:.1} MiB/s", value_f / MIB)
    } else if value_f >= KIB {
        format!("{:.1} KiB/s", value_f / KIB)
    } else {
        format!("{} B/s", value)
    }
}

fn format_percent(value: f64) -> String {
    format!("{value:.1}%")
}

fn panel_title(number: u8, title: &str, focused: bool) -> String {
    if focused {
        format!("[{number}] {title} *")
    } else {
        format!("[{number}] {title}")
    }
}

fn table_viewport(panel: Rect) -> usize {
    // One row for the top border, one for the table header, and one for the
    // bottom border. Scrolling remains internal to the table.
    panel.height.saturating_sub(3) as usize
}

fn panel_border_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn state_label(state: &str) -> &'static str {
    if state.contains('r') {
        "Running"
    } else if state.contains('b') {
        "Blocked"
    } else if state.contains('p') {
        "Paused"
    } else if state.contains('c') {
        "Crashed"
    } else if state.contains('d') {
        "Dying"
    } else {
        "Unknown"
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let constraints = if app.show_logs {
        vec![
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Min(8),
            Constraint::Min(5),
            Constraint::Length(3),
        ]
    } else {
        vec![
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Min(8),
            Constraint::Length(3),
        ]
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    let host_text = format!(
        "Host: {}\n\
         Xen: {}\n\
         CPU: {}\n\
         NUMA: {}\n\
         RAM: {} MB\n\
         Free RAM: {} MB\n\
         Scheduler: {}",
        app.xm_info.host,
        app.xm_info.xen_version,
        app.xm_info.nr_cpus,
        app.xm_info.nr_nodes,
        app.xm_info.total_memory_mb,
        app.xm_info.free_memory_mb,
        app.xm_info.scheduler,
    );

    let host =
        Paragraph::new(host_text).block(Block::default().title(" Xen Host ").borders(Borders::ALL));

    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("NAME"),
        Cell::from("STATE"),
        Cell::from("CPU%"),
        Cell::from("MEM"),
        Cell::from("MEM%"),
        Cell::from("VCPUS"),
        Cell::from("NET TX"),
        Cell::from("NET RX"),
        Cell::from("VBD RD"),
        Cell::from("VBD WR"),
    ]);

    let domain_viewport = table_viewport(chunks[1]);
    let domain_total = app.domains.lock().unwrap().len();
    app.set_scroll_metrics(Focus::Domains, domain_total, domain_viewport);
    let domain_scroll = app.domain_scroll.offset;

    let rows = {
        let domains = app.domains.lock().unwrap();
        domains
            .iter()
            .skip(domain_scroll)
            .map(|d| {
                Row::new(vec![
                    Cell::from(d.id.to_string()),
                    Cell::from(d.name.clone()),
                    Cell::from(state_label(&d.state)),
                    Cell::from(format!("{:.1}", d.cpu_percent)),
                    Cell::from(format!("{} MB", d.memory_mb)),
                    Cell::from(format!("{:.1}", d.memory_percent)),
                    Cell::from(d.vcpus.to_string()),
                    Cell::from(format!("{}", d.net_tx_kb)),
                    Cell::from(format!("{}", d.net_rx_kb)),
                    Cell::from(format!("{}", d.vbd_rd)),
                    Cell::from(format!("{}", d.vbd_wr)),
                ])
            })
            .collect::<Vec<_>>()
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Percentage(22),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(panel_title(1, "Domains", app.focus == Focus::Domains))
            .border_style(panel_border_style(app.focus == Focus::Domains))
            .title_style(panel_border_style(app.focus == Focus::Domains))
            .borders(Borders::ALL),
    );

    let network_header = Row::new(vec![
        Cell::from("IFACE"),
        Cell::from("RX"),
        Cell::from("TX"),
        Cell::from("RX PPS"),
        Cell::from("TX PPS"),
        Cell::from("RX DROP"),
        Cell::from("TX DROP"),
    ]);

    let network_viewport = table_viewport(chunks[2]);
    let network_total = app.network.lock().unwrap().len();
    app.set_scroll_metrics(Focus::Network, network_total, network_viewport);
    let network_scroll = app.network_scroll.offset;

    let network_rows = {
        let network = app.network.lock().unwrap();
        network
            .iter()
            .skip(network_scroll)
            .map(|n| {
                Row::new(vec![
                    Cell::from(n.name.clone()),
                    Cell::from(format_bytes_per_sec(n.rx_bytes_per_sec)),
                    Cell::from(format_bytes_per_sec(n.tx_bytes_per_sec)),
                    Cell::from(n.rx_packets_per_sec.to_string()),
                    Cell::from(n.tx_packets_per_sec.to_string()),
                    Cell::from(n.rx_drops_per_sec.to_string()),
                    Cell::from(n.tx_drops_per_sec.to_string()),
                ])
            })
            .collect::<Vec<_>>()
    };

    let network_table = Table::new(
        network_rows,
        [
            Constraint::Length(18),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(network_header)
    .block(
        Block::default()
            .title(panel_title(2, "Host Network", app.focus == Focus::Network))
            .border_style(panel_border_style(app.focus == Focus::Network))
            .title_style(panel_border_style(app.focus == Focus::Network))
            .borders(Borders::ALL),
    );

    let disk_header = Row::new(vec![
        Cell::from("DEVICE"),
        Cell::from("READ"),
        Cell::from("WRITE"),
        Cell::from("READ IOPS"),
        Cell::from("WRITE IOPS"),
        Cell::from("UTIL"),
    ]);
    let disk_viewport = table_viewport(chunks[3]);
    let disk_total = app.disk.lock().unwrap().len();
    app.set_scroll_metrics(Focus::Disk, disk_total, disk_viewport);
    let disk_scroll = app.disk_scroll.offset;
    let disk_rows = {
        let disk = app.disk.lock().unwrap();
        disk.iter()
            .skip(disk_scroll)
            .map(|d| {
                Row::new(vec![
                    Cell::from(d.name.clone()),
                    Cell::from(format_bytes_per_sec(d.read_bytes_per_sec)),
                    Cell::from(format_bytes_per_sec(d.write_bytes_per_sec)),
                    Cell::from(d.read_iops.to_string()),
                    Cell::from(d.write_iops.to_string()),
                    Cell::from(format_percent(d.utilization_percent)),
                ])
            })
            .collect::<Vec<_>>()
    };
    let disk_table = Table::new(
        disk_rows,
        [
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
        ],
    )
    .header(disk_header)
    .block(
        Block::default()
            .title(panel_title(3, "Host Disk I/O", app.focus == Focus::Disk))
            .border_style(panel_border_style(app.focus == Focus::Disk))
            .title_style(panel_border_style(app.focus == Focus::Disk))
            .borders(Borders::ALL),
    );

    let detail_layout =
        app.fc_panel_mode == crate::app::FcPanelMode::Detail && chunks[4].width >= 90;
    let fc_areas = if detail_layout {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
            .split(chunks[4])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100)])
            .split(chunks[4])
    };
    let fc_summary_area = fc_areas[0];
    let fc_detail_area = if detail_layout {
        fc_areas[1]
    } else {
        chunks[4]
    };
    let fc_viewport = table_viewport(if detail_layout {
        fc_summary_area
    } else {
        chunks[4]
    });
    let fc = app.fc.lock().unwrap();
    let selected_fc = if app.fc_panel_mode == crate::app::FcPanelMode::Detail {
        app.fc_detail_map.unwrap_or(0)
    } else {
        app.fc_selected.unwrap_or(0)
    };
    let compact_fc_rows = fc
        .maps
        .iter()
        .enumerate()
        .map(|(index, map)| {
            let row = Row::new(vec![
                Cell::from(map.wwid.clone()),
                Cell::from(map.mapper.clone()),
                Cell::from(map.size.clone()),
                Cell::from(format!("{}/{}", map.active_paths, map.total_paths)),
                Cell::from(map.status.clone()),
            ]);
            if index == selected_fc {
                row.style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                row
            }
        })
        .collect::<Vec<_>>();
    let (fc_title, fc_total, fc_header, fc_rows) = match app.fc_config.view {
        FcView::Ports => (
            "Ports",
            fc.hosts.len(),
            vec!["HOST", "PORT WWN", "STATE", "SPEED", "FABRIC"],
            fc.hosts
                .iter()
                .map(|host| {
                    Row::new(vec![
                        Cell::from(host.name.clone()),
                        Cell::from(host.port_wwn.clone().unwrap_or_else(|| "-".into())),
                        Cell::from(host.port_state.clone().unwrap_or_else(|| "-".into())),
                        Cell::from(host.speed.clone().unwrap_or_else(|| "-".into())),
                        Cell::from(host.fabric_name.clone().unwrap_or_else(|| "-".into())),
                    ])
                })
                .collect::<Vec<_>>(),
        ),
        FcView::Targets => (
            "Targets",
            fc.targets.len(),
            vec!["TARGET", "HOST", "PORT WWN", "NODE WWN", "PORT ID"],
            fc.targets
                .iter()
                .map(|target| {
                    Row::new(vec![
                        Cell::from(target.name.clone()),
                        Cell::from(format!("host{}", target.host)),
                        Cell::from(target.port_wwn.clone().unwrap_or_else(|| "-".into())),
                        Cell::from(target.node_wwn.clone().unwrap_or_else(|| "-".into())),
                        Cell::from(target.port_id.clone().unwrap_or_else(|| "-".into())),
                    ])
                })
                .collect::<Vec<_>>(),
        ),
        FcView::Multipath => (
            "Multipath",
            fc.maps.len(),
            vec![
                "WWID", "MAP", "SIZE", "PATHS", "STATUS", "READ", "WRITE", "RIOPS", "WIOPS", "UTIL",
            ],
            fc.maps
                .iter()
                .enumerate()
                .map(|(index, map)| {
                    let (read, write, riops, wiops, util) = map.io.as_ref().map_or_else(
                        || {
                            (
                                "N/A".to_string(),
                                "N/A".to_string(),
                                "N/A".to_string(),
                                "N/A".to_string(),
                                "N/A".to_string(),
                            )
                        },
                        |io| {
                            (
                                format_bytes_per_sec(io.read_bytes_per_sec),
                                format_bytes_per_sec(io.write_bytes_per_sec),
                                io.read_iops.to_string(),
                                io.write_iops.to_string(),
                                format_percent(io.utilization_percent),
                            )
                        },
                    );
                    let row = Row::new(vec![
                        Cell::from(map.wwid.clone()),
                        Cell::from(map.mapper.clone()),
                        Cell::from(map.size.clone()),
                        Cell::from(format!("{}/{}", map.active_paths, map.total_paths)),
                        Cell::from(map.status.clone()),
                        Cell::from(read),
                        Cell::from(write),
                        Cell::from(riops),
                        Cell::from(wiops),
                        Cell::from(util),
                    ]);
                    if index == selected_fc {
                        row.style(
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        row
                    }
                })
                .collect::<Vec<_>>(),
        ),
    };
    drop(fc);
    if app.fc_panel_mode == crate::app::FcPanelMode::Detail {
        app.fc_scroll.total = fc_total;
        app.fc_scroll.viewport = fc_viewport;
        app.fc_scroll.offset = app
            .fc_scroll
            .offset
            .min(fc_total.saturating_sub(fc_viewport));
    } else {
        app.set_scroll_metrics(Focus::Fc, fc_total, fc_viewport);
    }
    let fc_scroll = app.fc_scroll.offset;
    let fc_table = Table::new(
        fc_rows.into_iter().skip(fc_scroll),
        fc_header
            .iter()
            .map(|_| Constraint::Min(10))
            .collect::<Vec<_>>(),
    )
    .header(Row::new(
        fc_header.into_iter().map(Cell::from).collect::<Vec<_>>(),
    ))
    .block(
        Block::default()
            .title(panel_title(
                5,
                &format!("FC/SAN - {fc_title}"),
                app.focus == Focus::Fc,
            ))
            .border_style(panel_border_style(app.focus == Focus::Fc))
            .title_style(panel_border_style(app.focus == Focus::Fc))
            .borders(Borders::ALL),
    );
    let log_table = if app.show_logs {
        let filter_label = match app.log_filter {
            LogFilter::All => "ALL",
            LogFilter::WarningsAndErrors => "WARN+",
            LogFilter::ErrorsOnly => "ERROR",
        };
        let log_total = app
            .logs
            .iter()
            .filter(|entry| app.log_filter.matches(entry.level))
            .count();
        let log_viewport = table_viewport(chunks[5]);
        app.set_scroll_metrics(Focus::Logs, log_total, log_viewport);
        let log_scroll = app.log_scroll.offset;
        let mut entries: Vec<_> = app
            .logs
            .iter()
            .filter(|entry| app.log_filter.matches(entry.level))
            .cloned()
            .collect();
        entries.reverse();
        let rows = entries
            .iter()
            .skip(log_scroll)
            .take(log_viewport)
            .map(|entry| {
                Row::new(vec![
                    Cell::from(log::timestamp(entry)),
                    Cell::from(log::level_label(entry.level)),
                    Cell::from(log::source_label(entry.source)),
                    Cell::from(entry.message.clone()),
                ])
            });
        Some(
            Table::new(
                rows,
                [
                    Constraint::Length(10),
                    Constraint::Length(7),
                    Constraint::Length(8),
                    Constraint::Min(20),
                ],
            )
            .header(Row::new(vec![
                Cell::from("TIME"),
                Cell::from("LEVEL"),
                Cell::from("SOURCE"),
                Cell::from("MESSAGE"),
            ]))
            .block(
                Block::default()
                    .title(format!(
                        "{} [{filter_label}]",
                        panel_title(4, "Logs", app.focus == Focus::Logs)
                    ))
                    .border_style(panel_border_style(app.focus == Focus::Logs))
                    .title_style(panel_border_style(app.focus == Focus::Logs))
                    .borders(Borders::ALL),
            ),
        )
    } else {
        None
    };

    let focus_label = match app.focus {
        Focus::Domains => "Domains",
        Focus::Network => "Network",
        Focus::Disk => "Disk",
        Focus::Fc => "FC/SAN",
        Focus::Logs => "Logs",
    };
    let shortcuts =
        if app.focus == Focus::Fc && app.fc_panel_mode == crate::app::FcPanelMode::Detail {
            "h: hide detail  u/d: preview map  j/k: scroll tree  Esc: back"
        } else if app.focus == Focus::Fc {
            "h: hide detail  j/k: select  l: show detail  4: logs  o: FC config"
        } else {
            "1/2/3/5: panels  4: logs  j/k: scroll  q: quit"
        };
    let footer = Paragraph::new(format!("focus: {focus_label}  {shortcuts}"))
        .block(Block::default().borders(Borders::ALL));

    frame.render_widget(host, chunks[0]);
    frame.render_widget(table, chunks[1]);
    frame.render_widget(network_table, chunks[2]);
    frame.render_widget(disk_table, chunks[3]);
    if app.fc_panel_mode == crate::app::FcPanelMode::Detail {
        frame.render_widget(Clear, fc_summary_area);
        frame.render_widget(Clear, fc_detail_area);
        let summary_table = Table::new(
            compact_fc_rows.into_iter().skip(app.fc_scroll.offset),
            [
                Constraint::Min(18),
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Length(7),
                Constraint::Length(10),
            ],
        )
        .header(Row::new(vec![
            Cell::from("WWID"),
            Cell::from("MAP"),
            Cell::from("SIZE"),
            Cell::from("PATHS"),
            Cell::from("STATUS"),
        ]))
        .block(
            Block::default()
                .title(panel_title(5, "FC/SAN Summary", app.focus == Focus::Fc))
                .border_style(panel_border_style(app.focus == Focus::Fc))
                .title_style(panel_border_style(app.focus == Focus::Fc))
                .borders(Borders::ALL),
        );
        if detail_layout {
            frame.render_widget(summary_table, fc_summary_area);
        }
        let mut rendered_detail = false;
        if let Some(map_index) = app.fc_detail_map {
            let detail_map = app
                .fc
                .lock()
                .ok()
                .and_then(|snapshot| snapshot.maps.get(map_index).cloned());
            if let Some(map) = detail_map {
                let detail_viewport = table_viewport(fc_detail_area);
                let mut tree_rows = vec![
                    Row::new(vec![Cell::from("└─ Multipath Map"), Cell::from("")]),
                    Row::new(vec![Cell::from("   ├─ WWID"), Cell::from(map.wwid.clone())]),
                    Row::new(vec![
                        Cell::from("   ├─ Mapper"),
                        Cell::from(map.mapper.clone()),
                    ]),
                    Row::new(vec![
                        Cell::from("   ├─ Vendor"),
                        Cell::from(map.vendor.clone()),
                    ]),
                    Row::new(vec![
                        Cell::from("   ├─ Model"),
                        Cell::from(map.model.clone()),
                    ]),
                    Row::new(vec![Cell::from("   ├─ Size"), Cell::from(map.size.clone())]),
                    Row::new(vec![
                        Cell::from("   ├─ Status"),
                        Cell::from(map.status.clone()),
                    ]),
                    Row::new(vec![
                        Cell::from("   ├─ Paths"),
                        Cell::from(format!("{}/{}", map.active_paths, map.total_paths)),
                    ]),
                ];
                let io_values = map.io.as_ref().map_or_else(
                    || {
                        vec![
                            ("Read", "N/A".to_string()),
                            ("Write", "N/A".to_string()),
                            ("Read IOPS", "N/A".to_string()),
                            ("Write IOPS", "N/A".to_string()),
                            ("Utilization", "N/A".to_string()),
                        ]
                    },
                    |io| {
                        vec![
                            ("Read", format_bytes_per_sec(io.read_bytes_per_sec)),
                            ("Write", format_bytes_per_sec(io.write_bytes_per_sec)),
                            ("Read IOPS", io.read_iops.to_string()),
                            ("Write IOPS", io.write_iops.to_string()),
                            ("Utilization", format_percent(io.utilization_percent)),
                        ]
                    },
                );
                tree_rows.push(Row::new(vec![Cell::from("   ├─ Disk I/O"), Cell::from("")]));
                for (index, (label, value)) in io_values.into_iter().enumerate() {
                    let branch = if index == 4 {
                        "   │  └─ "
                    } else {
                        "   │  ├─ "
                    };
                    tree_rows.push(Row::new(vec![
                        Cell::from(format!("{branch}{label}")),
                        Cell::from(value),
                    ]));
                }
                tree_rows.push(Row::new(vec![Cell::from("   └─ Paths"), Cell::from("")]));
                for (index, path) in map.paths.iter().enumerate() {
                    let path_branch = if index + 1 == map.paths.len() {
                        "      └─ "
                    } else {
                        "      ├─ "
                    };
                    tree_rows.push(Row::new(vec![
                        Cell::from(format!("{path_branch}{}", path.hctl)),
                        Cell::from(path.state.clone()),
                    ]));
                    let detail_branch = if index + 1 == map.paths.len() {
                        "         "
                    } else {
                        "      │  "
                    };
                    tree_rows.push(Row::new(vec![
                        Cell::from(format!("{detail_branch}├─ Device")),
                        Cell::from(path.device.clone()),
                    ]));
                    tree_rows.push(Row::new(vec![
                        Cell::from(format!("{detail_branch}└─ Major:Minor")),
                        Cell::from(path.major_minor.clone()),
                    ]));
                }
                app.set_scroll_metrics(Focus::Fc, tree_rows.len(), detail_viewport);
                let detail_scroll = app.fc_detail_scroll.offset;
                let detail_table = Table::new(
                    tree_rows.into_iter().skip(detail_scroll),
                    [Constraint::Length(32), Constraint::Min(24)],
                )
                .header(Row::new(vec![
                    Cell::from("FC/SAN TREE"),
                    Cell::from("VALUE"),
                ]))
                .block(
                    Block::default()
                        .title(panel_title(5, "FC/SAN Detail", app.focus == Focus::Fc))
                        .border_style(panel_border_style(app.focus == Focus::Fc))
                        .title_style(panel_border_style(app.focus == Focus::Fc))
                        .borders(Borders::ALL),
                );
                frame.render_widget(detail_table, fc_detail_area);
                rendered_detail = true;
            }
        }
        if !rendered_detail {
            frame.render_widget(
                Paragraph::new("No FC/SAN detail available").block(
                    Block::default()
                        .title("[5] FC/SAN Detail")
                        .borders(Borders::ALL),
                ),
                fc_detail_area,
            );
        }
    } else {
        frame.render_widget(fc_table, chunks[4]);
    }
    if let Some(log_table) = log_table {
        frame.render_widget(log_table, chunks[5]);
        frame.render_widget(footer, chunks[6]);
    } else {
        frame.render_widget(footer, chunks[5]);
    }

    if app.fc_config_open {
        let area = frame.area();
        let menu_area = Rect::new(
            area.x.saturating_add(area.width.saturating_sub(36) / 2),
            area.y.saturating_add(area.height.saturating_sub(10) / 2),
            36.min(area.width),
            10.min(area.height),
        );
        let options = [
            "Ports",
            "Targets",
            "Multipath",
            if app.fc_config.show_path_detail {
                "Path detail: Visible"
            } else {
                "Path detail: Hidden"
            },
        ];
        let mut text = String::from("FC/SAN Configuration\n\n");
        for (index, option) in options.iter().enumerate() {
            text.push_str(if index == app.fc_config_index {
                "> "
            } else {
                "  "
            });
            text.push_str(option);
            text.push('\n');
        }
        text.push_str("\nEnter: apply  Esc: cancel");
        frame.render_widget(Clear, menu_area);
        frame.render_widget(
            Paragraph::new(text).block(Block::default().title(" Config ").borders(Borders::ALL)),
            menu_area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_viewport_excludes_border_and_header() {
        assert_eq!(table_viewport(Rect::new(10, 20, 100, 12)), 9);
        assert_eq!(table_viewport(Rect::new(10, 20, 100, 3)), 0);
    }
}
