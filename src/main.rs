mod app;
mod collector;
mod log;
mod topology;
mod ui;

use std::{
    fs, io,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use crate::{
    app::{
        App, DetailTarget, DiskView, FcPanelMode, Focus, InputMode, LogFilter, NetworkView,
        ScrollResult, ScrollState, Workspace,
    },
    collector::{disk, net, xentop, xm},
    log::{LogEvent, LogLevel, LogSource, PersistentLogger},
};
use ratatui::{Terminal, backend::CrosstermBackend};

fn main() -> Result<()> {
    let realtime_domains = Arc::new(Mutex::new(Vec::<xentop::DomainStats>::new()));
    let domains = Arc::new(Mutex::new(Vec::<xentop::DomainView>::new()));
    let domain_meta = Arc::new(Mutex::new(xentop::DomainSnapshotMeta::default()));
    let topology = Arc::new(Mutex::new(topology::snapshot::TopologySnapshot::default()));
    let network = Arc::new(Mutex::new(Vec::new()));
    let disk_rates = Arc::new(Mutex::new(Vec::new()));
    let fc = Arc::new(Mutex::new(collector::fc::FcSnapshot::default()));
    let (log_tx, log_rx) = mpsc::channel();
    let (persistent_logger, log_history) = match PersistentLogger::from_home() {
        Ok(logger) => {
            let (history, warnings) = logger.load_history(crate::app::MAX_LOG_ENTRIES);
            for warning in warnings {
                eprintln!("ovm-top warning: {warning}");
            }
            (Some(logger), history)
        }
        Err(error) => {
            eprintln!(
                "ovm-top warning: persistent logging unavailable; using memory only: {error}"
            );
            (None, Default::default())
        }
    };

    let mut app = App {
        xm_info: if std::env::var("OVM_TOP_MOCK").is_ok() {
            xm::mock_xm_info()
        } else {
            xm::get_xm_info()?
        },

        domains: Arc::clone(&domains),
        domain_meta: Arc::clone(&domain_meta),
        topology: Arc::clone(&topology),
        network: Arc::clone(&network),
        disk: Arc::clone(&disk_rates),
        fc: Arc::clone(&fc),
        focus: Focus::Domains,
        workspace: Workspace::Overview,
        previous_workspace: Workspace::Overview,
        input_mode: InputMode::Normal,
        domain_scroll: ScrollState::default(),
        domain_selected: None,
        domain_detail_scroll: ScrollState::default(),
        domain_detail_open: false,
        domain_detail_index: None,
        network_scroll: ScrollState::default(),
        network_selected: None,
        network_detail_scroll: ScrollState::default(),
        network_detail_open: false,
        network_view: NetworkView::Performance,
        disk_scroll: ScrollState::default(),
        disk_selected: None,
        disk_detail_scroll: ScrollState::default(),
        disk_view: DiskView::Performance,
        fc_scroll: ScrollState::default(),
        fc_selected: None,
        fc_detail_scroll: ScrollState::default(),
        fc_panel_mode: FcPanelMode::Summary,
        fc_detail_map: None,
        detail_target: crate::app::DetailTarget::None,
        fc_config: Default::default(),
        fc_config_open: false,
        fc_config_index: 2,
        logs: log_history,
        persistent_logger,
        log_scroll: ScrollState::default(),
        log_filter: LogFilter::default(),
        search_input: String::new(),
        show_logs: false,
        last_scroll_boundary: None,
    };

    let _ = log_tx.send(LogEvent::info(LogSource::System, "ovm-top started"));

    if std::env::var("OVM_TOP_MOCK").is_err() {
        xentop::spawn_collector(
            Arc::clone(&realtime_domains),
            Arc::clone(&domain_meta),
            log_tx.clone(),
        )?;

        xentop::spawn_domain_view_collector(
            Arc::clone(&realtime_domains),
            Arc::clone(&domains),
            Arc::clone(&domain_meta),
            log_tx.clone(),
        );
        topology::snapshot::spawn_collector(Arc::clone(&topology), log_tx.clone());
    }

    net::spawn_network_collector(Arc::clone(&network), log_tx.clone());
    disk::spawn_disk_collector(Arc::clone(&disk_rates), log_tx.clone());
    collector::fc::spawn_fc_collector(Arc::clone(&fc), log_tx.clone());

    enable_raw_mode()?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, &mut app, &log_rx, &log_tx);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let _ = terminal.show_cursor();

    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    log_rx: &mpsc::Receiver<LogEvent>,
    log_tx: &mpsc::Sender<LogEvent>,
) -> Result<()> {
    loop {
        app.drain_logs(log_rx);
        terminal.draw(|frame| {
            ui::draw(frame, app);
        })?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if app.fc_config_open {
                    match key.code {
                        KeyCode::Esc => {
                            app.fc_config_open = false;
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Up => app.move_fc_config(false),
                        KeyCode::Down => app.move_fc_config(true),
                        KeyCode::Enter => app.apply_fc_config(),
                        _ => {}
                    }
                    continue;
                }
                if app.input_mode == InputMode::Search {
                    match key.code {
                        KeyCode::Esc => {
                            app.search_input.clear();
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Enter => {
                            app.log_filter.text = app.search_input.clone();
                            app.log_scroll.offset = 0;
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Backspace => {
                            app.search_input.pop();
                        }
                        KeyCode::Char(character) => app.search_input.push(character),
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('?') => {
                        app.input_mode = if app.input_mode == InputMode::Help {
                            InputMode::Normal
                        } else {
                            InputMode::Help
                        };
                    }
                    KeyCode::Esc if app.input_mode == InputMode::Help => {
                        app.input_mode = InputMode::Normal;
                    }
                    KeyCode::Esc if app.fc_panel_mode == FcPanelMode::Detail => {
                        app.close_fc_detail();
                        let _ = log_tx.send(LogEvent::info(LogSource::Ui, "FC/SAN detail closed"));
                    }
                    KeyCode::Esc if app.domain_detail_open => {
                        app.close_domain_detail();
                    }
                    KeyCode::Esc if app.network_detail_open => {
                        app.close_network_detail();
                    }
                    KeyCode::Esc if matches!(app.detail_target, DetailTarget::Disk(_)) => {
                        app.close_disk_detail();
                    }
                    KeyCode::Esc if app.workspace != Workspace::Overview => {
                        app.set_workspace(Workspace::Overview);
                    }
                    KeyCode::Esc => break,
                    KeyCode::Char('0') => set_workspace(app, Workspace::Overview, log_tx),
                    KeyCode::Char('1') => set_workspace(app, Workspace::Domains, log_tx),
                    KeyCode::Char('2') => set_workspace(app, Workspace::Network, log_tx),
                    KeyCode::Char('3') => set_workspace(app, Workspace::Disk, log_tx),
                    KeyCode::Char('5') => {
                        app.focus_fc();
                        log_focus_change(app, log_tx);
                    }
                    KeyCode::Char('4') => {
                        if app.workspace == Workspace::Logs {
                            app.hide_logs();
                            let _ = log_tx.send(LogEvent::info(LogSource::Ui, "logs hidden"));
                        } else {
                            app.focus_logs();
                            log_focus_change(app, log_tx);
                        }
                    }
                    KeyCode::Char('h')
                        if app.focus == Focus::Fc && app.fc_panel_mode == FcPanelMode::Detail =>
                    {
                        app.close_fc_detail();
                        let _ = log_tx.send(LogEvent::info(LogSource::Ui, "FC/SAN detail hidden"));
                    }
                    KeyCode::Char('h') if app.focus == Focus::Domains && app.domain_detail_open => {
                        app.close_domain_detail();
                    }
                    KeyCode::Char('h')
                        if app.focus == Focus::Network && app.network_detail_open =>
                    {
                        app.close_network_detail();
                    }
                    KeyCode::Char('h') if matches!(app.detail_target, DetailTarget::Disk(_)) => {
                        app.close_disk_detail();
                    }
                    KeyCode::Char('h') if app.workspace != Workspace::Overview => {
                        app.set_workspace(Workspace::Overview);
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if app.focus == Focus::Fc
                            && app.fc_panel_mode == FcPanelMode::Summary
                            && app.fc_config.view == crate::app::FcView::Multipath
                        {
                            app.select_fc_down();
                        } else if app.select_workspace_row(true) {
                        } else {
                            let result = app.scroll_down();
                            log_scroll_result(app, result, log_tx);
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        if app.focus == Focus::Fc
                            && app.fc_panel_mode == FcPanelMode::Summary
                            && app.fc_config.view == crate::app::FcView::Multipath
                        {
                            app.select_fc_up();
                        } else if app.select_workspace_row(false) {
                        } else {
                            let result = app.scroll_up();
                            log_scroll_result(app, result, log_tx);
                        }
                    }
                    KeyCode::Char('l')
                        if app.focus == Focus::Fc && app.fc_panel_mode == FcPanelMode::Summary =>
                    {
                        if app.open_fc_detail() {
                            let _ =
                                log_tx.send(LogEvent::info(LogSource::Ui, "FC/SAN detail opened"));
                        }
                    }
                    KeyCode::Char('l')
                        if app.focus == Focus::Domains && !app.domain_detail_open =>
                    {
                        let _ = app.open_domain_detail();
                    }
                    KeyCode::Char('l')
                        if app.focus == Focus::Network && !app.network_detail_open =>
                    {
                        let _ = app.open_network_detail();
                    }
                    KeyCode::Char('l') if app.focus == Focus::Disk => {
                        let _ = app.open_disk_detail();
                    }
                    KeyCode::Enter if app.focus == Focus::Domains => {
                        let _ = app.open_domain_detail();
                    }
                    KeyCode::Enter if app.focus == Focus::Network => {
                        let _ = app.open_network_detail();
                    }
                    KeyCode::Enter if app.focus == Focus::Disk => {
                        let _ = app.open_disk_detail();
                    }
                    KeyCode::Enter if app.focus == Focus::Fc => {
                        let _ = app.open_fc_detail();
                    }
                    KeyCode::Char('u')
                        if app.focus == Focus::Fc && app.fc_panel_mode == FcPanelMode::Detail =>
                    {
                        if !app.preview_fc_map(-1) {
                            let _ = log_tx.send(LogEvent::info(
                                LogSource::Ui,
                                "FC/SAN detail reached first map",
                            ));
                        }
                    }
                    KeyCode::Char('d')
                        if app.focus == Focus::Fc && app.fc_panel_mode == FcPanelMode::Detail =>
                    {
                        if !app.preview_fc_map(1) {
                            let _ = log_tx.send(LogEvent::info(
                                LogSource::Ui,
                                "FC/SAN detail reached last map",
                            ));
                        }
                    }
                    KeyCode::Char('/') if app.focus == Focus::Logs => {
                        app.search_input = app.log_filter.text.clone();
                        app.input_mode = InputMode::Search;
                    }
                    KeyCode::Char('f') if app.focus == Focus::Logs => app.cycle_log_filter(),
                    KeyCode::Char('F') if app.focus == Focus::Logs => app.cycle_log_source(),
                    KeyCode::Char('t') if app.focus == Focus::Logs => app.cycle_log_time_range(),
                    KeyCode::Char('c') if app.focus == Focus::Logs => app.clear_log_view(),
                    KeyCode::Char('C') if app.focus == Focus::Logs => {
                        app.logs.clear();
                        app.log_scroll = ScrollState::default();
                    }
                    KeyCode::Char('s') => {
                        if app.focus == Focus::Logs {
                            save_logs(app, log_tx);
                        }
                    }
                    KeyCode::Char('o') if app.focus == Focus::Fc => app.open_fc_config(),
                    KeyCode::Char('v') if app.focus == Focus::Network => {
                        app.network_view = match app.network_view {
                            NetworkView::Performance => NetworkView::Topology,
                            NetworkView::Topology => NetworkView::Performance,
                        };
                        app.network_detail_scroll.offset = 0;
                    }
                    KeyCode::Char('v') if app.focus == Focus::Disk => {
                        app.disk_view = match app.disk_view {
                            DiskView::Performance => DiskView::Topology,
                            DiskView::Topology => DiskView::Performance,
                        };
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        for _ in 0..app.current_scroll_state().viewport.max(2) / 2 {
                            let _ = app.scroll_down();
                        }
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        for _ in 0..app.current_scroll_state().viewport.max(2) / 2 {
                            let _ = app.scroll_up();
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn focus_name(focus: Focus) -> &'static str {
    match focus {
        Focus::Domains => "Domains",
        Focus::Network => "Network",
        Focus::Disk => "Disk",
        Focus::Fc => "FC/SAN",
        Focus::Logs => "Logs",
    }
}

fn set_workspace(app: &mut App, workspace: Workspace, log_tx: &mpsc::Sender<LogEvent>) {
    if app.workspace != workspace {
        app.set_workspace(workspace);
        log_focus_change(app, log_tx);
    }
}

fn log_focus_change(app: &App, log_tx: &mpsc::Sender<LogEvent>) {
    let _ = log_tx.send(LogEvent::info(
        LogSource::Ui,
        format!("focus changed to {}", focus_name(app.focus)),
    ));
}

fn log_scroll_result(app: &App, result: ScrollResult, log_tx: &mpsc::Sender<LogEvent>) {
    let name = focus_name(app.focus);
    let state = app.current_scroll_state();
    let max_offset = state.total.saturating_sub(state.viewport);
    let message = match result {
        ScrollResult::Moved { from, to } => {
            format!(
                "scroll {name}: {from} -> {to} (total={}, viewport={}, offset={}, max={max_offset})",
                state.total, state.viewport, state.offset
            )
        }
        ScrollResult::AtTop { repeated: false } => format!(
            "scroll {name}: reached top (total={}, viewport={}, offset={}, max={max_offset})",
            state.total, state.viewport, state.offset
        ),
        ScrollResult::AtBottom { repeated: false } => format!(
            "scroll {name}: reached bottom (total={}, viewport={}, offset={}, max={max_offset})",
            state.total, state.viewport, state.offset
        ),
        ScrollResult::AtTop { repeated: true } | ScrollResult::AtBottom { repeated: true } => {
            return;
        }
    };
    let _ = log_tx.send(LogEvent::info(LogSource::Ui, message));
}

fn save_logs(app: &App, log_tx: &mpsc::Sender<LogEvent>) {
    let result = (|| -> Result<PathBuf> {
        let directory = PathBuf::from("logs");
        fs::create_dir_all(&directory)?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let path = directory.join(format!("ovm-top-{timestamp}.log"));
        let content = app
            .logs
            .iter()
            .map(|entry| {
                format!(
                    "[{}] {:5} {:6} {}\n",
                    crate::log::timestamp(entry),
                    crate::log::level_label(entry.level),
                    crate::log::source_label(entry.source),
                    entry.message
                )
            })
            .collect::<String>();
        fs::write(&path, content)?;
        Ok(path)
    })();

    let event = match result {
        Ok(path) => LogEvent {
            level: LogLevel::Info,
            source: LogSource::Ui,
            message: format!("logs saved to {}", path.display()),
        },
        Err(error) => LogEvent {
            level: LogLevel::Error,
            source: LogSource::Ui,
            message: format!("failed to save logs: {error}"),
        },
    };
    let _ = log_tx.send(event);
}
