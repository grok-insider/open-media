//! Mouse event handling: click/double-click routing per screen, wheel
//! scrolling, and the double-click tracker.

use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::prelude::Rect;

use super::draw::{home_row_height, library_row_height};
use super::input::{
    activate_control, play_selected, select_episode, select_home, select_library_item,
    select_result, select_season,
};
use super::layout::{
    episodes_layout, header_layout, list_index_at, list_index_at_heights, panel_control_at,
    rect_contains, results_layout, sources_layout, sources_side_panel_layout, tab_at,
    top_level_layout,
};
use super::state::{Focus, PanelControl, Screen};
use super::App;

#[derive(Clone, Copy)]
pub(super) struct LastMouseClick {
    screen: Screen,
    x: u16,
    y: u16,
    at: Instant,
}

pub(super) fn handle_mouse(app: &mut App, mouse: MouseEvent, area: Rect) {
    if app.help {
        match mouse.kind {
            MouseEventKind::Down(_) => {
                app.help = false;
                app.last_click = None;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {}
            _ => {}
        }
        return;
    }

    let layout = top_level_layout(area);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Tab bar clicks.
            let header = header_layout(layout.header);
            let compact = area.width < 80;
            if let Some(root) = tab_at(header.tabs, mouse.column, mouse.row, compact) {
                app.go_root(root);
                app.last_click = None;
                return;
            }

            let double = is_double_click(app, mouse.column, mouse.row);
            handle_left_click(app, layout.body, mouse.column, mouse.row, double);
        }
        MouseEventKind::ScrollUp => handle_wheel(app, layout.body, mouse.column, mouse.row, -1),
        MouseEventKind::ScrollDown => handle_wheel(app, layout.body, mouse.column, mouse.row, 1),
        _ => {}
    }
}

fn is_double_click(app: &mut App, x: u16, y: u16) -> bool {
    let now = Instant::now();
    let screen = app.screen();
    let double = app
        .last_click
        .map(|last| {
            last.screen == screen
                && last.x == x
                && last.y == y
                && now.duration_since(last.at) <= Duration::from_millis(500)
        })
        .unwrap_or(false);
    app.last_click = Some(LastMouseClick {
        screen,
        x,
        y,
        at: now,
    });
    double
}

fn handle_left_click(app: &mut App, body: Rect, x: u16, y: u16, double: bool) {
    match app.screen() {
        Screen::Home => {
            let heights: Vec<u16> = app
                .home_rows
                .iter()
                .map(|r| home_row_height(r, &app.continue_watching))
                .collect();
            if let Some(idx) = list_index_at_heights(body, x, y, &heights) {
                if app.home_rows.get(idx).is_some_and(|r| r.is_selectable()) {
                    app.home_state.select(Some(idx));
                    if double {
                        select_home(app);
                    }
                }
            }
        }
        Screen::Library => {
            // Filter chips row is the first line of body.
            let chunks = ratatui::layout::Layout::vertical([
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Min(1),
            ])
            .split(body);
            if rect_contains(chunks[0], x, y) {
                // Approximate filter chip hit: cycle by x position.
                let rel = x.saturating_sub(chunks[0].x) as usize;
                let chip_w = 12usize;
                let idx = rel / chip_w;
                let filters = [
                    super::state::LibraryFilter::All,
                    super::state::LibraryFilter::Watching,
                    super::state::LibraryFilter::Planned,
                    super::state::LibraryFilter::Completed,
                    super::state::LibraryFilter::Dropped,
                ];
                if let Some(f) = filters.get(idx) {
                    app.library_filter = *f;
                    app.load_library();
                }
                return;
            }
            let heights: Vec<u16> = app.library.iter().map(library_row_height).collect();
            if let Some(idx) = list_index_at_heights(chunks[1], x, y, &heights) {
                app.library_state.select(Some(idx));
                if double {
                    select_library_item(app);
                }
            }
        }
        Screen::Search => {
            let _focused = rect_contains(body, x, y);
        }
        Screen::Results => {
            let layout = results_layout(body);
            if let Some(idx) = list_index_at(layout.list, x, y, app.results.len()) {
                app.results_state.select(Some(idx));
                if double {
                    select_result(app);
                }
            }
        }
        Screen::Seasons => {
            if let Some(idx) = list_index_at(body, x, y, app.seasons.len()) {
                app.seasons_state.select(Some(idx));
                if double {
                    select_season(app);
                }
            }
        }
        Screen::Episodes => {
            let layout = episodes_layout(body);
            if let Some(idx) = list_index_at(layout.list, x, y, app.episodes.len()) {
                app.episodes_state.select(Some(idx));
                if double {
                    select_episode(app);
                }
            }
        }
        Screen::Sources => handle_sources_click(app, body, x, y, double),
    }
}

fn handle_sources_click(app: &mut App, body: Rect, x: u16, y: u16, double: bool) {
    let layout = sources_layout(body);
    if let Some(idx) = list_index_at(layout.list, x, y, app.visible.len()) {
        app.candidates_state.select(Some(idx));
        app.focus = Focus::List;
        if double {
            play_selected(app);
        }
        return;
    }

    let Some(panel) = layout.panel else {
        return;
    };
    let panel_layout = sources_side_panel_layout(panel);
    if let Some(control) = panel_control_at(panel_layout.filters, x, y) {
        app.focus = Focus::Panel;
        let idx = PanelControl::ALL
            .iter()
            .position(|&c| c == control)
            .unwrap_or(0);
        app.panel_state.select(Some(idx));
        if double {
            activate_control(app);
        }
    }
}

fn handle_wheel(app: &mut App, body: Rect, x: u16, y: u16, delta: i32) {
    match app.screen() {
        Screen::Home if rect_contains(body, x, y) => app.home_list_move(delta),
        Screen::Library if rect_contains(body, x, y) => {
            let chunks = ratatui::layout::Layout::vertical([
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Min(1),
            ])
            .split(body);
            if rect_contains(chunks[1], x, y) {
                App::list_move(&mut app.library_state, app.library.len(), delta);
            }
        }
        Screen::Results => {
            let layout = results_layout(body);
            if rect_contains(layout.list, x, y) {
                App::list_move(&mut app.results_state, app.results.len(), delta);
            }
        }
        Screen::Seasons if rect_contains(body, x, y) => {
            App::list_move(&mut app.seasons_state, app.seasons.len(), delta)
        }
        Screen::Episodes => {
            let layout = episodes_layout(body);
            if rect_contains(layout.list, x, y) {
                App::list_move(&mut app.episodes_state, app.episodes.len(), delta);
            }
        }
        Screen::Sources => {
            let layout = sources_layout(body);
            if rect_contains(layout.list, x, y) {
                App::list_move(&mut app.candidates_state, app.visible.len(), delta);
                return;
            }
            if let Some(panel) = layout.panel {
                let panel_layout = sources_side_panel_layout(panel);
                if rect_contains(panel_layout.filters, x, y) {
                    App::list_move(&mut app.panel_state, PanelControl::ALL.len(), delta);
                }
            }
        }
        Screen::Search => {}
        _ => {}
    }
}
