use super::{
    layout::{compact, master_detail, range, viewport},
    widgets::{block, state, status},
};
use crate::{
    app::{App, DetailTarget},
    topology::snapshot::MappingConfidence,
};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Cell, Row, Table},
};

pub(super) fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let detail = matches!(app.detail_target, DetailTarget::Domain(_));
    let layout = master_detail(area, detail);
    if let Some(summary) = layout.master {
        let visible = viewport(summary);
        let narrow = compact(summary);
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
                let row = if narrow {
                    Row::new(vec![
                        item.name.clone(),
                        state(&item.state).into(),
                        format!("{:.1}", item.cpu_percent),
                        format!("{:.1}", item.memory_percent),
                        item.vcpus.to_string(),
                    ])
                } else {
                    Row::new(vec![
                        item.id.to_string(),
                        item.name.clone(),
                        state(&item.state).into(),
                        format!("{:.1}", item.cpu_percent),
                        format!("{:.1}", item.memory_percent),
                        item.vcpus.to_string(),
                        item.memory_mb.to_string(),
                    ])
                };
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
            if narrow {
                Table::new(
                    rows,
                    [
                        Constraint::Min(18),
                        Constraint::Length(9),
                        Constraint::Length(7),
                        Constraint::Length(7),
                        Constraint::Length(5),
                    ],
                )
                .header(Row::new(["NAME", "STATE", "CPU%", "MEM%", "VCPU"]))
            } else {
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
            }
            .block(block(format!(
                "Domains [{}] [{freshness}]",
                range(app.domain_scroll.offset, domains.len(), visible),
            ))),
            summary,
        );
    }
    if detail {
        draw_domain_detail(frame, app, layout.detail);
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
        let value_width = usize::from(area.width.saturating_sub(26).max(1));
        macro_rules! detail {
            ($label:expr, $value:expr $(,)?) => {
                detail_row($label, $value, value_width)
            };
        }
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
        let vifs = topology.vifs.iter().filter(|item| item.domid == domid);
        let mut vif_count = 0;
        for vif in vifs {
            vif_count += 1;
            rows.push(detail!(
                format!("├─ VIF {}", vif.vif),
                format!(
                    "MAC={} bridge={}",
                    vif.mac.as_deref().unwrap_or("Unknown"),
                    vif.bridge.as_deref().unwrap_or("Unknown")
                ),
            ));
        }
        if vif_count == 0 {
            rows.push(Row::new(vec![
                Cell::from("├─ VIF topology"),
                Cell::from("∅ (guest topology not applicable)"),
            ]));
        }
        let devices = topology
            .block_devices
            .iter()
            .filter(|item| item.domid == domid);
        let mut device_count = 0;
        for device in devices {
            device_count += 1;
            rows.push(detail!(
                format!("├─ VBD {}", device.frontend),
                "block device",
            ));
            rows.push(detail!(
                "│  ├─ Host",
                display_value(device.host_device.as_deref()),
            ));
            rows.push(detail!(
                "│  ├─ DM",
                display_value(device.dm_name.as_deref()),
            ));
            rows.push(detail!("│  ├─ WWID", display_value(device.wwid.as_deref()),));
            rows.push(detail!(
                "│  ├─ Mapping Confidence",
                mapping_confidence_label(device.confidence),
            ));
            rows.push(detail!("│  ├─ Evidence", "see detail rows"));
            rows.push(detail!(
                "│  │  ├─ Backend",
                if device.backend.is_empty() {
                    "Unknown"
                } else {
                    &device.backend
                },
            ));
            rows.push(detail!(
                "│  │  ├─ Device",
                display_value(device.device.as_deref()),
            ));
            rows.push(detail!(
                "│  │  ├─ Virtual device",
                display_value(device.virtual_device.as_deref()),
            ));
            rows.push(detail!(
                "│  │  ├─ Physical device",
                display_value(device.physical_device.as_deref()),
            ));
            rows.push(detail!(
                "│  │  ├─ Backing file",
                display_value(device.backing_file.as_deref()),
            ));
            rows.push(detail!(
                "│  │  ├─ Major:minor",
                display_value(device.major_minor.as_deref()),
            ));
            rows.push(detail!(
                "│  │  └─ DM UUID",
                display_value(device.dm_uuid.as_deref()),
            ));
            if let Some(reason) = device.unresolved_reason.as_deref() {
                rows.push(detail!(
                    "│  └─ Unresolved",
                    format!("stage={:?} reason={reason}", device.unresolved_stage),
                ));
            }
        }
        if device_count == 0 {
            rows.push(Row::new(vec![
                Cell::from("├─ VBD topology"),
                Cell::from("∅ (guest topology not applicable)"),
            ]));
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

fn detail_row(
    label: impl Into<String>,
    value: impl Into<String>,
    value_width: usize,
) -> Row<'static> {
    let value = value.into();
    Row::new(vec![
        Cell::from(label.into()),
        Cell::from(
            wrap_detail_value(&value, value_width)
                .into_iter()
                .map(Line::from)
                .collect::<Vec<_>>(),
        ),
    ])
}

fn wrap_detail_value(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if value.is_empty() {
        return vec![String::new()];
    }
    value
        .chars()
        .collect::<Vec<_>>()
        .chunks(width)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn display_value(value: Option<&str>) -> &str {
    value.filter(|value| !value.is_empty()).unwrap_or("Unknown")
}

fn mapping_confidence_label(confidence: MappingConfidence) -> &'static str {
    match confidence {
        MappingConfidence::Exact => "Exact",
        MappingConfidence::Derived => "Derived",
        MappingConfidence::Fallback => "Fallback",
        MappingConfidence::Unknown => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vbd_detail_preserves_mapping_confidence_labels() {
        assert_eq!(mapping_confidence_label(MappingConfidence::Exact), "Exact");
        assert_eq!(
            mapping_confidence_label(MappingConfidence::Derived),
            "Derived"
        );
        assert_eq!(
            mapping_confidence_label(MappingConfidence::Fallback),
            "Fallback"
        );
        assert_eq!(
            mapping_confidence_label(MappingConfidence::Unknown),
            "Unknown"
        );
    }

    #[test]
    fn missing_or_empty_vbd_values_remain_visible() {
        assert_eq!(display_value(None), "Unknown");
        assert_eq!(display_value(Some("")), "Unknown");
        assert_eq!(
            display_value(Some("/dev/mapper/guest")),
            "/dev/mapper/guest"
        );
    }

    #[test]
    fn long_detail_values_wrap_without_loss() {
        let value = "/dev/mapper/a-very-long-backing-file-name";
        let wrapped = wrap_detail_value(value, 9);
        assert_eq!(wrapped.concat(), value);
        assert!(wrapped.len() > 1);
    }

    #[test]
    fn long_unresolved_reason_wraps_without_loss() {
        let value =
            "stage=Backend reason=backend identity was not found at the expected xenstore path";
        assert_eq!(wrap_detail_value(value, 12).concat(), value);
    }
}
