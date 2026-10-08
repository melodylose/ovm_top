use crate::app::{App, InputMode, Workspace};
use ratatui::Frame;

mod disk;
mod domains;
mod fc;
mod layout;
mod logs;
mod network;
mod overview;
mod topology;
mod widgets;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let root = layout::root(frame.area());
    widgets::draw_header(frame, app, root.header);
    match app.workspace {
        Workspace::Overview => overview::render(frame, app, root.workspace),
        Workspace::Domains => domains::render(frame, app, root.workspace),
        Workspace::Network => network::render(frame, app, root.workspace),
        Workspace::Disk => disk::render(frame, app, root.workspace),
        Workspace::Logs => logs::render(frame, app, root.workspace),
        Workspace::FcSan => fc::render(frame, app, root.workspace),
    }
    widgets::draw_footer(frame, app, root.footer);
    if app.input_mode == InputMode::Help {
        widgets::draw_help(frame);
    }
    if app.fc_config_open {
        widgets::draw_fc_menu(frame, app);
    }
}
