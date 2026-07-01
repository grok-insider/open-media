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
use open_media_app::{Engine, PlayRequest, SearchProgress};
use open_media_config::Config;
use open_media_core::model::{Episode, Media, MediaKind, Season};
use open_media_core::stream::{CacheState, Quality, SourceCandidate};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::Image;
use tokio::sync::mpsc;

use crate::stills::{StillMsg, Stills};

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

/// Semantic colors the TUI draws with. Replaces scattered `Color::*` literals
/// so the palette can be swapped wholesale by `ui.theme`. Two presets exist:
/// [`Theme::dark`] (the historical hardcoded look) and [`Theme::light`].
#[derive(Clone, Copy)]
struct Theme {
    /// Headings, focused borders, primary highlights.
    accent: Color,
    /// Footer/status body text.
    status: Color,
    /// Secondary/label text and unfocused borders.
    dim: Color,
    /// Selection / highlight background.
    selection_bg: Color,
    /// Selection / highlight foreground.
    selection_fg: Color,
}

impl Theme {
    /// The original hardcoded palette — kept byte-for-byte so `dark`/`auto` is a
    /// no-op for existing users.
    fn dark() -> Self {
        Self {
            accent: Color::Cyan,
            status: Color::Gray,
            dim: Color::DarkGray,
            selection_bg: Color::DarkGray,
            selection_fg: Color::White,
        }
    }

    /// A light palette: a deeper accent that reads on light backgrounds, darker
    /// dim/secondary text, and a light selection band with dark text.
    fn light() -> Self {
        Self {
            accent: Color::Blue,
            status: Color::DarkGray,
            dim: Color::Gray,
            selection_bg: Color::Gray,
            selection_fg: Color::Black,
        }
    }

    /// Pick a preset from `cfg.ui.theme` (`"light"` → light; `"dark"`/`"auto"`/
    /// anything else → dark). `"auto"` maps to dark for now; true
    /// terminal-background detection is a follow-up.
    fn from_cfg(theme: &str) -> Self {
        match theme.trim().to_ascii_lowercase().as_str() {
            "light" => Self::light(),
            _ => Self::dark(),
        }
    }

    /// Border color for a pane given its focus state.
    fn border(&self, focused: bool) -> Color {
        if focused {
            self.accent
        } else {
            self.dim
        }
    }

    /// The shared list highlight style (selection band).
    fn highlight(&self) -> Style {
        Style::new()
            .bg(self.selection_bg)
            .fg(self.selection_fg)
            .bold()
    }
}

/// Which screen is active.
#[derive(PartialEq)]
enum Screen {
    Search,
    Results,
    Seasons,
    Episodes,
    Sources,
}

/// Which pane has keyboard focus on the Sources screen.
#[derive(Clone, Copy, PartialEq)]
enum Focus {
    List,
    Panel,
}

/// How the visible candidates are ordered.
#[derive(Clone, Copy, PartialEq, Debug)]
enum SortKey {
    Relevance,
    Seeders,
    Quality,
    Size,
}

impl SortKey {
    fn label(self) -> &'static str {
        match self {
            SortKey::Relevance => "Relevance",
            SortKey::Seeders => "Seeders",
            SortKey::Quality => "Quality",
            SortKey::Size => "Size",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            SortKey::Relevance => "relevance",
            SortKey::Seeders => "seeders",
            SortKey::Quality => "quality",
            SortKey::Size => "size",
        }
    }
    fn from_str(s: &str) -> SortKey {
        match s.to_ascii_lowercase().as_str() {
            "seeders" => SortKey::Seeders,
            "quality" => SortKey::Quality,
            "size" => SortKey::Size,
            _ => SortKey::Relevance,
        }
    }
    /// Cycle through the sort keys by `dir` (+1 next / -1 previous).
    fn cycle(self, dir: i32) -> SortKey {
        const ALL: [SortKey; 4] = [
            SortKey::Relevance,
            SortKey::Seeders,
            SortKey::Quality,
            SortKey::Size,
        ];
        let pos = ALL.iter().position(|&s| s == self).unwrap_or(0) as i32;
        ALL[(pos + dir).rem_euclid(ALL.len() as i32) as usize]
    }
}

