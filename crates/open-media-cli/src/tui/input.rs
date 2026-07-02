//! Keyboard event handling and the user-initiated actions (select/search/play)
//! shared by the keyboard and mouse paths.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyModifiers};
use open_media_app::{Engine, PlayRequest};
use open_media_core::model::{Episode, Media, MediaKind};
use open_media_core::tracking::LibraryItem;
use tokio::sync::mpsc;

use super::state::{cycle_opt, cycle_quality, Focus, LibraryFilter, PanelControl, Screen};
use super::{App, Msg};

pub(super) fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    if app.help {
        match code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter => app.help = false,
            _ => {}
        }
        return;
    }

    if code == KeyCode::Char('?') {
        app.help = true;
        return;
    }

    if code == KeyCode::Char('/') && app.screen != Screen::Search {
        app.screen = Screen::Search;
        app.status = "Type a title and press Enter".into();
        return;
    }

    match app.screen {
        Screen::Home => match code {
            KeyCode::Char('j') | KeyCode::Down => App::list_move(&mut app.home_state, 4, 1),
            KeyCode::Char('k') | KeyCode::Up => App::list_move(&mut app.home_state, 4, -1),
            KeyCode::Enter => select_home(app),
            KeyCode::Char('l') => {
                app.screen = Screen::Library;
                app.load_library();
            }
            KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
            _ => {}
        },
        Screen::Library => match code {
            KeyCode::Char('j') | KeyCode::Down => {
                App::list_move(&mut app.library_state, app.library.len(), 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                App::list_move(&mut app.library_state, app.library.len(), -1)
            }
            KeyCode::Char('h') | KeyCode::Left => {
                app.library_filter = app.library_filter.cycle(-1);
                app.load_library();
            }
            KeyCode::Char('l') | KeyCode::Right => {
                app.library_filter = app.library_filter.cycle(1);
                app.load_library();
            }
            KeyCode::Enter => select_library_item(app),
            KeyCode::Esc => app.screen = Screen::Home,
            KeyCode::Char('q') => app.should_quit = true,
            _ => {}
        },
        Screen::Search => match code {
            KeyCode::Enter => start_search(app),
            KeyCode::Char(c) => app.query.push(c),
            KeyCode::Backspace => {
                app.query.pop();
            }
            KeyCode::Esc => app.should_quit = true,
            _ => {}
        },
        Screen::Results => match code {
            KeyCode::Char('j') | KeyCode::Down => {
                App::list_move(&mut app.results_state, app.results.len(), 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                App::list_move(&mut app.results_state, app.results.len(), -1)
            }
            KeyCode::Enter => select_result(app),
            KeyCode::Esc => app.screen = Screen::Search,
            KeyCode::Char('q') => app.should_quit = true,
            _ => {}
        },
        Screen::Seasons => match code {
            KeyCode::Char('j') | KeyCode::Down => {
                App::list_move(&mut app.seasons_state, app.seasons.len(), 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                App::list_move(&mut app.seasons_state, app.seasons.len(), -1)
            }
            KeyCode::Enter => select_season(app),
            KeyCode::Esc => app.screen = Screen::Results,
            KeyCode::Char('q') => app.should_quit = true,
            _ => {}
        },
        Screen::Episodes => match code {
            KeyCode::Char('j') | KeyCode::Down => {
                App::list_move(&mut app.episodes_state, app.episodes.len(), 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                App::list_move(&mut app.episodes_state, app.episodes.len(), -1)
            }
            KeyCode::Enter => select_episode(app),
            // Back to Seasons if we came through a multi-season picker, else Results.
            KeyCode::Esc => {
                app.screen = if app.seasons.len() > 1 {
                    Screen::Seasons
                } else {
                    Screen::Results
                }
            }
            KeyCode::Char('q') => app.should_quit = true,
            _ => {}
        },
        Screen::Sources => handle_sources_key(app, code),
    }
}

fn handle_sources_key(app: &mut App, code: KeyCode) {
    // Tab toggles focus between the list and the filter panel.
    if code == KeyCode::Tab {
        app.focus = match app.focus {
            Focus::List => Focus::Panel,
            Focus::Panel => Focus::List,
        };
        return;
    }

    match app.focus {
        Focus::List => match code {
            KeyCode::Char('j') | KeyCode::Down => {
                App::list_move(&mut app.candidates_state, app.visible.len(), 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                App::list_move(&mut app.candidates_state, app.visible.len(), -1)
            }
            KeyCode::Enter => play_selected(app),
            KeyCode::Esc => {
                app.screen = if app
                    .media
                    .as_ref()
                    .map(|m| m.kind.is_episodic())
                    .unwrap_or(false)
                {
                    Screen::Episodes
                } else {
                    Screen::Results
                }
            }
            KeyCode::Char('q') => app.should_quit = true,
            _ => {}
        },
        Focus::Panel => match code {
            KeyCode::Char('j') | KeyCode::Down => {
                App::list_move(&mut app.panel_state, PanelControl::ALL.len(), 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                App::list_move(&mut app.panel_state, PanelControl::ALL.len(), -1)
            }
            KeyCode::Char('h') | KeyCode::Left => adjust_filter(app, -1),
            KeyCode::Char('l') | KeyCode::Right => adjust_filter(app, 1),
            KeyCode::Enter => activate_control(app),
            KeyCode::Esc => app.focus = Focus::List,
            KeyCode::Char('q') => app.should_quit = true,
            _ => {}
        },
    }
}

/// Which panel control is focused.
fn focused_control(app: &App) -> PanelControl {
    PanelControl::ALL[app.panel_state.selected().unwrap_or(0)]
}

/// Change the focused control's value by `dir` (left/-1, right/+1).
fn adjust_filter(app: &mut App, dir: i32) {
    match focused_control(app) {
        PanelControl::Sort => app.filters.sort = app.filters.sort.cycle(dir),
        PanelControl::Quality => app.filters.quality = cycle_quality(app.filters.quality, dir),
        PanelControl::Language => {
            app.filters.language = cycle_opt(&app.filters.language, &app.languages, dir)
        }
        PanelControl::Provider => {
            app.filters.provider = cycle_opt(&app.filters.provider, &app.providers, dir)
        }
        PanelControl::Cached => app.filters.cached_only = !app.filters.cached_only,
        PanelControl::Clear => {}
    }
    app.recompute_visible();
}

/// Enter on a control: toggle Cached, run Clear, else behave like "next".
pub(super) fn activate_control(app: &mut App) {
    match focused_control(app) {
        PanelControl::Cached => {
            app.filters.cached_only = !app.filters.cached_only;
            app.recompute_visible();
        }
        PanelControl::Clear => {
            app.filters.clear();
            app.recompute_visible();
        }
        _ => adjust_filter(app, 1),
    }
}

pub(super) fn select_home(app: &mut App) {
    match app.home_state.selected().unwrap_or(0) {
        0 => {
            app.library_filter = LibraryFilter::Watching;
            app.screen = Screen::Library;
            app.load_library();
        }
        1 => {
            app.library_filter = LibraryFilter::Planned;
            app.screen = Screen::Library;
            app.load_library();
        }
        2 => {
            app.library_filter = LibraryFilter::Completed;
            app.screen = Screen::Library;
            app.load_library();
        }
        _ => {
            app.screen = Screen::Search;
            app.status = "Type a title and press Enter".into();
        }
    }
}

pub(super) fn select_library_item(app: &mut App) {
    let Some(idx) = app.library_state.selected() else {
        return;
    };
    let Some(item) = app.library.get(idx).cloned() else {
        return;
    };
    let media = media_from_library_item(&item);
    app.results = vec![media.clone()];
    app.results_state.select(Some(0));
    app.query = item.title.clone();
    if media.kind.is_episodic() && item.last_episode.is_some() {
        app.media = Some(media.clone());
        app.sel_season = item.last_season.or(Some(1));
        app.sel_episode = item.last_episode;
        app.sel_episode_title = None;
        app.sel_episode_still = None;
        app.sel_episode_runtime = None;
        app.busy = true;
        app.status = "Finding saved episode sources…".into();
        let engine = app.engine.clone();
        let tx = app.tx.clone();
        let season = app.sel_season;
        let episode = app.sel_episode;
        tokio::spawn(async move {
            let media = engine.details(&media.ids).await.unwrap_or(media);
            send_sources(&engine, &tx, media, season, episode, None, None, None).await;
        });
        return;
    }
    select_result(app);
}

fn media_from_library_item(item: &LibraryItem) -> Media {
    Media {
        kind: item.kind,
        ids: item.ids.clone(),
        title: item.title.clone(),
        original_title: None,
        year: item.year,
        score: None,
        overview: None,
        poster: item.poster.clone(),
        genres: Vec::new(),
        status: None,
        episode_count: None,
        season_count: None,
    }
}

pub(super) fn start_search(app: &mut App) {
    let query = app.query.trim().to_string();
    if query.is_empty() {
        return;
    }
    app.search_id = app.search_id.wrapping_add(1);
    let search_id = app.search_id;
    app.busy = true;
    app.results.clear();
    app.results_state.select(None);
    app.screen = Screen::Search;
    app.status = format!("Searching “{query}”…");
    let engine = app.engine.clone();
    let tx = app.tx.clone();
    tokio::spawn(async move {
        let result = engine
            .search_incremental(&query, None, |progress| {
                let _ = tx.send(Msg::SearchProgress {
                    search_id,
                    progress,
                });
            })
            .await;
        if let Err(e) = result {
            let _ = tx.send(Msg::SearchError {
                search_id,
                error: e.to_string(),
            });
        };
    });
}

pub(super) fn search_status(results: usize, failed_providers: usize, finished: bool) -> String {
    let mut status = if finished {
        format!("{results} results")
    } else {
        format!("{results} results · still searching...")
    };
    if failed_providers > 0 {
        let noun = if failed_providers == 1 {
            "provider"
        } else {
            "providers"
        };
        status.push_str(&format!(" · {failed_providers} {noun} failed"));
    }
    status
}

pub(super) fn select_result(app: &mut App) {
    let Some(idx) = app.results_state.selected() else {
        return;
    };
    let Some(media) = app.results.get(idx).cloned() else {
        return;
    };
    app.busy = true;
    app.status = "Loading…".into();
    // Reset prior episodic state so a new pick doesn't inherit stale coordinates.
    app.seasons.clear();
    app.sel_season = None;
    app.sel_episode = None;
    app.sel_episode_title = None;
    app.sel_episode_still = None;
    app.sel_episode_runtime = None;
    let engine = app.engine.clone();
    let tx = app.tx.clone();
    tokio::spawn(async move {
        // Hydrate ids (IMDB) for sources; fall back to the search result.
        let media = engine.details(&media.ids).await.unwrap_or(media);
        if media.kind == MediaKind::Movie {
            // Movies have no coordinates or episode title.
            send_sources(&engine, &tx, media, None, None, None, None, None).await;
            return;
        }
        // Episodic: list seasons. >1 → picker; otherwise jump straight to the
        // single (or synthetic, for flat-numbered anime) season's episodes.
        match engine.seasons(&media.ids).await {
            Ok(seasons) if seasons.len() > 1 => {
                let _ = tx.send(Msg::Seasons(media, seasons));
            }
            Ok(seasons) => {
                let season = seasons.first().map(|s| s.number).unwrap_or(1);
                fetch_episodes(&engine, &tx, media, season).await;
            }
            Err(_) => fetch_episodes(&engine, &tx, media, 1).await,
        }
    });
}

pub(super) fn select_season(app: &mut App) {
    let (Some(idx), Some(media)) = (app.seasons_state.selected(), app.media.clone()) else {
        return;
    };
    let season = app.seasons.get(idx).map(|s| s.number).unwrap_or(1);
    app.busy = true;
    app.status = format!("Loading season {season}…");
    let engine = app.engine.clone();
    let tx = app.tx.clone();
    tokio::spawn(async move {
        fetch_episodes(&engine, &tx, media, season).await;
    });
}

/// Fetch a season's episodes (real titles when available), degrading to bare
/// numbered entries from `episode_count` if the provider returns none.
async fn fetch_episodes(
    engine: &Arc<Engine>,
    tx: &mpsc::UnboundedSender<Msg>,
    media: Media,
    season: u32,
) {
    let episodes = match engine.episodes(&media.ids, season).await {
        Ok(eps) if !eps.is_empty() => eps,
        _ => fallback_episodes(season, media.episode_count.unwrap_or(1).max(1)),
    };
    let _ = tx.send(Msg::Episodes(media, season, episodes));
}

/// Bare episodes `1..=count` with no titles — the graceful fallback when a
/// provider can't enumerate a season (e.g. a currently-airing anime).
fn fallback_episodes(season: u32, count: u32) -> Vec<Episode> {
    (1..=count)
        .map(|number| Episode {
            season,
            number,
            title: None,
            air_date: None,
            overview: None,
            runtime_minutes: None,
            rating: None,
            still: None,
        })
        .collect()
}

pub(super) fn select_episode(app: &mut App) {
    let (Some(idx), Some(media)) = (app.episodes_state.selected(), app.media.clone()) else {
        return;
    };
    let Some(ep) = app.episodes.get(idx).cloned() else {
        return;
    };
    // Pin the chosen coordinates + title so sources and playback stay consistent.
    app.sel_season = Some(ep.season);
    app.sel_episode = Some(ep.number);
    app.sel_episode_title = ep.title.clone();
    app.sel_episode_still = ep.still.clone();
    app.sel_episode_runtime = ep.runtime_minutes;
    app.busy = true;
    app.status = format!("Finding sources for {}…", ep_coordinate(&ep));
    let engine = app.engine.clone();
    let tx = app.tx.clone();
    let (season, episode, title, still, runtime) = (
        Some(ep.season),
        Some(ep.number),
        ep.title,
        ep.still,
        ep.runtime_minutes,
    );
    tokio::spawn(async move {
        send_sources(&engine, &tx, media, season, episode, title, still, runtime).await;
    });
}

#[allow(clippy::too_many_arguments)]
async fn send_sources(
    engine: &Arc<Engine>,
    tx: &mpsc::UnboundedSender<Msg>,
    media: Media,
    season: Option<u32>,
    episode: Option<u32>,
    episode_title: Option<String>,
    episode_still: Option<String>,
    episode_runtime_minutes: Option<u32>,
) {
    let req = PlayRequest {
        media: media.clone(),
        season,
        episode,
        episode_title,
        episode_still,
        episode_runtime_minutes,
        include_uncached: true,
    };
    let _ = match engine.find_sources(&req).await {
        Ok(candidates) => tx.send(Msg::Sources { media, candidates }),
        Err(e) => tx.send(Msg::Error(e.to_string())),
    };
}

pub(super) fn play_selected(app: &mut App) {
    let Some(media) = app.media.clone() else {
        return;
    };
    // Map the (filtered) list cursor back to the underlying candidate.
    let Some(candidate) = app.current_candidate().cloned() else {
        return;
    };
    // Use the coordinates pinned at episode selection — not the live cursor.
    let season = app.sel_season;
    let episode = app.sel_episode;
    let episode_title = app.sel_episode_title.clone();
    let episode_still = app.sel_episode_still.clone();
    let episode_runtime_minutes = app.sel_episode_runtime;
    app.busy = true;
    app.status = format!("Playing {}…", media.display_title());

    let engine = app.engine.clone();
    let tx = app.tx.clone();
    tokio::spawn(async move {
        let req = PlayRequest {
            media,
            season,
            episode,
            episode_title,
            episode_still,
            episode_runtime_minutes,
            include_uncached: true,
        };
        let _ = tx.send(Msg::Status("Resolving + launching player…".into()));
        let _ = match engine.play(&req, &candidate).await {
            Ok(()) => tx.send(Msg::PlayEnded),
            Err(e) => tx.send(Msg::Error(e.to_string())),
        };
    });
}

/// `S01E01` for status lines.
fn ep_coordinate(ep: &Episode) -> String {
    format!("S{:02}E{:02}", ep.season, ep.number)
}
