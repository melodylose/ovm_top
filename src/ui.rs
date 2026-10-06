use crate::{
    app::{App, Focus, LogFilter},
    log,
};

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
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
            Constraint::Min(5),
            Constraint::Length(3),
        ]
    } else {
        vec![
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Length(8),
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
        let log_viewport = table_viewport(chunks[4]);
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
        Focus::Logs => "Logs",
    };
    let footer = Paragraph::new(format!(
        "focus: {focus_label}  1/2/3: panels  4/l: logs  j/k: scroll  f: filter  c: clear  s: save  q: quit"
    ))
        .block(Block::default().borders(Borders::ALL));

    frame.render_widget(host, chunks[0]);
    frame.render_widget(table, chunks[1]);
    frame.render_widget(network_table, chunks[2]);
    frame.render_widget(disk_table, chunks[3]);
    if let Some(log_table) = log_table {
        frame.render_widget(log_table, chunks[4]);
        frame.render_widget(footer, chunks[5]);
    } else {
        frame.render_widget(footer, chunks[4]);
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