/// Rows in the filter/sort panel (the focusable controls).
#[derive(Clone, Copy, PartialEq)]
enum PanelControl {
    Sort,
    Quality,
    Language,
    Provider,
    Cached,
    Clear,
}

impl PanelControl {
    const ALL: [PanelControl; 6] = [
        PanelControl::Sort,
        PanelControl::Quality,
        PanelControl::Language,
        PanelControl::Provider,
        PanelControl::Cached,
        PanelControl::Clear,
    ];
}

/// The active filter + sort state for the Sources list.
#[derive(Clone)]
struct SourceFilters {
    sort: SortKey,
    quality: Option<Quality>,
    language: Option<String>,
    provider: Option<String>,
    cached_only: bool,
}

impl SourceFilters {
    /// Seed from persisted config.
    fn from_cfg(s: &open_media_config::SourcesUi) -> Self {
        Self {
            sort: SortKey::from_str(&s.sort),
            quality: parse_quality(&s.quality),
            language: parse_all_opt(&s.language),
            provider: parse_all_opt(&s.provider),
            cached_only: s.cached_only,
        }
    }

    /// Write back into config for persistence.
    fn write_cfg(&self, s: &mut open_media_config::SourcesUi) {
        s.sort = self.sort.as_str().to_string();
        s.quality = self
            .quality
            .map(|q| q.label().to_string())
            .unwrap_or_else(|| "all".into());
        s.language = self.language.clone().unwrap_or_else(|| "all".into());
        s.provider = self.provider.clone().unwrap_or_else(|| "all".into());
        s.cached_only = self.cached_only;
    }

    /// Whether a candidate passes all active filters.
    fn matches(&self, c: &SourceCandidate) -> bool {
        if let Some(q) = self.quality {
            if c.quality != q {
                return false;
            }
        }
        if self.cached_only && c.cache != CacheState::Cached {
            return false;
        }
        if let Some(lang) = &self.language {
            if !c.tags.languages.iter().any(|l| l == lang) {
                return false;
            }
        }
        if let Some(p) = &self.provider {
            if &c.provider != p {
                return false;
            }
        }
        true
    }

    fn clear(&mut self) {
        self.quality = None;
        self.language = None;
        self.provider = None;
        self.cached_only = false;
    }
}

/// `"all"`/empty → `None`, otherwise `Some(value)`.
fn parse_all_opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("all") {
        None
    } else {
        Some(t.to_string())
    }
}

/// `"all"` → `None`; a known label → `Some(Quality)`; unknown → `None`.
fn parse_quality(s: &str) -> Option<Quality> {
    match parse_all_opt(s) {
        None => None,
        Some(v) => match Quality::from_label(&v) {
            Quality::Unknown => None,
            q => Some(q),
        },
    }
}

/// Order candidates by the filters' sort key, returning indices into
/// `candidates`. Relevance keeps the engine's ranked order; all sorts are stable
/// (ties fall back to original index).
fn visible_indices(candidates: &[SourceCandidate], f: &SourceFilters) -> Vec<usize> {
    let mut idx: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| f.matches(c))
        .map(|(i, _)| i)
        .collect();
    match f.sort {
        SortKey::Relevance => {}
        SortKey::Seeders => idx.sort_by(|&a, &b| {
            candidates[b]
                .seeders
                .unwrap_or(0)
                .cmp(&candidates[a].seeders.unwrap_or(0))
                .then(a.cmp(&b))
        }),
        SortKey::Quality => idx.sort_by(|&a, &b| {
            candidates[b]
                .quality
                .cmp(&candidates[a].quality)
                .then(a.cmp(&b))
        }),
        SortKey::Size => idx.sort_by(|&a, &b| {
            candidates[b]
                .size_bytes
                .cmp(&candidates[a].size_bytes)
                .then(a.cmp(&b))
        }),
    }
    idx
}

