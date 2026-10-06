use std::{
    collections::VecDeque,
    sync::mpsc::Receiver,
    sync::{Arc, Mutex},
};

use crate::{
    collector::{disk::DiskRate, fc::FcSnapshot, net::NetRate, xentop::DomainView, xm::XmInfo},
    log::{LogEntry, LogEvent},
};

const MAX_LOG_ENTRIES: usize = 1_000;

#[derive(Debug, Clone, Copy, Default)]
pub enum LogFilter {
    #[default]
    All,
    WarningsAndErrors,
    ErrorsOnly,
}

impl LogFilter {
    pub fn matches(self, level: crate::log::LogLevel) -> bool {
        match self {
            Self::All => true,
            Self::WarningsAndErrors => !matches!(level, crate::log::LogLevel::Info),
            Self::ErrorsOnly => matches!(level, crate::log::LogLevel::Error),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Domains,
    Network,
    Disk,
    Fc,
    Logs,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FcView {
    Ports,
    Targets,
    #[default]
    Multipath,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FcPanelMode {
    #[default]
    Summary,
    Detail,
}

#[derive(Debug, Clone, Copy)]
pub struct FcDisplayConfig {
    pub view: FcView,
    pub show_path_detail: bool,
    pub merge_disk_io: bool,
}

impl Default for FcDisplayConfig {
    fn default() -> Self {
        Self {
            view: FcView::Multipath,
            show_path_detail: false,
            merge_disk_io: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollState {
    pub offset: usize,
    pub total: usize,
    pub viewport: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum ScrollResult {
    Moved { from: usize, to: usize },
    AtTop { repeated: bool },
    AtBottom { repeated: bool },
}

#[derive(Debug, Default)]
pub struct App {
    pub xm_info: XmInfo,
    pub domains: Arc<Mutex<Vec<DomainView>>>,
    pub network: Arc<Mutex<Vec<NetRate>>>,
    pub disk: Arc<Mutex<Vec<DiskRate>>>,
    pub fc: Arc<Mutex<FcSnapshot>>,
    pub focus: Focus,
    pub domain_scroll: ScrollState,
    pub network_scroll: ScrollState,
    pub disk_scroll: ScrollState,
    pub fc_scroll: ScrollState,
    pub fc_selected: Option<usize>,
    pub fc_detail_scroll: ScrollState,
    pub fc_panel_mode: FcPanelMode,
    pub fc_detail_map: Option<usize>,
    pub fc_config: FcDisplayConfig,
    pub fc_config_open: bool,
    pub fc_config_index: usize,
    pub logs: VecDeque<LogEntry>,
    pub log_scroll: ScrollState,
    pub log_filter: LogFilter,
    pub show_logs: bool,
    pub(crate) last_scroll_boundary: Option<(Focus, bool)>,
}

impl App {
    pub fn scroll_down(&mut self) -> ScrollResult {
        let focus = self.focus;
        let (offset, max_offset) = {
            let state = self.scroll_state_mut();
            (state.offset, state.total.saturating_sub(state.viewport))
        };
        if offset >= max_offset {
            let repeated = self.last_scroll_boundary == Some((focus, false));
            self.last_scroll_boundary = Some((focus, false));
            return ScrollResult::AtBottom { repeated };
        }

        let from = offset;
        let to = {
            let state = self.scroll_state_mut();
            state.offset += 1;
            state.offset
        };
        self.last_scroll_boundary = None;
        ScrollResult::Moved { from, to }
    }

    pub fn scroll_up(&mut self) -> ScrollResult {
        let focus = self.focus;
        let offset = self.scroll_state_mut().offset;
        if offset == 0 {
            let repeated = self.last_scroll_boundary == Some((focus, true));
            self.last_scroll_boundary = Some((focus, true));
            return ScrollResult::AtTop { repeated };
        }

        let from = offset;
        let to = {
            let state = self.scroll_state_mut();
            state.offset -= 1;
            state.offset
        };
        self.last_scroll_boundary = None;
        ScrollResult::Moved { from, to }
    }

    pub fn set_scroll_metrics(&mut self, focus: Focus, total: usize, viewport: usize) {
        let state = match focus {
            Focus::Domains => &mut self.domain_scroll,
            Focus::Network => &mut self.network_scroll,
            Focus::Disk => &mut self.disk_scroll,
            Focus::Fc if self.fc_panel_mode == FcPanelMode::Detail => &mut self.fc_detail_scroll,
            Focus::Fc => &mut self.fc_scroll,
            Focus::Logs => &mut self.log_scroll,
        };
        state.total = total;
        state.viewport = viewport;
        state.offset = state.offset.min(total.saturating_sub(viewport));
    }

    pub fn current_scroll_state(&self) -> ScrollState {
        match self.focus {
            Focus::Domains => self.domain_scroll,
            Focus::Network => self.network_scroll,
            Focus::Disk => self.disk_scroll,
            Focus::Fc if self.fc_panel_mode == FcPanelMode::Detail => self.fc_detail_scroll,
            Focus::Fc => self.fc_scroll,
            Focus::Logs => self.log_scroll,
        }
    }

    fn scroll_state_mut(&mut self) -> &mut ScrollState {
        match self.focus {
            Focus::Domains => &mut self.domain_scroll,
            Focus::Network => &mut self.network_scroll,
            Focus::Disk => &mut self.disk_scroll,
            Focus::Fc if self.fc_panel_mode == FcPanelMode::Detail => &mut self.fc_detail_scroll,
            Focus::Fc => &mut self.fc_scroll,
            Focus::Logs => &mut self.log_scroll,
        }
    }

    pub fn focus_logs(&mut self) {
        self.show_logs = true;
        self.focus = Focus::Logs;
    }

    pub fn focus_fc(&mut self) {
        self.focus = Focus::Fc;
    }

    pub fn open_fc_detail(&mut self) -> bool {
        let map_count = self
            .fc
            .lock()
            .map(|snapshot| snapshot.maps.len())
            .unwrap_or(0);
        if map_count == 0 {
            return false;
        }
        let selected = self.fc_selected.unwrap_or(0).min(map_count - 1);
        self.fc_detail_map = Some(selected);
        self.ensure_fc_map_visible(selected);
        self.fc_selected = None;
        self.fc_detail_scroll = ScrollState::default();
        self.fc_panel_mode = FcPanelMode::Detail;
        true
    }

    pub fn close_fc_detail(&mut self) {
        let selected = self.fc_detail_map;
        self.fc_panel_mode = FcPanelMode::Summary;
        self.fc_detail_map = None;
        self.fc_selected = selected.or(Some(self.fc_scroll.offset));
        self.fc_detail_scroll = ScrollState::default();
    }

    pub fn preview_fc_map(&mut self, delta: isize) -> bool {
        let map_count = self
            .fc
            .lock()
            .map(|snapshot| snapshot.maps.len())
            .unwrap_or(0);
        let Some(current) = self.fc_detail_map else {
            return false;
        };
        if map_count == 0 {
            self.close_fc_detail();
            return false;
        }

        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize)
        };
        if next >= map_count || next == current {
            return false;
        }

        self.fc_detail_map = Some(next);
        self.ensure_fc_map_visible(next);
        self.fc_detail_scroll = ScrollState::default();
        true
    }

    fn ensure_fc_map_visible(&mut self, index: usize) {
        let viewport = self.fc_scroll.viewport.max(1);
        if index < self.fc_scroll.offset {
            self.fc_scroll.offset = index;
        } else if index >= self.fc_scroll.offset + viewport {
            self.fc_scroll.offset = index + 1 - viewport;
        }
        self.fc_scroll.offset = self
            .fc_scroll
            .offset
            .min(self.fc_scroll.total.saturating_sub(viewport));
    }

    pub fn select_fc_down(&mut self) {
        let count = self
            .fc
            .lock()
            .map(|snapshot| snapshot.maps.len())
            .unwrap_or(0);
        if count == 0 {
            self.fc_selected = None;
            return;
        }
        let selected = self
            .fc_selected
            .unwrap_or(0)
            .saturating_add(1)
            .min(count - 1);
        self.fc_selected = Some(selected);
        self.ensure_fc_selection_visible();
    }

    pub fn select_fc_up(&mut self) {
        let count = self
            .fc
            .lock()
            .map(|snapshot| snapshot.maps.len())
            .unwrap_or(0);
        if count == 0 {
            self.fc_selected = None;
            return;
        }
        let selected = self.fc_selected.unwrap_or(0).saturating_sub(1);
        self.fc_selected = Some(selected);
        self.ensure_fc_selection_visible();
    }

    fn ensure_fc_selection_visible(&mut self) {
        let Some(selected) = self.fc_selected else {
            return;
        };
        self.ensure_fc_map_visible(selected);
    }

    pub fn open_fc_config(&mut self) {
        self.fc_config_open = true;
        self.fc_config_index = match self.fc_config.view {
            FcView::Ports => 0,
            FcView::Targets => 1,
            FcView::Multipath => 2,
        };
    }

    pub fn move_fc_config(&mut self, down: bool) {
        if down {
            self.fc_config_index = (self.fc_config_index + 1).min(3);
        } else {
            self.fc_config_index = self.fc_config_index.saturating_sub(1);
        }
    }

    pub fn apply_fc_config(&mut self) {
        if self.fc_config_index <= 2 {
            self.fc_config.view = match self.fc_config_index {
                0 => FcView::Ports,
                1 => FcView::Targets,
                _ => FcView::Multipath,
            };
            self.fc_scroll.offset = 0;
            self.fc_selected = None;
            self.close_fc_detail();
        } else {
            self.fc_config.show_path_detail = !self.fc_config.show_path_detail;
            if self.fc_config.show_path_detail {
                self.open_fc_detail();
            } else {
                self.close_fc_detail();
            }
        }
        self.fc_config_open = false;
    }

    pub fn hide_logs(&mut self) {
        self.show_logs = false;
        if self.focus == Focus::Logs {
            self.focus = Focus::Domains;
        }
    }

    pub fn drain_logs(&mut self, receiver: &Receiver<LogEvent>) {
        while let Ok(event) = receiver.try_recv() {
            let was_at_log_bottom = self.focus == Focus::Logs
                && self.show_logs
                && self.log_scroll.offset
                    >= self
                        .log_scroll
                        .total
                        .saturating_sub(self.log_scroll.viewport);

            self.logs.push_back(event.into());
            if self.logs.len() > MAX_LOG_ENTRIES {
                self.logs.pop_front();
            }

            self.log_scroll.total = self
                .logs
                .iter()
                .filter(|entry| self.log_filter.matches(entry.level))
                .count();
            if was_at_log_bottom {
                self.log_scroll.offset = self
                    .log_scroll
                    .total
                    .saturating_sub(self.log_scroll.viewport);
            } else {
                self.log_scroll.offset = self.log_scroll.offset.min(
                    self.log_scroll
                        .total
                        .saturating_sub(self.log_scroll.viewport),
                );
            }
        }
    }

    pub fn cycle_log_filter(&mut self) {
        self.log_filter = match self.log_filter {
            LogFilter::All => LogFilter::WarningsAndErrors,
            LogFilter::WarningsAndErrors => LogFilter::ErrorsOnly,
            LogFilter::ErrorsOnly => LogFilter::All,
        };
        self.log_scroll.offset = 0;
        self.last_scroll_boundary = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{LogEvent, LogSource};
    use std::sync::mpsc;

    fn app_at_log_position(offset: usize) -> App {
        let mut app = App::default();
        app.focus = Focus::Logs;
        app.show_logs = true;
        app.log_scroll = ScrollState {
            offset,
            total: 20,
            viewport: 5,
        };
        for _ in 0..20 {
            app.logs
                .push_back(LogEvent::info(LogSource::Ui, "existing").into());
        }
        app
    }

    #[test]
    fn appends_logs_while_following_bottom() {
        let mut app = app_at_log_position(15);
        let (tx, rx) = mpsc::channel();
        tx.send(LogEvent::info(LogSource::Ui, "new")).unwrap();

        app.drain_logs(&rx);

        assert_eq!(app.log_scroll.total, 21);
        assert_eq!(app.log_scroll.offset, 16);
    }

    #[test]
    fn appends_logs_without_jumping_when_reading_history() {
        let mut app = app_at_log_position(10);
        let (tx, rx) = mpsc::channel();
        tx.send(LogEvent::info(LogSource::Ui, "new")).unwrap();

        app.drain_logs(&rx);

        assert_eq!(app.log_scroll.total, 21);
        assert_eq!(app.log_scroll.offset, 10);
    }

    fn app_with_fc_maps(count: usize) -> App {
        let mut app = App::default();
        app.fc = Arc::new(Mutex::new(FcSnapshot {
            maps: (0..count)
                .map(|index| crate::collector::fc::MultipathMap {
                    wwid: format!("wwid-{index}"),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }));
        app.fc_scroll = ScrollState {
            offset: 0,
            total: count,
            viewport: 2,
        };
        app
    }

    #[test]
    fn fc_selection_moves_and_keeps_selected_row_visible() {
        let mut app = app_with_fc_maps(5);

        app.select_fc_down();
        assert_eq!(app.fc_selected, Some(1));
        assert_eq!(app.fc_scroll.offset, 0);

        app.select_fc_down();
        assert_eq!(app.fc_selected, Some(2));
        assert_eq!(app.fc_scroll.offset, 1);

        app.select_fc_up();
        assert_eq!(app.fc_selected, Some(1));
        assert_eq!(app.fc_scroll.offset, 1);
    }

    #[test]
    fn opening_fc_detail_clears_parent_selection() {
        let mut app = app_with_fc_maps(2);
        app.fc_selected = Some(1);

        assert!(app.open_fc_detail());
        assert_eq!(app.fc_detail_map, Some(1));
        assert_eq!(app.fc_selected, None);
        assert_eq!(app.fc_panel_mode, FcPanelMode::Detail);

        app.close_fc_detail();
        assert_eq!(app.fc_panel_mode, FcPanelMode::Summary);
        assert_eq!(app.fc_selected, Some(1));
    }

    #[test]
    fn fc_detail_preview_moves_and_resets_scroll() {
        let mut app = app_with_fc_maps(3);
        app.fc_selected = Some(1);
        assert!(app.open_fc_detail());
        app.fc_detail_scroll.offset = 4;

        assert!(app.preview_fc_map(-1));
        assert_eq!(app.fc_detail_map, Some(0));
        assert_eq!(app.fc_detail_scroll.offset, 0);

        assert!(!app.preview_fc_map(-1));
        assert!(app.preview_fc_map(1));
        assert_eq!(app.fc_detail_map, Some(1));
    }
}
