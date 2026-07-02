//! Mouse event handling: click/double-click routing per screen, wheel
//! scrolling, and the double-click tracker.

use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::prelude::Rect;

use super::input::{
    activate_control, play_selected, select_episode, select_home, select_library_item,
    select_result, select_season,
};
use super::layout::{
    episodes_layout, list_index_at, panel_control_at, rect_contains, results_layout,
    sources_layout, sources_side_panel_layout, top_level_layout,
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
    let double = app
        .last_click
        .map(|last| {
            last.screen == app.screen
                && last.x == x
                && last.y == y
                && now.duration_since(last.at) <= Duration::from_millis(500)
        })
        .unwrap_or(false);
    app.last_click = Some(LastMouseClick {
        screen: app.screen,
        x,
        y,
        at: now,
    });
    double
}

fn handle_left_click(app: &mut App, body: Rect, x: u16, y: u16, double: bool) {
    match app.screen {
        Screen::Home => {
            if let Some(idx) = list_index_at(body, x, y, 4) {
                app.home_state.select(Some(idx));
                if double {
                    select_home(app);
                }
            }
        }
        Screen::Library => {
            if let Some(idx) = list_index_at(body, x, y, app.library.len()) {
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
    match app.screen {
        Screen::Home if rect_contains(body, x, y) => App::list_move(&mut app.home_state, 4, delta),
        Screen::Library if rect_contains(body, x, y) => {
            App::list_move(&mut app.library_state, app.library.len(), delta)
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
