mod app;
mod collector;
mod log;
mod ui;

use std::{
    fs, io,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use crate::{
    app::{App, Focus, LogFilter, ScrollResult, ScrollState},
    collector::{disk, net, xentop, xm},
    log::{LogEvent, LogLevel, LogSource},
};
use ratatui::{Terminal, backend::CrosstermBackend};

fn main() -> Result<()> {
    let realtime_domains = Arc::new(Mutex::new(Vec::<xentop::DomainStats>::new()));
    let domains = Arc::new(Mutex::new(Vec::<xentop::DomainView>::new()));
    let network = Arc::new(Mutex::new(Vec::new()));
    let disk_rates = Arc::new(Mutex::new(Vec::new()));
    let (log_tx, log_rx) = mpsc::channel();

    let mut app = App {
        xm_info: if std::env::var("OVM_TOP_MOCK").is_ok() {
            xm::mock_xm_info()
        } else {
            xm::get_xm_info()?
        },

        domains: Arc::clone(&domains),
        network: Arc::clone(&network),
        disk: Arc::clone(&disk_rates),
        focus: Focus::Domains,
        domain_scroll: ScrollState::default(),
        network_scroll: ScrollState::default(),
        disk_scroll: ScrollState::default(),
        logs: Default::default(),
        log_scroll: ScrollState::default(),
        log_filter: LogFilter::default(),
        show_logs: false,
        last_scroll_boundary: None,
    };

    let _ = log_tx.send(LogEvent::info(LogSource::System, "ovm-top started"));

    if std::env::var("OVM_TOP_MOCK").is_err() {
        xentop::spawn_collector(Arc::clone(&realtime_domains), log_tx.clone())?;

        xentop::spawn_domain_view_collector(
            Arc::clone(&realtime_domains),
            Arc::clone(&domains),
            log_tx.clone(),
        );
    }

    net::spawn_network_collector(Arc::clone(&network), log_tx.clone());
    disk::spawn_disk_collector(Arc::clone(&disk_rates), log_tx.clone());

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
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('1') => set_focus(app, Focus::Domains, log_tx),
                    KeyCode::Char('2') => set_focus(app, Focus::Network, log_tx),
                    KeyCode::Char('3') => set_focus(app, Focus::Disk, log_tx),
                    KeyCode::Char('4') => {
                        app.focus_logs();
                        log_focus_change(app, log_tx);
                    }
                    KeyCode::Char('l') => {
                        if app.show_logs {
                            app.hide_logs();
                            let _ = log_tx.send(LogEvent::info(LogSource::Ui, "logs hidden"));
                        } else {
                            app.focus_logs();
                            log_focus_change(app, log_tx);
                        }
                    }
                    KeyCode::Char('j') => {
                        let result = app.scroll_down();
                        log_scroll_result(app, result, log_tx);
                    }
                    KeyCode::Char('k') => {
                        let result = app.scroll_up();
                        log_scroll_result(app, result, log_tx);
                    }
                    KeyCode::Char('f') if app.focus == Focus::Logs => app.cycle_log_filter(),
                    KeyCode::Char('c') if app.focus == Focus::Logs => app.logs.clear(),
                    KeyCode::Char('s') => {
                        if app.focus == Focus::Logs {
                            save_logs(app, log_tx);
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
        Focus::Logs => "Logs",
    }
}

fn set_focus(app: &mut App, focus: Focus, log_tx: &mpsc::Sender<LogEvent>) {
    if app.focus != focus {
        app.focus = focus;
        app.last_scroll_boundary = None;
        let _ = log_tx.send(LogEvent::info(
            LogSource::Ui,
            format!("focus changed to {}", focus_name(focus)),
        ));
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
