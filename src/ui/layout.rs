use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Root {
    pub header: Rect,
    pub workspace: Rect,
    pub footer: Rect,
}

pub(super) fn root(area: Rect) -> Root {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(area);
    Root {
        header: areas[0],
        workspace: areas[1],
        footer: areas[2],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MasterDetail {
    pub master: Option<Rect>,
    pub detail: Rect,
}

pub(super) fn viewport(area: Rect) -> usize {
    area.height.saturating_sub(3) as usize
}

/// Classify the terminal using the space left after the fixed header/footer.
/// A 70x20 terminal therefore is SMALL (its workspace is only 14 rows).
pub(super) fn screen_class(area: Rect) -> &'static str {
    let workspace_height = area.height.saturating_sub(6);
    if area.width >= 100 && workspace_height >= 24 {
        "LARGE"
    } else if area.width >= 70 && workspace_height >= 18 {
        "MEDIUM"
    } else {
        "SMALL"
    }
}

pub(super) fn compact(area: Rect) -> bool {
    area.width < 100
}

pub(super) fn range(offset: usize, total: usize, visible: usize) -> String {
    if total == 0 {
        return "0/0".into();
    }
    let start = offset.min(total - 1) + 1;
    let end = offset.saturating_add(visible).min(total);
    if start == end {
        format!("{start}/{total}")
    } else {
        format!("{start}-{end}/{total}")
    }
}

pub(super) fn master_detail(area: Rect, detail: bool) -> MasterDetail {
    if !detail {
        return MasterDetail {
            master: Some(area),
            detail: area,
        };
    }
    if area.width >= 100 && area.height >= 24 {
        let parts = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        MasterDetail {
            master: Some(parts[0]),
            detail: parts[1],
        }
    } else if area.width >= 70 && area.height >= 18 {
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        MasterDetail {
            master: Some(parts[0]),
            detail: parts[1],
        }
    } else {
        MasterDetail {
            master: None,
            detail: area,
        }
    }
}

pub(super) fn full_screen(area: Rect) -> Rect {
    area
}

pub(super) fn popup(frame: &Frame, width: u16, height: u16) -> Rect {
    let area = frame.area();
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_ranges_are_unambiguous() {
        assert_eq!(range(0, 0, 5), "0/0");
        assert_eq!(range(0, 5, 5), "1-5/5");
        assert_eq!(range(3, 23, 4), "4-7/23");
    }

    #[test]
    fn responsive_detail_modes() {
        assert!(
            master_detail(Rect::new(0, 0, 60, 15), true)
                .master
                .is_none()
        );
        assert!(
            master_detail(Rect::new(0, 0, 100, 30), true)
                .master
                .is_some()
        );
    }

    #[test]
    fn root_keeps_header_and_footer_geometry_centralized() {
        let layout = root(Rect::new(0, 0, 100, 30));
        assert_eq!(layout.header.height, 3);
        assert_eq!(layout.workspace.height, 24);
        assert_eq!(layout.footer.height, 3);
    }

    #[test]
    fn screen_class_uses_workspace_height() {
        assert_eq!(screen_class(Rect::new(0, 0, 70, 20)), "SMALL");
        assert_eq!(screen_class(Rect::new(0, 0, 70, 24)), "MEDIUM");
        assert_eq!(screen_class(Rect::new(0, 0, 100, 30)), "LARGE");
    }
}
