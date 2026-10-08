use super::{
    layout::{compact, master_detail, range, viewport},
    topology,
    widgets::{block, bytes},
};
use crate::app::{App, DetailTarget, NetworkView};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Style},
    widgets::{Row, Table},
};

pub(super) fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let detail = matches!(app.detail_target, DetailTarget::Network(_));
    let layout = master_detail(area, detail);
    if let Some(summary) = layout.master {
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
                let narrow = compact(summary);
                let rows = items
                    .iter()
                    .enumerate()
                    .skip(app.network_scroll.offset)
                    .map(|(index, item)| {
                        let row = if narrow {
                            Row::new(vec![
                                item.name.clone(),
                                bytes(item.rx_bytes_per_sec),
                                bytes(item.tx_bytes_per_sec),
                            ])
                        } else {
                            Row::new(vec![
                                item.name.clone(),
                                bytes(item.rx_bytes_per_sec),
                                bytes(item.tx_bytes_per_sec),
                                item.rx_packets_per_sec.to_string(),
                                item.tx_packets_per_sec.to_string(),
                                item.rx_drops_per_sec.to_string(),
                                item.tx_drops_per_sec.to_string(),
                            ])
                        };
                        if app.network_selected == Some(index) {
                            row.style(Style::default().bg(Color::DarkGray).fg(Color::Yellow))
                        } else {
                            row
                        }
                    });
                let table = if narrow {
                    Table::new(
                        rows,
                        [
                            Constraint::Min(12),
                            Constraint::Length(13),
                            Constraint::Length(13),
                        ],
                    )
                    .header(Row::new(["IFACE", "RX", "TX"]))
                } else {
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
                };
                frame.render_widget(
                    table.block(block(format!(
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
        draw_network_topology(frame, app, layout.detail, true);
    }
}

fn draw_network_topology(frame: &mut Frame, app: &mut App, area: Rect, selected_only: bool) {
    let snapshot = app
        .topology
        .lock()
        .map(|item| item.clone())
        .unwrap_or_default();
    let selected = match &app.detail_target {
        DetailTarget::Network(name) if selected_only => Some(name.as_str()),
        _ => None,
    };
    let scroll = if selected_only {
        &mut app.network_detail_scroll
    } else {
        &mut app.network_scroll
    };
    topology::render(
        frame,
        &snapshot.network_ui,
        area,
        scroll,
        "Network Topology",
        selected,
    );
}