/// Distinct, sorted values present across candidates (for the cycleable
/// language/provider option lists).
fn distinct_languages(candidates: &[SourceCandidate]) -> Vec<String> {
    let mut v: Vec<String> = candidates
        .iter()
        .flat_map(|c| c.tags.languages.iter().cloned())
        .collect();
    v.sort();
    v.dedup();
    v
}

fn distinct_providers(candidates: &[SourceCandidate]) -> Vec<String> {
    let mut v: Vec<String> = candidates.iter().map(|c| c.provider.clone()).collect();
    v.sort();
    v.dedup();
    v
}

/// Cycle an `Option<String>` selection through `[None, options…]` by `dir`.
fn cycle_opt(current: &Option<String>, options: &[String], dir: i32) -> Option<String> {
    let mut all: Vec<Option<String>> = vec![None];
    all.extend(options.iter().cloned().map(Some));
    let pos = all.iter().position(|x| x == current).unwrap_or(0) as i32;
    let next = (pos + dir).rem_euclid(all.len() as i32) as usize;
    all[next].clone()
}

/// Cycle the quality filter through `[All, 2160p, 1080p, 720p, 480p, 360p]`.
fn cycle_quality(current: Option<Quality>, dir: i32) -> Option<Quality> {
    let all: [Option<Quality>; 6] = [
        None,
        Some(Quality::P2160),
        Some(Quality::P1080),
        Some(Quality::P720),
        Some(Quality::P480),
        Some(Quality::P360),
    ];
    let pos = all.iter().position(|&q| q == current).unwrap_or(0) as i32;
    all[(pos + dir).rem_euclid(all.len() as i32) as usize]
}

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
            screen: Screen::Search,
            query: initial_query.unwrap_or_default(),
            search_id: 0,
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

    /// Ask the still loader to fetch the image for the selected episode. Cheap
    /// to call every frame: it only acts on the Episodes screen when images are
    /// supported, and the loader ignores URLs already loading/ready/failed.
    fn request_visible_still(&mut self) {
        if self.screen != Screen::Episodes || !self.stills.enabled() {
            return;
        }
        let Some(url) = self.current_still_url().map(str::to_string) else {
            return;
        };
        self.stills
            .request(&url, STILL_TARGET_CELLS, self.still_tx.clone());
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
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key.code, key.modifiers);
                }
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
        app.request_visible_still();

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
fn activate_control(app: &mut App) {
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

fn start_search(app: &mut App) {
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

fn search_status(results: usize, failed_providers: usize, finished: bool) -> String {
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
            still: None,
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

fn play_selected(app: &mut App) {
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
            .style(Style::new().fg(app.theme.accent).bold())
            .block(Block::default().borders(Borders::ALL)),
        chunks[0],
    );

    match app.screen {
        Screen::Search => draw_search(f, app, chunks[1]),
        Screen::Results => draw_results(f, app, chunks[1]),
        Screen::Seasons => draw_seasons(f, app, chunks[1]),
        Screen::Episodes => draw_episodes(f, app, chunks[1]),
        Screen::Sources => draw_sources_screen(f, app, chunks[1]),
    }

    // Footer / status
    let hints = match app.screen {
        Screen::Search => "type query · Enter: search · Esc: quit",
        Screen::Results => "j/k: move · Enter: select · /: search · q: quit",
        Screen::Seasons => "j/k: move · Enter: select · Esc: back · q: quit",
        Screen::Episodes => "j/k: move · Enter: select · Esc: back · q: quit",
        Screen::Sources => match app.focus {
            Focus::List => "Tab: filters · j/k: move · Enter: play · Esc: back · q: quit",
            Focus::Panel => "Tab: list · j/k: control · ←/→: change · Enter: apply · Esc: list",
        },
    };
    let spin = if app.busy { "⏳ " } else { "" };
    f.render_widget(
        Paragraph::new(format!("{spin}{}", app.status))
            .style(Style::new().fg(app.theme.status))
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
    render_list(
        f,
        &app.theme,
        area,
        "Results",
        items,
        &app.results_state,
        true,
    );
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
    render_list(
        f,
        &app.theme,
        area,
        "Seasons",
        items,
        &app.seasons_state,
        true,
    );
}

fn draw_episodes(f: &mut Frame, app: &App, area: Rect) {
    // Split off a passive detail panel when there's room; otherwise the list
    // takes the full width (narrow terminals aren't squeezed).
    let show_panel = area.width >= PANEL_MIN_WIDTH;
    let (list_area, panel_area) = if show_panel {
        let cols =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(EPISODE_PANEL_WIDTH)])
                .split(area);
        (cols[0], Some(cols[1]))
    } else {
        (area, None)
    };

    let items: Vec<ListItem> = app
        .episodes
        .iter()
        .map(|ep| ListItem::new(episode_row(ep)))
        .collect();
    render_list(
        f,
        &app.theme,
        list_area,
        "Episodes",
        items,
        &app.episodes_state,
        true,
    );

    if let Some(panel) = panel_area {
        draw_episode_panel(f, app, panel);
    }
}

/// One list line per episode: `E01 · Title`, falling back to the air date and
/// then to the bare coordinate when a provider couldn't supply a title.
fn episode_row(ep: &Episode) -> String {
    match &ep.title {
        Some(t) if !t.is_empty() => format!("E{:02} · {t}", ep.number),
        _ => match &ep.air_date {
            Some(d) if !d.is_empty() => format!("E{:02}   ({d})", ep.number),
            _ => format!("E{:02}", ep.number),
        },
    }
}

/// Passive detail panel for the highlighted episode: series context on top,
/// then the selected episode's title, air date, runtime, rating, and synopsis.
/// Follows the list cursor; it is not keyboard-focusable.
fn draw_episode_panel(f: &mut Frame, app: &App, area: Rect) {
    let sel = app.episodes_state.selected().unwrap_or(0);
    let ep = app.episodes.get(sel);

    let mut lines: Vec<Line> = Vec::new();
    let label = |s: &str| Span::styled(s.to_string(), Style::new().fg(app.theme.dim));

    // --- Series context (from the parent Media) ---
    if let Some(m) = &app.media {
        lines.push(Line::from(Span::styled(
            m.title.clone(),
            Style::new().fg(app.theme.accent).bold(),
        )));
        let mut meta: Vec<String> = Vec::new();
        if let Some(y) = m.year {
            meta.push(y.to_string());
        }
        if let Some(s) = m.score.filter(|s| *s > 0.0) {
            meta.push(format!("★ {s:.1}"));
        }
        if let (Some(sc), Some(ec)) = (m.season_count, m.episode_count) {
            meta.push(format!("{sc}S · {ec}E"));
        } else if let Some(ec) = m.episode_count {
            meta.push(format!("{ec}E"));
        }
        if !meta.is_empty() {
            lines.push(Line::from(label(&meta.join("   "))));
        }
        if !m.genres.is_empty() {
            lines.push(Line::from(label(&m.genres.join(", "))));
        }
        lines.push(Line::from(""));
    }

    // --- Selected episode ---
    match ep {
        Some(ep) => {
            let title = match &ep.title {
                Some(t) if !t.is_empty() => format!("E{:02} · {t}", ep.number),
                _ => format!("E{:02}", ep.number),
            };
            lines.push(Line::from(Span::styled(title, Style::new().bold())));

            let mut facts: Vec<String> = Vec::new();
            if let Some(d) = &ep.air_date {
                if !d.is_empty() {
                    facts.push(format!("Aired {d}"));
                }
            }
            if let Some(r) = ep.runtime_minutes {
                facts.push(format!("{r} min"));
            }
            // Treat a 0.0 rating as "unrated" — Cinemeta returns "0" for
            // episodes it has no score for, and `★ 0.0` reads as misleading.
            if let Some(rt) = ep.rating.filter(|r| *r > 0.0) {
                facts.push(format!("★ {rt:.1}"));
            }
            if !facts.is_empty() {
                lines.push(Line::from(label(&facts.join("   "))));
            }

            if let Some(o) = &ep.overview {
                if !o.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(o.clone()));
                }
            } else {
                lines.push(Line::from(""));
                lines.push(Line::from(label("No synopsis available.")));
            }
        }
        None => lines.push(Line::from(label("No episode selected."))),
    }

    // Frame the panel, then split the interior into a still image on top and
    // the text details below.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(app.theme.dim))
        .title("Episode");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let (image_area, text_area) = if app.stills.enabled() && inner.height > STILL_ROWS + 2 {
        let rows =
            Layout::vertical([Constraint::Length(STILL_ROWS), Constraint::Min(0)]).split(inner);
        (Some(rows[0]), rows[1])
    } else {
        (None, inner)
    };

    if let Some(img_area) = image_area {
        draw_still(f, app, img_area);
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), text_area);
}

