//! Pure screen geometry shared by mouse hit-testing and rendering: the layout
//! structs, the split/panel calculations, and rect/list coordinate helpers.

use ratatui::prelude::{Constraint, Layout, Rect};

use super::state::{PanelControl, Root};
use super::{EPISODE_PANEL_WIDTH, PANEL_MIN_WIDTH, PANEL_WIDTH, RESULT_PANEL_WIDTH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TopLevelLayout {
    pub(super) header: Rect,
    pub(super) body: Rect,
    pub(super) footer: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HeaderLayout {
    pub(super) brand: Rect,
    pub(super) tabs: Rect,
    pub(super) trail: Rect,
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

/// Header (brand + tabs + trail) · body · footer (status + key hints).
/// Header is 2 rows so tabs read clearly; footer is 3 (bordered status+hints).
pub(super) fn top_level_layout(area: Rect) -> TopLevelLayout {
    let chunks = Layout::vertical([
        Constraint::Length(2),
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

/// Split the header into brand | tabs | trail | right margin.
pub(super) fn header_layout(area: Rect) -> HeaderLayout {
    // Narrow terminals: shorter brand column so tabs stay usable.
    // Reserve 2 columns on the right so the trail never touches the edge.
    const RIGHT_PAD: u16 = 2;
    let brand_w = if area.width < 80 { 10 } else { 14 };
    let usable = area.width.saturating_sub(RIGHT_PAD);
    let trail_w = if usable < 80 {
        usable.saturating_sub(brand_w + 18).min(24)
    } else {
        usable.saturating_mul(35) / 100
    };
    let cols = Layout::horizontal([
        Constraint::Length(brand_w),
        Constraint::Min(18),
        Constraint::Length(trail_w),
        Constraint::Length(RIGHT_PAD),
    ])
    .split(area);
    HeaderLayout {
        brand: cols[0],
        tabs: cols[1],
        trail: cols[2],
    }
}

/// Which root tab (if any) contains the click. Tabs are laid out left-to-right
/// with fixed slots based on label length + padding.
pub(super) fn tab_at(area: Rect, x: u16, y: u16, compact: bool) -> Option<Root> {
    if !rect_contains(area, x, y) {
        return None;
    }
    let mut cursor = area.x;
    for root in Root::ALL {
        let label = if compact { root.short() } else { root.label() };
        // " Home " style padding (1 space each side) + optional brackets for
        // active — hit box uses a generous fixed width.
        let w = (label.len() as u16).saturating_add(4).max(6);
        let end = cursor.saturating_add(w);
        if x >= cursor && x < end {
            return Some(root);
        }
        cursor = end;
        if cursor >= area.x.saturating_add(area.width) {
            break;
        }
    }
    None
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
    centered_rect(72, 70, area)
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

/// List hit-test when items have variable heights (e.g. 2-line progress grids).
pub(super) fn list_index_at_heights(area: Rect, x: u16, y: u16, heights: &[u16]) -> Option<usize> {
    let inner = border_inner(area);
    if !rect_contains(inner, x, y) {
        return None;
    }
    let mut row = inner.y;
    for (i, &h) in heights.iter().enumerate() {
        let h = h.max(1);
        if y >= row && y < row.saturating_add(h) {
            return Some(i);
        }
        row = row.saturating_add(h);
        if row >= inner.y.saturating_add(inner.height) {
            break;
        }
    }
    None
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
