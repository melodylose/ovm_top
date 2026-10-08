use std::{
    collections::VecDeque,
    sync::mpsc::Receiver,
    sync::{Arc, Mutex},
};

use crate::{
    collector::{
        disk::DiskRate,
        fc::FcSnapshot,
        net::NetRate,
        xentop::{DomainSnapshotMeta, DomainView},
        xm::XmInfo,
    },
    log::{LogEntry, LogEvent, LogLevel, LogSource, PersistentLogger},
    topology::snapshot::TopologySnapshot,
};

pub(crate) const MAX_LOG_ENTRIES: usize = 1_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogTimeRange {
    #[default]
    All,
    LastMinute,
    LastFiveMinutes,
    LastFifteenMinutes,
    LastHour,
}

impl LogTimeRange {
    pub fn seconds(self) -> Option<u64> {
        match self {
            Self::All => None,
            Self::LastMinute => Some(60),
            Self::LastFiveMinutes => Some(300),
            Self::LastFifteenMinutes => Some(900),
            Self::LastHour => Some(3600),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    pub min_level: Option<LogLevel>,
    pub source: Option<LogSource>,
    pub text: String,
    pub time_range: LogTimeRange,
}

impl LogFilter {
    pub fn matches(&self, entry: &LogEntry) -> bool {
        let level_matches = match self.min_level {
            None => true,
            Some(LogLevel::Info) => true,
            Some(LogLevel::Warn) => entry.level != LogLevel::Info,
            Some(LogLevel::Error) => entry.level == LogLevel::Error,
        };
        let source_matches = self.source.is_none_or(|source| source == entry.source);
        let text_matches = self.text.is_empty()
            || entry
                .message
                .to_ascii_lowercase()
                .contains(&self.text.to_ascii_lowercase());
        let time_matches = self.time_range.seconds().is_none_or(|seconds| {
            std::time::SystemTime::now()
                .duration_since(entry.timestamp)
                .map(|age| age.as_secs() <= seconds)
                .unwrap_or(true)
        });
        level_matches && source_matches && text_matches && time_matches
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Workspace {
    #[default]
    Overview,
    Domains,
    Network,
    Disk,
    Logs,
    FcSan,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InputMode {
    #[default]
    Normal,
    Detail,
    ViewMenu,
    Filter,
    Search,
    TimeRange,
    Help,
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
    Overview,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DetailTarget {
    #[default]
    None,
    Domain(u32),
    Network(String),
    Disk(String),
    Multipath(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetworkView {
    #[default]
    Performance,
    Topology,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiskView {
    #[default]
    Performance,
    Topology,
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
    pub domain_meta: Arc<Mutex<DomainSnapshotMeta>>,
    pub topology: Arc<Mutex<TopologySnapshot>>,
    pub network: Arc<Mutex<Vec<NetRate>>>,
    pub disk: Arc<Mutex<Vec<DiskRate>>>,
    pub fc: Arc<Mutex<FcSnapshot>>,
    pub focus: Focus,
    pub workspace: Workspace,
    pub previous_workspace: Workspace,
    pub input_mode: InputMode,
    pub domain_scroll: ScrollState,
    pub domain_selected: Option<usize>,
    pub domain_detail_scroll: ScrollState,
    pub domain_detail_open: bool,
    pub domain_detail_index: Option<usize>,
    pub network_scroll: ScrollState,
    pub network_selected: Option<usize>,
    pub network_detail_scroll: ScrollState,
    pub network_detail_open: bool,
    pub network_view: NetworkView,
    pub disk_scroll: ScrollState,
    pub disk_selected: Option<usize>,
    pub disk_detail_scroll: ScrollState,
    pub disk_view: DiskView,
    pub fc_scroll: ScrollState,
    pub fc_selected: Option<usize>,
    pub fc_detail_scroll: ScrollState,
    pub fc_panel_mode: FcPanelMode,
    pub fc_detail_map: Option<usize>,
    pub detail_target: DetailTarget,
    pub fc_config: FcDisplayConfig,
    pub fc_config_open: bool,
    pub fc_config_index: usize,
    pub logs: VecDeque<LogEntry>,
    pub persistent_logger: Option<PersistentLogger>,
    pub log_scroll: ScrollState,
    pub log_filter: LogFilter,
    pub search_input: String,
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
            Focus::Domains if self.domain_detail_open => &mut self.domain_detail_scroll,
            Focus::Domains => &mut self.domain_scroll,
            Focus::Network if self.network_detail_open => &mut self.network_detail_scroll,
            Focus::Network => &mut self.network_scroll,
            Focus::Disk if matches!(self.detail_target, DetailTarget::Disk(_)) => {
                &mut self.disk_detail_scroll
            }
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
            Focus::Domains if self.domain_detail_open => self.domain_detail_scroll,
            Focus::Domains => self.domain_scroll,
            Focus::Network if self.network_detail_open => self.network_detail_scroll,
            Focus::Network => self.network_scroll,
            Focus::Disk if matches!(self.detail_target, DetailTarget::Disk(_)) => {
                self.disk_detail_scroll
            }
            Focus::Disk => self.disk_scroll,
            Focus::Fc if self.fc_panel_mode == FcPanelMode::Detail => self.fc_detail_scroll,
            Focus::Fc => self.fc_scroll,
            Focus::Logs => self.log_scroll,
        }
    }

    fn scroll_state_mut(&mut self) -> &mut ScrollState {
        match self.focus {
            Focus::Domains if self.domain_detail_open => &mut self.domain_detail_scroll,
            Focus::Domains => &mut self.domain_scroll,
            Focus::Network if self.network_detail_open => &mut self.network_detail_scroll,
            Focus::Network => &mut self.network_scroll,
            Focus::Disk if matches!(self.detail_target, DetailTarget::Disk(_)) => {
                &mut self.disk_detail_scroll
            }
            Focus::Disk => &mut self.disk_scroll,
            Focus::Fc if self.fc_panel_mode == FcPanelMode::Detail => &mut self.fc_detail_scroll,
            Focus::Fc => &mut self.fc_scroll,
            Focus::Logs => &mut self.log_scroll,
        }
    }

    pub fn focus_logs(&mut self) {
        self.show_logs = true;
        self.set_workspace(Workspace::Logs);
    }

    pub fn set_workspace(&mut self, workspace: Workspace) {
        if self.workspace != workspace {
            self.previous_workspace = self.workspace;
            self.domain_detail_open = false;
            self.network_detail_open = false;
            self.fc_panel_mode = FcPanelMode::Summary;
            self.fc_detail_map = None;
            self.detail_target = DetailTarget::None;
        }
        self.workspace = workspace;
        self.focus = match workspace {
            Workspace::Overview | Workspace::Domains => Focus::Domains,
            Workspace::Network => Focus::Network,
            Workspace::Disk => Focus::Disk,
            Workspace::Logs => Focus::Logs,
            Workspace::FcSan => Focus::Fc,
        };
        self.input_mode = InputMode::Normal;
        self.last_scroll_boundary = None;
    }

    pub fn open_domain_detail(&mut self) -> bool {
        let count = self
            .domains
            .lock()
            .map(|domains| domains.len())
            .unwrap_or(0);
        if count == 0 {
            return false;
        }
        self.close_network_detail();
        if self.fc_panel_mode == FcPanelMode::Detail {
            self.close_fc_detail();
        }
        let index = self
            .domain_selected
            .unwrap_or(self.domain_scroll.offset)
            .min(count - 1);
        self.domain_detail_index = Some(index);
        self.domain_detail_scroll = ScrollState::default();
        self.domain_detail_open = true;
        let domid = self
            .domains
            .lock()
            .ok()
            .and_then(|domains| domains.get(index).map(|domain| domain.id))
            .unwrap_or_default();
        self.detail_target = DetailTarget::Domain(domid);
        self.input_mode = InputMode::Detail;
        true
    }

    pub fn open_network_detail(&mut self) -> bool {
        let available = self
            .topology
            .lock()
            .map(|snapshot| !snapshot.interfaces.is_empty())
            .unwrap_or(false);
        if !available {
            return false;
        }
        self.close_domain_detail();
        if self.fc_panel_mode == FcPanelMode::Detail {
            self.close_fc_detail();
        }
        self.network_detail_scroll = ScrollState::default();
        self.network_detail_open = true;
        let name =
            self.network
                .lock()
                .ok()
                .and_then(|network| {
                    network
                        .get(self.network_selected.unwrap_or(self.network_scroll.offset))
                        .map(|item| item.name.clone())
                })
                .or_else(|| {
                    self.topology.lock().ok().and_then(|snapshot| {
                        snapshot.interfaces.first().map(|item| item.name.clone())
                    })
                })
                .unwrap_or_else(|| "Unknown".to_string());
        self.detail_target = DetailTarget::Network(name);
        self.input_mode = InputMode::Detail;
        true
    }

    pub fn close_network_detail(&mut self) {
        self.network_detail_open = false;
        self.network_detail_scroll = ScrollState::default();
        if matches!(self.detail_target, DetailTarget::Network(_)) {
            self.detail_target = DetailTarget::None;
            self.input_mode = InputMode::Normal;
        }
    }

    pub fn close_domain_detail(&mut self) {
        self.domain_detail_open = false;
        self.domain_detail_index = None;
        self.domain_detail_scroll = ScrollState::default();
        if matches!(self.detail_target, DetailTarget::Domain(_)) {
            self.detail_target = DetailTarget::None;
            self.input_mode = InputMode::Normal;
        }
    }

    pub fn open_disk_detail(&mut self) -> bool {
        let name = self.disk.lock().ok().and_then(|disks| {
            disks
                .get(self.disk_selected.unwrap_or(self.disk_scroll.offset))
                .map(|disk| disk.name.clone())
        });
        let Some(name) = name else { return false };
        self.close_domain_detail();
        self.close_network_detail();
        if self.fc_panel_mode == FcPanelMode::Detail {
            self.close_fc_detail();
        }
        self.disk_detail_scroll = ScrollState::default();
        self.detail_target = DetailTarget::Disk(name);
        self.input_mode = InputMode::Detail;
        true
    }

    pub fn close_disk_detail(&mut self) {
        if matches!(self.detail_target, DetailTarget::Disk(_)) {
            self.detail_target = DetailTarget::None;
            self.disk_detail_scroll = ScrollState::default();
            self.input_mode = InputMode::Normal;
        }
    }

    pub fn focus_fc(&mut self) {
        self.set_workspace(Workspace::FcSan);
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
        self.close_domain_detail();
        self.close_network_detail();
        let selected = self.fc_selected.unwrap_or(0).min(map_count - 1);
        self.fc_detail_map = Some(selected);
        self.ensure_fc_map_visible(selected);
        self.fc_selected = None;
        self.fc_detail_scroll = ScrollState::default();
        self.fc_panel_mode = FcPanelMode::Detail;
        let wwid = self
            .fc
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.maps.get(selected).map(|map| map.wwid.clone()))
            .unwrap_or_else(|| "Unknown".to_string());
        self.detail_target = DetailTarget::Multipath(wwid);
        self.input_mode = InputMode::Detail;
        true
    }

    pub fn close_fc_detail(&mut self) {
        let selected = self.fc_detail_map;
        self.fc_panel_mode = FcPanelMode::Summary;
        self.fc_detail_map = None;
        self.fc_selected = selected.or(Some(self.fc_scroll.offset));
        self.fc_detail_scroll = ScrollState::default();
        if matches!(self.detail_target, DetailTarget::Multipath(_)) {
            self.detail_target = DetailTarget::None;
            self.input_mode = InputMode::Normal;
        }
    }

    /// Close detail when a refresh removed the map it was showing.
    pub fn reconcile_fc_detail(&mut self) {
        if self.fc_panel_mode != FcPanelMode::Detail {
            return;
        }
        let Some(wwid) = (match &self.detail_target {
            DetailTarget::Multipath(wwid) => Some(wwid.clone()),
            _ => None,
        }) else {
            self.close_fc_detail();
            return;
        };

        let index = self
            .fc
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.maps.iter().position(|map| map.wwid == wwid));
        let Some(index) = index else {
            self.close_fc_detail();
            if let Ok(snapshot) = self.fc.lock() {
                self.fc_selected = if snapshot.maps.is_empty() {
                    None
                } else {
                    self.fc_selected
                        .map(|selected| selected.min(snapshot.maps.len() - 1))
                };
            }
            return;
        };

        // The index is only a viewport/selection cache; WWID is the identity.
        self.fc_detail_map = Some(index);
        self.ensure_fc_map_visible(index);
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
        if let Ok(snapshot) = self.fc.lock()
            && let Some(map) = snapshot.maps.get(next)
        {
            self.detail_target = DetailTarget::Multipath(map.wwid.clone());
        }
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
            FcView::Overview => 0,
            FcView::Ports => 1,
            FcView::Targets => 2,
            FcView::Multipath => 3,
        };
        self.input_mode = InputMode::ViewMenu;
    }

    pub fn move_fc_config(&mut self, down: bool) {
        if down {
            self.fc_config_index = (self.fc_config_index + 1).min(4);
        } else {
            self.fc_config_index = self.fc_config_index.saturating_sub(1);
        }
    }

    pub fn apply_fc_config(&mut self) {
        if self.fc_config_index <= 3 {
            self.fc_config.view = match self.fc_config_index {
                0 => FcView::Overview,
                1 => FcView::Ports,
                2 => FcView::Targets,
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
        self.input_mode = InputMode::Normal;
    }

    pub fn hide_logs(&mut self) {
        self.show_logs = false;
        if self.workspace == Workspace::Logs {
            let target = if self.previous_workspace == Workspace::Logs {
                Workspace::Overview
            } else {
                self.previous_workspace
            };
            self.set_workspace(target);
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

            let entry = LogEntry::from(event);
            if let Some(logger) = self.persistent_logger.as_mut()
                && let Err(error) = logger.write(&entry)
            {
                eprintln!(
                    "ovm-top warning: persistent logging disabled after write failure: {error}"
                );
                self.persistent_logger = None;
            }
            self.logs.push_back(entry);
            if self.logs.len() > MAX_LOG_ENTRIES {
                self.logs.pop_front();
            }

            self.log_scroll.total = self
                .logs
                .iter()
                .filter(|entry| self.log_filter.matches(entry))
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
        self.log_filter.min_level = match self.log_filter.min_level {
            None => Some(LogLevel::Warn),
            Some(LogLevel::Warn) => Some(LogLevel::Error),
            _ => None,
        };
        self.log_scroll.offset = 0;
        self.last_scroll_boundary = None;
    }

    pub fn select_workspace_row(&mut self, down: bool) -> bool {
        let (selected, scroll, count) = match self.focus {
            Focus::Domains if !self.domain_detail_open => (
                &mut self.domain_selected,
                &mut self.domain_scroll,
                self.domains
                    .lock()
                    .map(|items| items.len())
                    .unwrap_or_default(),
            ),
            Focus::Network if !self.network_detail_open => (
                &mut self.network_selected,
                &mut self.network_scroll,
                self.network
                    .lock()
                    .map(|items| items.len())
                    .unwrap_or_default(),
            ),
            Focus::Disk if !matches!(self.detail_target, DetailTarget::Disk(_)) => (
                &mut self.disk_selected,
                &mut self.disk_scroll,
                self.disk
                    .lock()
                    .map(|items| items.len())
                    .unwrap_or_default(),
            ),
            _ => return false,
        };
        if count == 0 {
            *selected = None;
            return true;
        }
        let current = selected.unwrap_or(scroll.offset).min(count - 1);
        let next = if down {
            current.saturating_add(1).min(count - 1)
        } else {
            current.saturating_sub(1)
        };
        *selected = Some(next);
        let visible = scroll.viewport.max(1);
        if next < scroll.offset {
            scroll.offset = next;
        } else if next >= scroll.offset.saturating_add(visible) {
            scroll.offset = next + 1 - visible;
        }
        true
    }

    pub fn cycle_log_source(&mut self) {
        self.log_filter.source = match self.log_filter.source {
            None => Some(LogSource::System),
            Some(LogSource::System) => Some(LogSource::Domain),
            Some(LogSource::Domain) => Some(LogSource::Network),
            Some(LogSource::Network) => Some(LogSource::Disk),
            Some(LogSource::Disk) => Some(LogSource::Storage),
            Some(LogSource::Storage) => Some(LogSource::Ui),
            Some(LogSource::Ui) => None,
        };
        self.log_scroll.offset = 0;
    }

    pub fn cycle_log_time_range(&mut self) {
        self.log_filter.time_range = match self.log_filter.time_range {
            LogTimeRange::All => LogTimeRange::LastMinute,
            LogTimeRange::LastMinute => LogTimeRange::LastFiveMinutes,
            LogTimeRange::LastFiveMinutes => LogTimeRange::LastFifteenMinutes,
            LogTimeRange::LastFifteenMinutes => LogTimeRange::LastHour,
            LogTimeRange::LastHour => LogTimeRange::All,
        };
        self.log_scroll.offset = 0;
    }

    pub fn clear_log_view(&mut self) {
        self.log_filter = LogFilter::default();
        self.search_input.clear();
        self.log_scroll.offset = 0;
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

    #[test]
    fn persistent_rotation_failure_keeps_memory_logging_available() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ovm-top-app-fallback-{}-{unique}",
            std::process::id()
        ));
        let logger = crate::log::PersistentLogger::with_config(
            root.clone(),
            std::time::SystemTime::now(),
            1,
            7,
        )
        .unwrap();
        let mut app = App {
            persistent_logger: Some(logger),
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        tx.send(LogEvent::info(LogSource::Ui, "first")).unwrap();
        app.drain_logs(&rx);
        std::fs::remove_dir_all(&root).unwrap();
        tx.send(LogEvent::info(LogSource::Ui, "second")).unwrap();

        app.drain_logs(&rx);

        assert_eq!(app.logs.len(), 2);
        assert!(app.persistent_logger.is_none());
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
        assert_eq!(app.detail_target, DetailTarget::None);
    }

    #[test]
    fn detail_targets_are_mutually_exclusive() {
        let mut app = app_with_fc_maps(1);
        app.domains = Arc::new(Mutex::new(vec![DomainView {
            name: "domain-a".into(),
            id: 1,
            state: "-b----".into(),
            memory_mb: 1,
            vcpus: 1,
            cpu_percent: 0.0,
            memory_percent: 0.0,
            net_tx_kb: 0.0,
            net_rx_kb: 0.0,
            vbd_rd: 0,
            vbd_wr: 0,
            identity_source: crate::topology::snapshot::IdentitySource::FullName,
        }]));
        app.topology = Arc::new(Mutex::new(TopologySnapshot {
            interfaces: vec![crate::topology::snapshot::NetInterface {
                name: "xenbr0".into(),
                ..Default::default()
            }],
            ..Default::default()
        }));

        assert!(app.open_domain_detail());
        assert_eq!(app.detail_target, DetailTarget::Domain(1));
        assert!(app.open_network_detail());
        assert!(!app.domain_detail_open);
        assert_eq!(app.detail_target, DetailTarget::Network("xenbr0".into()));
        assert!(app.open_fc_detail());
        assert!(!app.network_detail_open);
        assert_eq!(app.detail_target, DetailTarget::Multipath("wwid-0".into()));
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

    #[test]
    fn fc_detail_closes_when_refresh_removes_selected_map() {
        let mut app = app_with_fc_maps(2);
        app.fc_selected = Some(1);
        assert!(app.open_fc_detail());

        app.fc.lock().unwrap().maps.pop();
        app.reconcile_fc_detail();

        assert_eq!(app.fc_panel_mode, FcPanelMode::Summary);
        assert_eq!(app.fc_detail_map, None);
        assert_eq!(app.detail_target, DetailTarget::None);
        assert_eq!(app.fc_selected, Some(0));
    }

    #[test]
    fn fc_detail_follows_wwid_when_refresh_reorders_maps() {
        let mut app = app_with_fc_maps(2);
        app.fc_selected = Some(1);
        assert!(app.open_fc_detail());

        app.fc.lock().unwrap().maps.swap(0, 1);
        app.reconcile_fc_detail();

        assert_eq!(app.detail_target, DetailTarget::Multipath("wwid-1".into()));
        assert_eq!(app.fc_detail_map, Some(0));
    }
}
