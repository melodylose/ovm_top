use super::{
    layout::full_screen,
    widgets::{block, compact_layer_status, layer_status, state},
};
use crate::{
    app::App,
    topology::snapshot::{StorageHealth, StorageRedundancy},
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::{Paragraph, Wrap},
};

pub(super) fn render(frame: &mut Frame, app: &App, area: Rect) {
    let area = full_screen(area);
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
    let domain_meta = app
        .domain_meta
        .lock()
        .map(|meta| meta.clone())
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
    let single = fc
        .maps
        .iter()
        .filter(|map| map.redundancy == StorageRedundancy::Single)
        .count();
    let redundant = fc
        .maps
        .iter()
        .filter(|map| map.redundancy == StorageRedundancy::Redundant)
        .count();
    let chunks = if area.height < 20 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(34),
                Constraint::Percentage(33),
                Constraint::Percentage(33),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(6),
                Constraint::Length(6),
                Constraint::Min(8),
            ])
            .split(area)
    };
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
            "VM: {}   Running: {running}   Inventory: {}   Realtime: {}\nVBD: {} ({})   Mapping: {}   VIF: {} ({})",
            domains.len(),
            layer_status(&domain_meta.layers.inventory),
            layer_status(&domain_meta.layers.realtime),
            topology.block_devices.len(),
            layer_status(&topology.layers.vbd),
            layer_status(&topology.layers.storage_mapping),
            topology.vifs.len(),
            layer_status(&topology.layers.vif),
        ))
        .block(block("Domains Summary")),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(format!(
            "Network interfaces: {} ({})\nHost disks: {} ({})\nFC ports: {online}/{} online ({})\nTargets: {} ({})\nMultipath: {} healthy, {degraded} degraded; {single} single, {redundant} redundant, {} total ({})\nStorage metadata: host-side only; array-side unavailable",
            topology.interfaces.len(),
            compact_layer_status(&topology.layers.network_inventory).text(),
            app.disk.lock().map(|items| items.len()).unwrap_or_default(),
            layer_status(&topology.layers.block_inventory),
            fc.hosts.len(),
            layer_status(&fc.layers.fc_hba),
            fc.targets.len(),
            layer_status(&fc.layers.fc_transport),
            healthy,
            fc.maps.len(),
            layer_status(&fc.layers.multipath),
        ))
        .block(block("Infrastructure")),
        chunks[2],
    );
}
