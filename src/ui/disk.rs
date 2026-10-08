use super::{
    layout::{compact, master_detail, range, viewport},
    topology,
    widgets::{block, bytes},
};
use crate::app::{App, DetailTarget, DiskView};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Style},
    widgets::{Cell, Row, Table},
};

pub(super) fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let detail = matches!(app.detail_target, DetailTarget::Disk(_));
    let layout = master_detail(area, detail);
    if let Some(summary) = layout.master {
        let disks = app
            .disk
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default();
        let visible = viewport(summary);
        let narrow = compact(summary);
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
                            let row = if narrow {
                                Row::new(vec![
                                    item.name.clone(),
                                    bytes(item.read_bytes_per_sec),
                                    bytes(item.write_bytes_per_sec),
                                    format!("{:.1}%", item.utilization_percent),
                                ])
                            } else {
                                Row::new(vec![
                                    item.name.clone(),
                                    bytes(item.read_bytes_per_sec),
                                    bytes(item.write_bytes_per_sec),
                                    item.read_iops.to_string(),
                                    item.write_iops.to_string(),
                                    format!("{:.1}%", item.utilization_percent),
                                ])
                            };
                            if app.disk_selected == Some(index) {
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
                                Constraint::Min(14),
                                Constraint::Length(13),
                                Constraint::Length(13),
                                Constraint::Length(8),
                            ],
                        )
                        .header(Row::new(["DEVICE", "READ", "WRITE", "UTIL"]))
                    } else {
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
                    }
                    .block(block(format!(
                        "Disk Performance [{}]",
                        range(app.disk_scroll.offset, disks.len(), visible)
                    ))),
                    summary,
                );
            }
            DiskView::Topology => {
                let state = app
                    .topology
                    .lock()
                    .map(|item| item.storage_ui.clone())
                    .unwrap_or_default();
                topology::render(
                    frame,
                    &state,
                    summary,
                    &mut app.disk_scroll,
                    "Storage Topology",
                    None,
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
            layout.detail,
        );
    }
}
