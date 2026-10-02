mod app;
mod collector;
mod ui;

use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use crate::app::App;
use crate::collector::{net, xentop, xm};
use ratatui::{Terminal, backend::CrosstermBackend};

fn main() -> Result<()> {
    let domains = Arc::new(Mutex::new(Vec::new()));
    let network = Arc::new(Mutex::new(Vec::new()));

    let mut app = App {
        xm_info: if std::env::var("OVM_TOP_MOCK").is_ok() {
            xm::mock_xm_info()
        } else {
            xm::get_xm_info()?
        },

        domains: Arc::clone(&domains),
        network: Arc::clone(&network),
    };

    if std::env::var("OVM_TOP_MOCK").is_err() {
        xentop::spawn_collector(Arc::clone(&domains))?;
    } else {
        let mut d = domains.lock().unwrap();

        d.push(xentop::DomainStats {
            name: "Domain-0".into(),
            state: "-----r".into(),
            cpu_percent: 2.9,
            memory_percent: 1.6,
            vcpus: 20,
            ..Default::default()
        });
    }

    net::spawn_network_collector(Arc::clone(&network));

    enable_raw_mode()?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let _ = terminal.show_cursor();

    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| {
            ui::draw(frame, app);
        })?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    _ => {}
                }
            }
        }
    }

    Ok(())
}
