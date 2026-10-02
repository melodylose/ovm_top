use crate::app::App;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
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

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
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
        Cell::from("NAME"),
        Cell::from("STATE"),
        Cell::from("CPU%"),
        Cell::from("MEM%"),
        Cell::from("VCPUS"),
        Cell::from("NET TX"),
        Cell::from("NET RX"),
        Cell::from("VBD RD"),
        Cell::from("VBD WR"),
    ]);

    let domains = app.domains.lock().unwrap();

    let rows = domains.iter().map(|d| {
        Row::new(vec![
            Cell::from(d.name.clone()),
            Cell::from(state_label(&d.state)),
            Cell::from(format!("{:.1}", d.cpu_percent)),
            Cell::from(format!("{:.1}", d.memory_percent)),
            Cell::from(d.vcpus.to_string()),
            Cell::from(format!("{}", d.net_tx_kb)),
            Cell::from(format!("{}", d.net_rx_kb)),
            Cell::from(format!("{}", d.vbd_rd)),
            Cell::from(format!("{}", d.vbd_wr)),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(22),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(Block::default().title(" Domains ").borders(Borders::ALL));

    let network_header = Row::new(vec![
        Cell::from("IFCAE"),
        Cell::from("RX"),
        Cell::from("TX"),
        Cell::from("RX PPS"),
        Cell::from("TX PPS"),
        Cell::from("RX DROP"),
        Cell::from("TX DROP"),
    ]);

    let network = app.network.lock().unwrap();

    let network_rows = network.iter().map(|n| {
        Row::new(vec![
            Cell::from(n.name.clone()),
            Cell::from(format_bytes_per_sec(n.rx_bytes_per_sec)),
            Cell::from(format_bytes_per_sec(n.tx_bytes_per_sec)),
            Cell::from(n.rx_packets_per_sec.to_string()),
            Cell::from(n.tx_packets_per_sec.to_string()),
            Cell::from(n.rx_drops_per_sec.to_string()),
            Cell::from(n.tx_drops_per_sec.to_string()),
        ])
    });

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
            .title(" Host Network ")
            .borders(Borders::ALL),
    );

    let footer = Paragraph::new("q: quit").block(Block::default().borders(Borders::ALL));

    frame.render_widget(host, chunks[0]);
    frame.render_widget(table, chunks[1]);
    frame.render_widget(network_table, chunks[2]);
    frame.render_widget(footer, chunks[3]);
}
