use crate::{
    app::{App, FcPanelMode, Workspace},
    topology::{
        snapshot::{MappingConfidence, SnapshotStatus, StorageHealth, StorageRedundancy},
        status::{Availability, Freshness, LayerStatus},
        view::Evidence,
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap},
};

use super::layout::{popup, screen_class};

pub(super) fn bytes(value: u64) -> String {
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

pub(super) fn block(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .title(title.into())
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
}

pub(super) fn state(value: &str) -> &'static str {
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

pub(super) fn health(value: StorageHealth) -> &'static str {
    match value {
        StorageHealth::Healthy => "HEALTHY",
        StorageHealth::Degraded => "DEGRADED",
        StorageHealth::Failed => "FAILED",
        StorageHealth::Unknown => "UNKNOWN",
    }
}

pub(super) fn status(value: SnapshotStatus) -> &'static str {
    match value {
        SnapshotStatus::Live => "LIVE",
        SnapshotStatus::Stale => "STALE",
        SnapshotStatus::MergeFallback => "FALLBACK",
        SnapshotStatus::NoData => "NO DATA",
        SnapshotStatus::MergeError => "MERGE ERROR",
    }
}

pub(super) fn redundancy(value: StorageRedundancy) -> &'static str {
    match value {
        StorageRedundancy::Single => "SINGLE",
        StorageRedundancy::Redundant => "REDUNDANT",
        StorageRedundancy::Reduced => "REDUCED",
        StorageRedundancy::None => "NONE",
        StorageRedundancy::Unknown => "UNKNOWN",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactLevel {
    Healthy,
    Degraded,
    Failed,
    Unknown,
    Empty,
    Diagnostic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompactStatus {
    text: String,
    level: CompactLevel,
}

impl CompactStatus {
    pub(super) fn style(&self) -> Style {
        let color = match self.level {
            CompactLevel::Healthy => Color::Green,
            CompactLevel::Degraded => Color::Yellow,
            CompactLevel::Failed | CompactLevel::Diagnostic => Color::Red,
            CompactLevel::Unknown | CompactLevel::Empty => Color::DarkGray,
        };
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }
}

/// Shared compact state formatter. This only translates existing UI evidence;
/// it does not classify collector or topology data.
pub(super) fn compact_status(
    freshness: Freshness,
    availability: Availability,
    health: Option<StorageHealth>,
    redundancy: Option<StorageRedundancy>,
    confidence: Option<MappingConfidence>,
    evidence: Option<Evidence>,
) -> CompactStatus {
    // Keep the compact cell to a glyph and one primary semantic code.  The
    // glyph already carries diagnostic/empty semantics, so I/E would only
    // repeat information and make the main lists noisy.
    let code = match redundancy {
        Some(StorageRedundancy::Redundant) => Some('R'),
        Some(StorageRedundancy::Single) => Some('S'),
        Some(StorageRedundancy::Reduced) => Some('M'),
        _ if matches!(confidence, Some(MappingConfidence::Unknown)) => Some('M'),
        _ if matches!(confidence, Some(MappingConfidence::Derived))
            || matches!(evidence, Some(Evidence::Derived)) =>
        {
            Some('D')
        }
        _ if freshness == Freshness::Fallback
            || matches!(confidence, Some(MappingConfidence::Fallback))
            || matches!(evidence, Some(Evidence::Fallback)) =>
        {
            Some('F')
        }
        _ => None,
    };

    let level = if matches!(
        availability,
        Availability::Unavailable | Availability::Error | Availability::CollectorIncompatible
    ) || freshness == Freshness::NoData
        || matches!(evidence, Some(Evidence::Unavailable))
    {
        CompactLevel::Diagnostic
    } else if availability == Availability::Empty {
        CompactLevel::Empty
    } else if matches!(health, Some(StorageHealth::Failed)) {
        CompactLevel::Failed
    } else if availability == Availability::Partial
        || matches!(health, Some(StorageHealth::Degraded))
        || matches!(redundancy, Some(StorageRedundancy::Reduced))
        || matches!(confidence, Some(MappingConfidence::Unknown))
    {
        CompactLevel::Degraded
    } else if matches!(health, Some(StorageHealth::Unknown)) {
        CompactLevel::Unknown
    } else {
        CompactLevel::Healthy
    };
    let glyph = match level {
        CompactLevel::Healthy => '●',
        CompactLevel::Degraded => '▲',
        CompactLevel::Failed => '×',
        CompactLevel::Unknown => '?',
        CompactLevel::Empty => '∅',
        CompactLevel::Diagnostic => '!',
    };
    CompactStatus {
        text: if let Some(code) = code {
            format!("{glyph} {code}")
        } else {
            glyph.to_string()
        },
        level,
    }
}

pub(super) fn compact_layer_status(value: &LayerStatus) -> CompactStatus {
    compact_status(value.freshness, value.availability, None, None, None, None)
}

pub(super) fn compact_storage_status(
    health: StorageHealth,
    redundancy: StorageRedundancy,
) -> CompactStatus {
    compact_status(
        Freshness::Live,
        Availability::Available,
        Some(health),
        Some(redundancy),
        None,
        None,
    )
}

pub(super) fn freshness(value: Freshness) -> &'static str {
    match value {
        Freshness::Live => "LIVE",
        Freshness::Stale => "STALE",
        Freshness::Fallback => "FALLBACK",
        Freshness::NoData => "NO DATA",
    }
}

pub(super) fn availability(value: Availability) -> &'static str {
    match value {
        Availability::Available => "AVAILABLE",
        Availability::Empty => "EMPTY",
        Availability::Unavailable => "UNAVAILABLE",
        Availability::Partial => "PARTIAL",
        Availability::Error => "ERROR",
        Availability::CollectorIncompatible => "COLLECTOR INCOMPATIBLE",
    }
}

