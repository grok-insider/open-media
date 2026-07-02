//! Interactive terminal UI (ratatui).
//!
//! A render-from-state loop with an `mpsc` channel for async results — the
//! littlejohn/miru pattern. All engine I/O is `tokio::spawn`ed and posts a
//! [`Msg`] back, so the UI never blocks. Flow:
//!   Search → Results → (episodic ⇒ Seasons? ⇒ Episodes) → Sources → play → back.

mod draw;
mod input;
mod layout;
mod mouse;
mod state;
#[cfg(test)]
mod tests;

use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::{execute, terminal};
use open_media_app::{Engine, SearchProgress};
use open_media_config::Config;
use open_media_core::model::{Episode, Media, Season};
use open_media_core::stream::SourceCandidate;
use open_media_core::tracking::LibraryItem;
use ratatui::prelude::*;
use ratatui::widgets::ListState;
use tokio::sync::mpsc;

use crate::stills::{StillMsg, Stills};

use draw::draw;
use input::{handle_key, search_status, start_search};
use mouse::{handle_mouse, LastMouseClick};
use state::{
    distinct_languages, distinct_providers, visible_indices, Focus, LibraryFilter, Screen,
    SourceFilters, Theme,
};

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Below this terminal width the Sources side panel is hidden (list goes full
/// width) so narrow terminals aren't squeezed.
const PANEL_MIN_WIDTH: u16 = 100;
/// Width of the Sources side panel.
const PANEL_WIDTH: u16 = 34;
/// Width of the Episodes detail side panel (a touch wider — it holds wrapped
/// synopsis text).
const EPISODE_PANEL_WIDTH: u16 = 40;
/// Rows reserved at the top of the episode panel for the still image.
const STILL_ROWS: u16 = 11;
/// Target cell box `(cols, rows)` the still is resized to fit. Width matches the
/// panel interior (border-inset); `Resize::Fit` keeps the aspect ratio within
/// this, so landscape stills won't use the full height.
const STILL_TARGET_CELLS: (u16, u16) = (EPISODE_PANEL_WIDTH - 2, STILL_ROWS);
/// Width of the passive Results detail panel.
const RESULT_PANEL_WIDTH: u16 = 42;
/// Rows reserved for posters in media detail panels.
const POSTER_ROWS: u16 = 16;
const POSTER_TARGET_CELLS: (u16, u16) = (RESULT_PANEL_WIDTH - 2, POSTER_ROWS);

/// Async results posted back to the UI loop.
enum Msg {
    SearchProgress {
        search_id: u64,
        progress: SearchProgress,
    },
    SearchError {
        search_id: u64,
        error: String,
    },
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
    search_id: u64,
    status: String,
    busy: bool,
    should_quit: bool,
    help: bool,

    home_state: ListState,

    library: Vec<LibraryItem>,
    all_library: Vec<LibraryItem>,
    library_state: ListState,
    library_filter: LibraryFilter,

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
    sel_episode_still: Option<String>,
    /// Selected episode's runtime (minutes), forwarded to AniSkip for interval
    /// validation. `None` for movies/unknown.
    sel_episode_runtime: Option<u32>,

    candidates: Vec<SourceCandidate>,
    candidates_state: ListState,

    /// Resolved color palette (from `cfg.ui.theme`).
    theme: Theme,

    /// Sources side panel: filters/sort, focus, and the derived visible view.
    cfg: Config,
    focus: Focus,
    filters: SourceFilters,
    panel_state: ListState,
    /// Indices into `candidates` after filtering + sorting; the list cursor
    /// selects within this, not `candidates` directly.
    visible: Vec<usize>,
    languages: Vec<String>,
    providers: Vec<String>,

    /// Terminal-image rendering for episode stills / posters (kitty/sixel/
    /// iTerm2, or unicode half-blocks). Holds the detected picker + still cache.
    stills: Stills,
    still_tx: mpsc::UnboundedSender<StillMsg>,

    tx: mpsc::UnboundedSender<Msg>,

    last_click: Option<LastMouseClick>,
}

impl App {
    fn new(
        engine: Arc<Engine>,
        cfg: Config,
        tx: mpsc::UnboundedSender<Msg>,
        stills: Stills,
        still_tx: mpsc::UnboundedSender<StillMsg>,
        initial_query: Option<String>,
    ) -> Self {
        let filters = SourceFilters::from_cfg(&cfg.ui.sources);
        let theme = Theme::from_cfg(&cfg.ui.theme);
        let mut panel_state = ListState::default();
        panel_state.select(Some(0));
        Self {
            engine,
            screen: if initial_query.is_some() {
                Screen::Search
            } else {
                Screen::Home
            },
            query: initial_query.unwrap_or_default(),
            search_id: 0,
            status: "Browse your library or press / to search".into(),
            busy: false,
            should_quit: false,
            help: false,
            home_state: {
                let mut s = ListState::default();
                s.select(Some(0));
                s
            },
            library: Vec::new(),
            all_library: Vec::new(),
            library_state: ListState::default(),
            library_filter: LibraryFilter::All,
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
            sel_episode_still: None,
            sel_episode_runtime: None,
            candidates: Vec::new(),
            candidates_state: ListState::default(),
            theme,
            cfg,
            focus: Focus::List,
            filters,
            panel_state,
            visible: Vec::new(),
            languages: Vec::new(),
            providers: Vec::new(),
            stills,
            still_tx,
            tx,
            last_click: None,
        }
    }

    /// Recompute the filtered/sorted view and clamp the list cursor.
    fn recompute_visible(&mut self) {
        self.visible = visible_indices(&self.candidates, &self.filters);
        let sel = if self.visible.is_empty() {
            None
        } else {
            Some(
                self.candidates_state
                    .selected()
                    .unwrap_or(0)
                    .min(self.visible.len() - 1),
            )
        };
        self.candidates_state.select(sel);
        self.update_sources_status();
    }

    fn update_sources_status(&mut self) {
        let total = self.candidates.len();
        let shown = self.visible.len();
        self.status = if shown == total {
            format!("{total} sources")
        } else if shown == 0 {
            format!("0 of {total} sources — no match for filters (Clear to reset)")
        } else {
            format!("{shown} of {total} sources")
        };
    }

    /// The candidate currently highlighted in the (filtered) list, if any.
    fn current_candidate(&self) -> Option<&SourceCandidate> {
        let sel = self.candidates_state.selected()?;
        let idx = *self.visible.get(sel)?;
        self.candidates.get(idx)
    }

    /// The image URL to show for the currently-highlighted episode: its own
    /// still if it has one, else the series poster as a fallback.
    fn current_still_url(&self) -> Option<&str> {
        let ep = self.episodes.get(self.episodes_state.selected()?)?;
        ep.still
            .as_deref()
            .or(self.media.as_ref().and_then(|m| m.poster.as_deref()))
    }

    fn current_result_poster_url(&self) -> Option<&str> {
        let media = self.results.get(self.results_state.selected()?)?;
        media.poster.as_deref()
    }

    /// Ask the still loader to fetch the image for the selected episode. Cheap
    /// to call every frame: it only acts on the Episodes screen when images are
    /// supported, and the loader ignores URLs already loading/ready/failed.
    fn request_visible_image(&mut self) {
        if !self.stills.enabled() {
            return;
        }
        let target = match self.screen {
            Screen::Episodes => STILL_TARGET_CELLS,
            Screen::Results => POSTER_TARGET_CELLS,
            _ => return,
        };
        let url = match self.screen {
            Screen::Episodes => self.current_still_url(),
            Screen::Results => self.current_result_poster_url(),
            _ => None,
        };
        let Some(url) = url.map(str::to_string) else {
            return;
        };
        self.stills.request(&url, target, self.still_tx.clone());
    }

    fn load_library(&mut self) {
        if let Ok(items) = self.engine.list_library(None) {
            self.all_library = items;
        }
        match self.engine.list_library(self.library_filter.status()) {
            Ok(items) => {
                self.library = items;
                self.library_state
                    .select((!self.library.is_empty()).then_some(0));
                self.status = if self.library.is_empty() {
                    format!(
                        "No {} items yet",
                        self.library_filter.label().to_ascii_lowercase()
                    )
                } else {
                    format!(
                        "{} {} items",
                        self.library.len(),
                        self.library_filter.label()
                    )
                };
            }
            Err(e) => self.status = format!("Library unavailable: {e}"),
        }
    }

    fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::SearchProgress {
                search_id,
                progress,
            } => {
                if search_id != self.search_id {
                    return;
                }
                self.apply_search_progress(progress);
            }
            Msg::SearchError { search_id, error } => {
                if search_id != self.search_id {
                    return;
                }
                self.busy = false;
                self.status = format!("Error: {error}");
            }
            Msg::Seasons(media, seasons) => {
                self.busy = false;
                self.seasons = seasons;
                self.seasons_state
                    .select((!self.seasons.is_empty()).then_some(0));
                self.media = Some(media);
                self.screen = Screen::Seasons;
                self.status = "Pick a season".into();
            }
            Msg::Episodes(media, season, episodes) => {
                self.busy = false;
                self.episodes = episodes;
                self.episodes_state
                    .select((!self.episodes.is_empty()).then_some(0));
                self.sel_season = Some(season);
                self.media = Some(media);
                self.screen = Screen::Episodes;
                self.status = "Pick an episode".into();
            }
            Msg::Sources { media, candidates } => {
                self.busy = false;
                self.candidates = candidates;
                self.languages = distinct_languages(&self.candidates);
                self.providers = distinct_providers(&self.candidates);
                self.candidates_state
                    .select((!self.candidates.is_empty()).then_some(0));
                self.focus = Focus::List;
                self.panel_state.select(Some(0));
                self.media = Some(media);
                self.screen = Screen::Sources;
                self.recompute_visible();
            }
            Msg::PlayEnded => {
                self.busy = false;
                self.status = "Playback ended".into();
            }
            Msg::Status(s) => {
                self.busy = true;
                self.status = s;
            }
            Msg::Error(e) => {
                self.busy = false;
                self.status = format!("Error: {e}");
            }
        }
    }

    fn apply_search_progress(&mut self, progress: SearchProgress) {
        self.results = progress.results;
        self.results_state
            .select((!self.results.is_empty()).then_some(0));
        if !self.results.is_empty() || progress.finished {
            self.screen = Screen::Results;
        }
        self.busy = !progress.finished;
        self.status = search_status(
            self.results.len(),
            progress.failed_providers,
            progress.finished,
        );
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
pub async fn run(engine: Engine, cfg: Config, initial_query: Option<String>) -> anyhow::Result<()> {
    let engine = Arc::new(engine);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (still_tx, mut still_rx) = mpsc::unbounded_channel();

    // Enter the alternate screen *before* probing terminal graphics support:
    // `Stills::detect` briefly reads/writes stdio, which must happen after the
    // alt-screen switch but before we start consuming key events.
    let mut term = setup_terminal()?;
    let stills = Stills::detect();

    let mut app = App::new(engine, cfg, tx, stills, still_tx, initial_query);
    app.load_library();

    // A pre-filled query (`open-media "frieren"`) searches immediately;
    // start_search no-ops on an empty query, so bare `open-media` still lands on
    // an idle search box.
    start_search(&mut app);

    let result = run_loop(&mut term, &mut app, &mut rx, &mut still_rx).await;
    restore_terminal(&mut term)?;

    // Persist the Sources panel's filter/sort selections for next time.
    app.filters.write_cfg(&mut app.cfg.ui.sources);
    if let Err(e) = open_media_config::save(&app.cfg) {
        tracing::warn!(error = %e, "failed to persist UI prefs");
    }
    result
}

async fn run_loop(
    term: &mut Term,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
    still_rx: &mut mpsc::UnboundedReceiver<StillMsg>,
) -> anyhow::Result<()> {
    loop {
        term.draw(|f| draw(f, app))?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(app, key.code, key.modifiers);
                }
                Event::Mouse(mouse) => {
                    handle_mouse(app, mouse, term.size()?.into());
                }
                _ => {}
            }
        }
        while let Ok(msg) = rx.try_recv() {
            app.handle_msg(msg);
        }
        // Apply any finished still loads, then request the still for the
        // currently-selected episode (idempotent — the cache debounces it).
        while let Ok(msg) = still_rx.try_recv() {
            app.stills.apply(msg);
        }
        app.request_visible_image();

        if app.should_quit {
            return Ok(());
        }
    }
}

fn setup_terminal() -> anyhow::Result<Term> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(term: &mut Term) -> anyhow::Result<()> {
    terminal::disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        DisableMouseCapture,
        terminal::LeaveAlternateScreen
    )?;
    term.show_cursor()?;
    Ok(())
}