/// Render the still for the selected episode into `area`: the decoded image
/// when ready, otherwise a centered status line (loading / unavailable).
fn draw_still(f: &mut Frame, app: &App, area: Rect) {
    let url = app.current_still_url();
    if let Some(protocol) = url.and_then(|u| app.stills.ready(u)) {
        // Cheap render of the already-resized+encoded protocol image.
        f.render_widget(Image::new(protocol), area);
        return;
    }

    let status = if url.is_some() {
        "  loading image…"
    } else {
        "  no image"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            status,
            Style::new().fg(app.theme.dim),
        ))),
        area,
    );
}

fn draw_sources_screen(f: &mut Frame, app: &App, area: Rect) {
    // Split off the side panel when there's room; otherwise list takes it all.
    let show_panel = area.width >= PANEL_MIN_WIDTH;
    let (list_area, panel_area) = if show_panel {
        let cols =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(PANEL_WIDTH)]).split(area);
        (cols[0], Some(cols[1]))
    } else {
        (area, None)
    };

    draw_sources_list(f, app, list_area, show_panel);
    if let Some(panel) = panel_area {
        draw_sources_panel(f, app, panel);
    }
}

fn draw_sources_list(f: &mut Frame, app: &App, area: Rect, show_panel: bool) {
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .filter_map(|&i| app.candidates.get(i))
        .map(|c| ListItem::new(source_row(c)))
        .collect();
    let focused = !show_panel || app.focus == Focus::List;
    let title = format!("Sources ({})", app.visible.len());
    render_list(
        f,
        &app.theme,
        area,
        &title,
        items,
        &app.candidates_state,
        focused,
    );
}

