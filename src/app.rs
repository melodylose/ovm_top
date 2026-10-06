use std::{
    collections::VecDeque,
    sync::mpsc::Receiver,
    sync::{Arc, Mutex},
};

use crate::{
    collector::{disk::DiskRate, net::NetRate, xentop::DomainView, xm::XmInfo},
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
    Logs,
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
    pub focus: Focus,
    pub domain_scroll: ScrollState,
    pub network_scroll: ScrollState,
    pub disk_scroll: ScrollState,
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
            Focus::Logs => self.log_scroll,
        }
    }

    fn scroll_state_mut(&mut self) -> &mut ScrollState {
        match self.focus {
            Focus::Domains => &mut self.domain_scroll,
            Focus::Network => &mut self.network_scroll,
            Focus::Disk => &mut self.disk_scroll,
            Focus::Logs => &mut self.log_scroll,
        }
    }

    pub fn focus_logs(&mut self) {
        self.show_logs = true;
        self.focus = Focus::Logs;
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
}
