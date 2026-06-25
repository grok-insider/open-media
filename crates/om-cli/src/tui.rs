//! Interactive terminal UI (ratatui).
//!
//! A render-from-state loop with an `mpsc` channel for async results — the
//! littlejohn/miru pattern. All engine I/O is `tokio::spawn`ed and posts a
//! [`Msg`] back, so the UI never blocks. Flow:
//!   Search → Results → (episodic ⇒ Seasons? ⇒ Episodes) → Sources → play → back.

use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{execute, terminal};
use om_app::{Engine, PlayRequest};
use om_core::model::{Episode, Media, MediaKind, Season};
use om_core::stream::{CacheState, SourceCandidate};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use tokio::sync::mpsc;

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Which screen is active.
#[derive(PartialEq)]
enum Screen {
    Search,
    Results,
    Seasons,
    Episodes,
    Sources,
}

/// Async results posted back to the UI loop.
enum Msg {
    Results(Vec<Media>),
    /// Multi-season series → show the season picker.
    Seasons(Media, Vec<Season>),
    /// Episodes for a resolved season (real titles when the provider has them).
    Episodes(Media, u32, Vec<Episode>),
    Sources {
        media: Media,
        candidates: Vec<SourceCandidate>,
    },
    PlayEnded,
    Status(String),
    Error(String),
}

struct App {
    engine: Arc<Engine>,
    screen: Screen,
    query: String,
    status: String,
    busy: bool,
    should_quit: bool,

    results: Vec<Media>,
    results_state: ListState,

    media: Option<Media>,

    seasons: Vec<Season>,
    seasons_state: ListState,

    episodes: Vec<Episode>,
    episodes_state: ListState,

    /// Coordinates resolved at episode/movie selection, threaded into the play
    /// request so sources and the player title agree (and don't desync with the
    /// list cursor). Movies leave all three `None`.
    sel_season: Option<u32>,
    sel_episode: Option<u32>,
    sel_episode_title: Option<String>,

    candidates: Vec<SourceCandidate>,
    candidates_state: ListState,

    tx: mpsc::UnboundedSender<Msg>,
}

impl App {
    fn new(
        engine: Arc<Engine>,
        tx: mpsc::UnboundedSender<Msg>,
        initial_query: Option<String>,
    ) -> Self {
        Self {
            engine,
            screen: Screen::Search,
            query: initial_query.unwrap_or_default(),
            status: "Type a title and press Enter".into(),
            busy: false,
            should_quit: false,
            results: Vec::new(),
            results_state: ListState::default(),
            media: None,
            seasons: Vec::new(),
            seasons_state: ListState::default(),
            episodes: Vec::new(),
            episodes_state: ListState::default(),
            sel_season: None,
            sel_episode: None,
            sel_episode_title: None,
            candidates: Vec::new(),
            candidates_state: ListState::default(),
            tx,
        }
    }

    fn handle_msg(&mut self, msg: Msg) {
        self.busy = false;
        match msg {
            Msg::Results(r) => {
                self.results = r;
                self.results_state
                    .select((!self.results.is_empty()).then_some(0));
                self.screen = Screen::Results;
                self.status = format!("{} results", self.results.len());
            }
            Msg::Seasons(media, seasons) => {
                self.seasons = seasons;
                self.seasons_state
                    .select((!self.seasons.is_empty()).then_some(0));
                self.media = Some(media);
                self.screen = Screen::Seasons;
                self.status = "Pick a season".into();
            }
            Msg::Episodes(media, season, episodes) => {
                self.episodes = episodes;
                self.episodes_state
                    .select((!self.episodes.is_empty()).then_some(0));
                self.sel_season = Some(season);
                self.media = Some(media);
                self.screen = Screen::Episodes;
                self.status = "Pick an episode".into();
            }
            Msg::Sources { media, candidates } => {
                self.candidates = candidates;
                self.candidates_state
                    .select((!self.candidates.is_empty()).then_some(0));
                self.media = Some(media);
                self.screen = Screen::Sources;
                self.status = format!("{} sources (Enter to play)", self.candidates.len());
            }
            Msg::PlayEnded => self.status = "Playback ended".into(),
            Msg::Status(s) => {
                self.busy = true;
                self.status = s;
            }
            Msg::Error(e) => self.status = format!("Error: {e}"),
        }
    }

