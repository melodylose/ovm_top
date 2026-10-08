use super::{
    layout::{full_screen, range, viewport},
    widgets::block,
};
use crate::{
    app::{App, InputMode, LogTimeRange},
    log,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::{Paragraph, Row, Table},
};

pub(super) fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let area = full_screen(area);
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
        .block(block("Log Filters")),
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
