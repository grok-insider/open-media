//! Interactive terminal UI (ratatui).
//!
//! A render-from-state loop with an `mpsc` channel for async results — the
//! littlejohn/miru pattern. All engine I/O is `tokio::spawn`ed and posts a
//! [`Msg`] back, so the UI never blocks. Flow:
//!   Search → Results → (episodic ⇒ Episodes) → Sources → Playing → back.

use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{execute, terminal};
use om_app::{Engine, PlayRequest};
use om_core::model::{Media, MediaKind};
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
    Episodes,
    Sources,
}

/// Async results posted back to the UI loop.
enum Msg {
    Results(Vec<Media>),
    Episodes(Media),
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
    episodes: Vec<u32>,
    episodes_state: ListState,

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
            episodes: Vec::new(),
            episodes_state: ListState::default(),
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
            Msg::Episodes(media) => {
                let count = media.episode_count.unwrap_or(12).max(1);
                self.episodes = (1..=count).collect();
                self.episodes_state.select(Some(0));
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
        Screen::Episodes => match code {
            KeyCode::Char('j') | KeyCode::Down => {
                App::list_move(&mut app.episodes_state, app.episodes.len(), 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                App::list_move(&mut app.episodes_state, app.episodes.len(), -1)
            }
            KeyCode::Enter => select_episode(app),
            KeyCode::Esc => app.screen = Screen::Results,
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
    let engine = app.engine.clone();
    let tx = app.tx.clone();
    tokio::spawn(async move {
        // Hydrate ids (IMDB) for sources; fall back to the search result.
        let media = engine.details(&media.ids).await.unwrap_or(media);
        if media.kind == MediaKind::Movie {
            send_sources(&engine, &tx, media, None, None).await;
        } else {
            let _ = tx.send(Msg::Episodes(media));
        }
    });
}

fn select_episode(app: &mut App) {
    let (Some(idx), Some(media)) = (app.episodes_state.selected(), app.media.clone()) else {
        return;
    };
    let episode = app.episodes.get(idx).copied().unwrap_or(1);
    app.busy = true;
    app.status = format!("Finding sources for episode {episode}…");
    let engine = app.engine.clone();
    let tx = app.tx.clone();
    tokio::spawn(async move {
        send_sources(&engine, &tx, media, Some(1), Some(episode)).await;
    });
}

async fn send_sources(
    engine: &Arc<Engine>,
    tx: &mpsc::UnboundedSender<Msg>,
    media: Media,
    season: Option<u32>,
    episode: Option<u32>,
) {
    let req = PlayRequest {
        media: media.clone(),
        season,
        episode,
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
    let episode = app
        .episodes_state
        .selected()
        .and_then(|i| app.episodes.get(i).copied());
    let season = media.kind.is_episodic().then_some(1);
    app.busy = true;
    app.status = format!("Playing {}…", media.display_title());

    let engine = app.engine.clone();
    let tx = app.tx.clone();
    tokio::spawn(async move {
        let req = PlayRequest {
            media,
            season,
            episode,
            include_uncached: true,
        };
        let _ = tx.send(Msg::Status("Resolving + launching player…".into()));
        let _ = match engine.play(&req, &candidate).await {
            Ok(()) => tx.send(Msg::PlayEnded),
            Err(e) => tx.send(Msg::Error(e.to_string())),
        };
    });
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
        Screen::Episodes => draw_episodes(f, app, chunks[1]),
        Screen::Sources => draw_sources(f, app, chunks[1]),
    }

    // Footer / status
    let hints = match app.screen {
        Screen::Search => "type query · Enter: search · Esc: quit",
        Screen::Results => "j/k: move · Enter: select · /: search · q: quit",
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

fn draw_episodes(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .episodes
        .iter()
        .map(|n| ListItem::new(format!("Episode {n}")))
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