pub(super) fn layer_status(value: &LayerStatus) -> String {
    compact_layer_status(value).text().to_string()
}

pub(super) fn empty(title: impl Into<String>, message: impl Into<String>) -> Paragraph<'static> {
    Paragraph::new(message.into())
        .style(Style::default().fg(Color::DarkGray))
        .wrap(Wrap { trim: false })
        .block(block(title))
}

pub(super) fn error(title: impl Into<String>, message: impl Into<String>) -> Paragraph<'static> {
    Paragraph::new(message.into())
        .style(Style::default().fg(Color::Red))
        .wrap(Wrap { trim: false })
        .block(block(title))
}

pub(super) fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let current = match app.workspace {
        Workspace::Overview => "Overview",
        Workspace::Domains => "Domains",
        Workspace::Network => "Network",
        Workspace::Disk => "Disk",
        Workspace::Logs => "Logs",
        Workspace::FcSan => "FC/SAN",
    };
    let size = screen_class(frame.area());
    frame.render_widget(
        Paragraph::new(format!(
            "ovm-top | {current} | {size}   0 Overview  1 Domains  2 Network  3 Disk  4 Logs  5 FC/SAN"
        ))
        .block(block("Workspace")),
        area,
    );
}

pub(super) fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints = match app.workspace {
        Workspace::Overview => "0-5: workspace  ?: help  q: quit",
        Workspace::Domains => "j/k: navigate  l/Enter: detail  h/Esc: back  ?: help",
        Workspace::Network => "j/k: navigate  v: performance/topology  l: detail  h: back",
        Workspace::Disk => "j/k: navigate  v: performance/topology  l: detail  h: back",
        Workspace::Logs => {
            "j/k: scroll  /: search  f: level  F: source  t: time  c: clear filters  4: back"
        }
        Workspace::FcSan if app.fc_panel_mode == FcPanelMode::Detail => {
            "u/d: preview  h/Esc: back  o: views  4: logs"
        }
        Workspace::FcSan => "j/k: select  l: detail  h/Esc: back  o: views  4: logs",
    };
    frame.render_widget(
        Paragraph::new(hints).block(block(format!("Mode: {:?}", app.input_mode))),
        area,
    );
}