/// One scannable line per candidate: cache · quality · size · seeders · provider
/// · release name (first line of the raw title, the full text lives in the panel).
fn source_row(c: &SourceCandidate) -> String {
    let cache = match c.cache {
        CacheState::Cached => "⚡",
        CacheState::Uncached => "⬇",
        CacheState::Unknown => " ",
    };
    let seeders = c
        .seeders
        .map(|s| format!("{s}S"))
        .unwrap_or_else(|| "—".into());
    let name = c.title.lines().next().unwrap_or(&c.title).trim();
    format!(
        "{cache} {:<5} {:>9} {:>6}  {:<12} {}",
        c.quality.label(),
        c.human_size(),
        seeders,
        truncate(&c.provider, 12),
        name,
    )
}

fn draw_sources_panel(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(8), Constraint::Min(0)]).split(area);
    draw_filter_box(f, app, rows[0]);
    draw_details_box(f, app, rows[1]);
}

fn draw_filter_box(f: &mut Frame, app: &App, area: Rect) {
    let panel_focused = app.focus == Focus::Panel;
    let sel = app.panel_state.selected().unwrap_or(0);
    let q = app
        .filters
        .quality
        .map(|q| q.label().to_string())
        .unwrap_or_else(|| "All".into());
    let lang = app.filters.language.clone().unwrap_or_else(|| "All".into());
    let prov = app.filters.provider.clone().unwrap_or_else(|| "All".into());
    let cached = if app.filters.cached_only { "On" } else { "Off" };

    let rows = [
        format!("Sort     : {}", app.filters.sort.label()),
        format!("Quality  : {q}"),
        format!("Language : {lang}"),
        format!("Provider : {prov}"),
        format!("Cached   : {cached}"),
        "[ Clear filters ]".to_string(),
    ];
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut item = ListItem::new(r.clone());
            if panel_focused && i == sel {
                item = item.style(app.theme.highlight());
            }
            item
        })
        .collect();
    let border = app.theme.border(panel_focused);
    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(border))
                .title("Filters / Sort"),
        ),
        area,
    );
}

