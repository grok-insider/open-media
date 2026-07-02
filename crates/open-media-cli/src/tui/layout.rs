//! Pure screen geometry shared by mouse hit-testing and rendering: the layout
//! structs, the split/panel calculations, and rect/list coordinate helpers.

use ratatui::prelude::{Constraint, Layout, Rect};

use super::state::PanelControl;
use super::{EPISODE_PANEL_WIDTH, PANEL_MIN_WIDTH, PANEL_WIDTH, RESULT_PANEL_WIDTH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TopLevelLayout {
    pub(super) header: Rect,
    pub(super) body: Rect,
    pub(super) footer: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SplitPanelLayout {
    pub(super) list: Rect,
    pub(super) panel: Option<Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SourcesPanelLayout {
    pub(super) filters: Rect,
    pub(super) details: Rect,
}

pub(super) fn top_level_layout(area: Rect) -> TopLevelLayout {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(area);
    TopLevelLayout {
        header: chunks[0],
        body: chunks[1],
        footer: chunks[2],
    }
}

pub(super) fn results_layout(area: Rect) -> SplitPanelLayout {
    split_optional_panel(area, RESULT_PANEL_WIDTH)
}

pub(super) fn episodes_layout(area: Rect) -> SplitPanelLayout {
    split_optional_panel(area, EPISODE_PANEL_WIDTH)
}

pub(super) fn sources_layout(area: Rect) -> SplitPanelLayout {
    split_optional_panel(area, PANEL_WIDTH)
}

fn split_optional_panel(area: Rect, panel_width: u16) -> SplitPanelLayout {
    if area.width >= PANEL_MIN_WIDTH {
        let cols =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(panel_width)]).split(area);
        SplitPanelLayout {
            list: cols[0],
            panel: Some(cols[1]),
        }
    } else {
        SplitPanelLayout {
            list: area,
            panel: None,
        }
    }
}

pub(super) fn sources_side_panel_layout(area: Rect) -> SourcesPanelLayout {
    let rows = Layout::vertical([Constraint::Length(8), Constraint::Min(0)]).split(area);
    SourcesPanelLayout {
        filters: rows[0],
        details: rows[1],
    }
}

pub(super) fn help_layout(area: Rect) -> Rect {
    centered_rect(72, 60, area)
}

pub(super) fn rect_contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x
        && x < area.x.saturating_add(area.width)
        && y >= area.y
        && y < area.y.saturating_add(area.height)
}

fn border_inner(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

pub(super) fn list_index_at(area: Rect, x: u16, y: u16, item_count: usize) -> Option<usize> {
    let inner = border_inner(area);
    if !rect_contains(inner, x, y) {
        return None;
    }
    let idx = y.saturating_sub(inner.y) as usize;
    (idx < item_count).then_some(idx)
}

pub(super) fn panel_control_at(area: Rect, x: u16, y: u16) -> Option<PanelControl> {
    list_index_at(area, x, y, PanelControl::ALL.len()).map(|idx| PanelControl::ALL[idx])
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}