    fn list_move(state: &mut ListState, len: usize, delta: i32) {
        if len == 0 {
            return;
        }
        let cur = state.selected().unwrap_or(0) as i32;
        let next = (cur + delta).rem_euclid(len as i32) as usize;
        state.select(Some(next));
    }
}

/// Run the TUI to completion.
pub async fn run(engine: Engine, initial_query: Option<String>) -> anyhow::Result<()> {
    let engine = Arc::new(engine);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App::new(engine, tx, initial_query);

    let mut term = setup_terminal()?;
    let result = run_loop(&mut term, &mut app, &mut rx).await;
    restore_terminal(&mut term)?;
    result
}

async fn run_loop(
    term: &mut Term,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
) -> anyhow::Result<()> {
    loop {
        term.draw(|f| draw(f, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key.code, key.modifiers);
                }
            }
        }
        while let Ok(msg) = rx.try_recv() {
            app.handle_msg(msg);
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    match app.screen {
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
            KeyCode::Char('/') | KeyCode::Esc => app.screen = Screen::Search,
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
        Screen::Sources => match code {
            KeyCode::Char('j') | KeyCode::Down => {
                App::list_move(&mut app.candidates_state, app.candidates.len(), 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                App::list_move(&mut app.candidates_state, app.candidates.len(), -1)
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
    }
}

fn start_search(app: &mut App) {
    let query = app.query.trim().to_string();
    if query.is_empty() {
        return;
    }
    app.busy = true;
    app.status = format!("Searching “{query}”…");
    let engine = app.engine.clone();
    let tx = app.tx.clone();
    tokio::spawn(async move {
        let _ = match engine.search(&query, None).await {
            Ok(r) => tx.send(Msg::Results(r)),
            Err(e) => tx.send(Msg::Error(e.to_string())),
        };
    });
}

fn select_result(app: &mut App) {
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
    let engine = app.engine.clone();
    let tx = app.tx.clone();
    tokio::spawn(async move {
        // Hydrate ids (IMDB) for sources; fall back to the search result.
        let media = engine.details(&media.ids).await.unwrap_or(media);
        if media.kind == MediaKind::Movie {
            // Movies have no coordinates or episode title.
            send_sources(&engine, &tx, media, None, None, None).await;
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

fn select_season(app: &mut App) {
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
        })
        .collect()
}

fn select_episode(app: &mut App) {
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
    app.busy = true;
    app.status = format!("Finding sources for {}…", ep_coordinate(&ep));
    let engine = app.engine.clone();
    let tx = app.tx.clone();
    let (season, episode, title) = (Some(ep.season), Some(ep.number), ep.title);
    tokio::spawn(async move {
        send_sources(&engine, &tx, media, season, episode, title).await;
    });
}

async fn send_sources(
    engine: &Arc<Engine>,
    tx: &mpsc::UnboundedSender<Msg>,
    media: Media,
    season: Option<u32>,
    episode: Option<u32>,
    episode_title: Option<String>,
) {
    let req = PlayRequest {
        media: media.clone(),
        season,
        episode,
        episode_title,
        include_uncached: true,
    };
    let _ = match engine.find_sources(&req).await {
        Ok(candidates) => tx.send(Msg::Sources { media, candidates }),
        Err(e) => tx.send(Msg::Error(e.to_string())),
    };
}

fn play_selected(app: &mut App) {
    let (Some(idx), Some(media)) = (app.candidates_state.selected(), app.media.clone()) else {
        return;
    };
    let Some(candidate) = app.candidates.get(idx).cloned() else {
        return;
    };
    // Use the coordinates pinned at episode selection — not the live cursor.
    let season = app.sel_season;
    let episode = app.sel_episode;
    let episode_title = app.sel_episode_title.clone();
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

// --- Rendering ---

fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(f.area());

    // Header
    let title = match app.screen {
        Screen::Search => "open-media — Search",
        Screen::Results => "open-media — Results",
        Screen::Seasons => "open-media — Seasons",
        Screen::Episodes => "open-media — Episodes",
        Screen::Sources => "open-media — Sources",
    };
    f.render_widget(
        Paragraph::new(title)
            .style(Style::new().fg(Color::Cyan).bold())
            .block(Block::default().borders(Borders::ALL)),
        chunks[0],
    );

    match app.screen {
        Screen::Search => draw_search(f, app, chunks[1]),
        Screen::Results => draw_results(f, app, chunks[1]),
        Screen::Seasons => draw_seasons(f, app, chunks[1]),
        Screen::Episodes => draw_episodes(f, app, chunks[1]),
        Screen::Sources => draw_sources(f, app, chunks[1]),
    }

    // Footer / status
    let hints = match app.screen {
        Screen::Search => "type query · Enter: search · Esc: quit",
        Screen::Results => "j/k: move · Enter: select · /: search · q: quit",
        Screen::Seasons => "j/k: move · Enter: select · Esc: back · q: quit",
        Screen::Episodes => "j/k: move · Enter: select · Esc: back · q: quit",
        Screen::Sources => "j/k: move · Enter: play · Esc: back · q: quit",
    };
    let spin = if app.busy { "⏳ " } else { "" };
    f.render_widget(
        Paragraph::new(format!("{spin}{}", app.status))
            .style(Style::new().fg(Color::Gray))
            .block(Block::default().borders(Borders::ALL).title(hints)),
        chunks[2],
    );
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(
        Paragraph::new(format!("› {}", app.query))
            .block(Block::default().borders(Borders::ALL).title("Query"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_results(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|m| {
            let badge = match m.kind {
                MediaKind::Movie => "[Movie]",
                MediaKind::Series => "[TV]",
                MediaKind::Anime => "[Anime]",
            };
            let year = m.year.map(|y| format!(" ({y})")).unwrap_or_default();
            ListItem::new(format!("{badge} {}{year}", m.display_title()))
        })
        .collect();
    render_list(f, area, "Results", items, &app.results_state);
}

fn draw_seasons(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .seasons
        .iter()
        .map(|s| {
            let name = s
                .name
                .clone()
                .unwrap_or_else(|| format!("Season {}", s.number));
            ListItem::new(format!("{name}  ({} episodes)", s.episode_count))
        })
        .collect();
    render_list(f, area, "Seasons", items, &app.seasons_state);
}

fn draw_episodes(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .episodes
        .iter()
        .map(|ep| {
            let label = match &ep.title {
                Some(t) if !t.is_empty() => format!("E{:02} · {t}", ep.number),
                _ => format!("E{:02}", ep.number),
            };
            ListItem::new(label)
        })
        .collect();
    render_list(f, area, "Episodes", items, &app.episodes_state);
}

fn draw_sources(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .candidates
        .iter()
        .map(|c| {
            let cache = match c.cache {
                CacheState::Cached => "⚡",
                CacheState::Uncached => "⬇",
                CacheState::Unknown => " ",
            };
            let seeders = c.seeders.map(|s| format!("{s}S")).unwrap_or_default();
            ListItem::new(format!(
                "{cache} {:<6} {:<9} {:<5} [{}] {}",
                c.quality.label(),
                c.human_size(),
                seeders,
                c.provider,
                c.title
            ))
        })
        .collect();
    render_list(f, area, "Sources", items, &app.candidates_state);
}

fn render_list(f: &mut Frame, area: Rect, title: &str, items: Vec<ListItem>, state: &ListState) {
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title.to_string()),
        )
        .highlight_style(Style::new().bg(Color::DarkGray).fg(Color::White).bold())
        .highlight_symbol("▶ ");
    let mut s = state.clone();
    f.render_stateful_widget(list, area, &mut s);
}

fn setup_terminal() -> anyhow::Result<Term> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(term: &mut Term) -> anyhow::Result<()> {
    terminal::disable_raw_mode()?;
    execute!(term.backend_mut(), terminal::LeaveAlternateScreen)?;
    term.show_cursor()?;
    Ok(())
}