fn draw_details_box(f: &mut Frame, app: &App, area: Rect) {
    let text = match app.current_candidate() {
        Some(c) => {
            let t = &c.tags;
            let langs = if t.languages.is_empty() {
                "—".to_string()
            } else {
                t.languages.join(", ")
            };
            let cache = match c.cache {
                CacheState::Cached => "cached ⚡",
                CacheState::Uncached => "uncached",
                CacheState::Unknown => "unknown",
            };
            let mut lines = vec![
                c.title
                    .lines()
                    .next()
                    .unwrap_or(&c.title)
                    .trim()
                    .to_string(),
                String::new(),
                format!("Provider : {}", c.provider),
                format!("Quality  : {}", c.quality.label()),
                format!("Size     : {}", c.human_size()),
                format!(
                    "Seeders  : {}",
                    c.seeders
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "?".into())
                ),
                format!("Cache    : {cache}"),
                format!("Language : {langs}"),
            ];
            if let Some(v) = &t.video_codec {
                lines.push(format!("Video    : {v}"));
            }
            if let Some(a) = &t.audio {
                lines.push(format!("Audio    : {a}"));
            }
            if let Some(h) = &t.hdr {
                lines.push(format!("HDR      : {h}"));
            }
            if let Some(s) = &t.source_type {
                lines.push(format!("Source   : {s}"));
            }
            lines.join("\n")
        }
        None => "No candidate selected.".to_string(),
    };
    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(app.theme.dim))
                    .title("Details"),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// Truncate to `max` chars (with an ellipsis) for fixed-width columns.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn render_list(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    title: &str,
    items: Vec<ListItem>,
    state: &ListState,
    focused: bool,
) {
    let border = theme.border(focused);
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(border))
                .title(title.to_string()),
        )
        .highlight_style(theme.highlight())
        .highlight_symbol("▶ ");
    let mut s = *state;
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

#[cfg(test)]
mod tests {
    use super::*;
    use open_media_core::stream::ReleaseTags;

    fn cand(
        provider: &str,
        quality: Quality,
        size: u64,
        seeders: Option<u32>,
        cache: CacheState,
        languages: &[&str],
    ) -> SourceCandidate {
        SourceCandidate {
            provider: provider.into(),
            title: format!("{provider} release"),
            quality,
            size_bytes: size,
            seeders,
            info_hash: Some("hash".into()),
            magnet: None,
            direct_url: None,
            file_index: None,
            cache,
            tags: ReleaseTags {
                languages: languages.iter().map(|s| s.to_string()).collect(),
                ..ReleaseTags::default()
            },
        }
    }

    fn sample() -> Vec<SourceCandidate> {
        vec![
            cand(
                "1337x",
                Quality::P1080,
                2_000,
                Some(400),
                CacheState::Cached,
                &["English"],
            ),
            cand(
                "RARBG",
                Quality::P2160,
                18_000,
                Some(40),
                CacheState::Uncached,
                &["English", "Italian"],
            ),
            cand(
                "TPB",
                Quality::P720,
                800,
                Some(900),
                CacheState::Unknown,
                &["Spanish"],
            ),
        ]
    }

    fn filters() -> SourceFilters {
        SourceFilters {
            sort: SortKey::Relevance,
            quality: None,
            language: None,
            provider: None,
            cached_only: false,
        }
    }