pub(super) fn draw_help(frame: &mut Frame) {
    let area = popup(frame, 72, 22);
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new("GLOBAL\n  0 Overview | 1 Domains | 2 Network | 3 Disk | 4 Logs | 5 FC/SAN\n  q quit | Esc close help; otherwise back (or quit in Overview) | ? close help\n\nNAVIGATION\n  j/k or arrows navigate | l/Enter detail | h/Esc back\n  u/d preview FC map | o FC/SAN subviews | v Network/Disk subview\n\nLOGS (memory + persistent history)\n  / text search | f level | F source | t time range | c clear filters\n\nCOMPACT STATUS\n  ● live/available  ▲ partial/degraded  × failed  ∅ empty\n  ? unknown  ! unavailable/error\n  R redundant  S single  D derived  M reduced/uncertain mapping\n  F fallback  I collector incompatible  E empty\n\nDETAIL STATUS\n  Detail views retain LIVE / AVAILABLE / Exact (or Derived/Fallback)\n  Freshness: LIVE / STALE / FALLBACK / NO DATA\n  Availability: EMPTY / UNAVAILABLE / PARTIAL / ERROR / COLLECTOR INCOMPATIBLE").wrap(Wrap { trim: false }).block(block("Context Help")), area);
}

pub(super) fn draw_fc_menu(frame: &mut Frame, app: &App) {
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
    fn compact_storage_state_keeps_health_and_redundancy_distinct() {
        assert_eq!(
            compact_storage_status(StorageHealth::Healthy, StorageRedundancy::Redundant).text(),
            "● R"
        );
        assert_eq!(
            compact_storage_status(StorageHealth::Healthy, StorageRedundancy::Single).text(),
            "● S"
        );
        assert_eq!(
            compact_storage_status(StorageHealth::Degraded, StorageRedundancy::Reduced).text(),
            "▲ M"
        );
        assert_eq!(
            compact_storage_status(StorageHealth::Failed, StorageRedundancy::Single).text(),
            "× S"
        );
    }

    #[test]
    fn compact_mapping_flags_never_claim_exact_evidence() {
        assert_eq!(
            compact_status(
                Freshness::Live,
                Availability::Available,
                Some(StorageHealth::Healthy),
                None,
                Some(MappingConfidence::Derived),
                None
            )
            .text(),
            "● D"
        );
        assert_eq!(
            compact_status(
                Freshness::Fallback,
                Availability::Available,
                Some(StorageHealth::Healthy),
                None,
                Some(MappingConfidence::Fallback),
                None
            )
            .text(),
            "● F"
        );
        assert_eq!(
            compact_status(
                Freshness::Live,
                Availability::Partial,
                None,
                None,
                Some(MappingConfidence::Unknown),
                None
            )
            .text(),
            "▲ M"
        );
    }

    #[test]
    fn compact_diagnostics_and_empty_are_not_health_failures() {
        assert_eq!(
            compact_status(
                Freshness::NoData,
                Availability::Unavailable,
                None,
                None,
                None,
                None
            )
            .text(),
            "!"
        );
        assert_eq!(
            compact_status(
                Freshness::Live,
                Availability::CollectorIncompatible,
                None,
                None,
                None,
                None
            )
            .text(),
            "!"
        );
        assert_eq!(
            compact_status(Freshness::Live, Availability::Empty, None, None, None, None).text(),
            "∅"
        );
        assert_eq!(
            compact_status(
                Freshness::Live,
                Availability::Available,
                Some(StorageHealth::Unknown),
                None,
                None,
                None,
            )
            .text(),
            "?"
        );
    }
}