    #[test]
    fn relevance_preserves_engine_order() {
        let c = sample();
        assert_eq!(visible_indices(&c, &filters()), vec![0, 1, 2]);
    }

    #[test]
    fn quality_filter_selects_one() {
        let c = sample();
        let mut f = filters();
        f.quality = Some(Quality::P2160);
        assert_eq!(visible_indices(&c, &f), vec![1]);
    }

    #[test]
    fn language_filter_matches_any_listed() {
        let c = sample();
        let mut f = filters();
        f.language = Some("Italian".into());
        assert_eq!(visible_indices(&c, &f), vec![1]);
        f.language = Some("English".into());
        assert_eq!(visible_indices(&c, &f), vec![0, 1]);
    }

    #[test]
    fn provider_and_cached_filters() {
        let c = sample();
        let mut f = filters();
        f.provider = Some("TPB".into());
        assert_eq!(visible_indices(&c, &f), vec![2]);
        let mut f2 = filters();
        f2.cached_only = true;
        assert_eq!(visible_indices(&c, &f2), vec![0]);
    }

    #[test]
    fn sorts_by_seeders_quality_size() {
        let c = sample();
        let mut f = filters();
        f.sort = SortKey::Seeders;
        assert_eq!(visible_indices(&c, &f), vec![2, 0, 1]); // 900, 400, 40
        f.sort = SortKey::Quality;
        assert_eq!(visible_indices(&c, &f), vec![1, 0, 2]); // 2160, 1080, 720
        f.sort = SortKey::Size;
        assert_eq!(visible_indices(&c, &f), vec![1, 0, 2]); // 18000, 2000, 800
    }

    #[test]
    fn empty_when_filters_exclude_all() {
        let c = sample();
        let mut f = filters();
        f.quality = Some(Quality::P480);
        assert!(visible_indices(&c, &f).is_empty());
    }

    #[test]
    fn quality_cycles_and_wraps() {
        assert_eq!(cycle_quality(None, 1), Some(Quality::P2160));
        assert_eq!(cycle_quality(Some(Quality::P360), 1), None); // wrap to All
        assert_eq!(cycle_quality(None, -1), Some(Quality::P360)); // wrap back
    }

    #[test]
    fn opt_cycle_includes_all_sentinel() {
        let opts = vec!["English".to_string(), "Italian".to_string()];
        assert_eq!(cycle_opt(&None, &opts, 1), Some("English".into()));
        assert_eq!(cycle_opt(&Some("Italian".into()), &opts, 1), None);
    }

    #[test]
    fn filters_roundtrip_through_config() {
        let mut ui = open_media_config::SourcesUi::default();
        let f = SourceFilters {
            sort: SortKey::Seeders,
            quality: Some(Quality::P1080),
            language: Some("English".into()),
            provider: Some("1337x".into()),
            cached_only: true,
        };
        f.write_cfg(&mut ui);
        assert_eq!(ui.sort, "seeders");
        assert_eq!(ui.quality, "1080p");
        let back = SourceFilters::from_cfg(&ui);
        assert_eq!(back.sort, SortKey::Seeders);
        assert_eq!(back.quality, Some(Quality::P1080));
        assert_eq!(back.language.as_deref(), Some("English"));
        assert!(back.cached_only);
    }

    #[test]
    fn config_all_sentinel_parses_to_none() {
        let ui = open_media_config::SourcesUi::default(); // all "all"
        let f = SourceFilters::from_cfg(&ui);
        assert_eq!(f.quality, None);
        assert_eq!(f.language, None);
        assert_eq!(f.provider, None);
        assert_eq!(f.sort, SortKey::Relevance);
    }

    #[test]
    fn search_status_distinguishes_partial_final_and_failures() {
        assert_eq!(
            search_status(12, 0, false),
            "12 results · still searching..."
        );
        assert_eq!(search_status(12, 0, true), "12 results");
        assert_eq!(
            search_status(12, 2, false),
            "12 results · still searching... · 2 providers failed"
        );
    }
}
