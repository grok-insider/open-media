//! Screen/focus enums, navigation stack, the theme palette, and the Sources
//! filter/sort state (with its pure filtering/sorting/cycling helpers).

use open_media_core::stream::{CacheState, Quality, SourceCandidate};
use open_media_core::tracking::ListStatus;
use ratatui::prelude::*;

/// Semantic colors the TUI draws with. Replaces scattered `Color::*` literals
/// so the palette can be swapped wholesale by `ui.theme`.
#[derive(Clone, Copy)]
pub(super) struct Theme {
    /// Full-frame background.
    pub(super) bg: Color,
    /// Subtle surface for panels / cards.
    pub(super) surface: Color,
    /// Headings, focused borders, primary highlights.
    pub(super) accent: Color,
    /// Primary body text.
    pub(super) text: Color,
    /// Footer/status body text.
    pub(super) status: Color,
    /// Secondary/label text.
    pub(super) dim: Color,
    /// Muted / placeholder text.
    pub(super) muted: Color,
    /// Unfocused borders.
    pub(super) border: Color,
    /// Focused borders (usually accent).
    pub(super) border_focus: Color,
    /// Selection / highlight background.
    pub(super) selection_bg: Color,
    /// Selection / highlight foreground.
    pub(super) selection_fg: Color,
    /// Healthy / cached / success.
    pub(super) success: Color,
    /// Medium seed health / warning.
    pub(super) warn: Color,
    /// Low seed health / danger.
    pub(super) danger: Color,
    /// Movie kind badge.
    pub(super) badge_movie: Color,
    /// Series/TV kind badge.
    pub(super) badge_series: Color,
    /// Anime kind badge.
    pub(super) badge_anime: Color,
}

impl Theme {
    /// Media-app charcoal with a restrained cyan accent.
    fn dark() -> Self {
        Self {
            bg: Color::Rgb(20, 20, 22),
            surface: Color::Rgb(28, 28, 32),
            accent: Color::Rgb(64, 180, 180),
            text: Color::Rgb(225, 225, 228),
            status: Color::Rgb(160, 160, 168),
            dim: Color::Rgb(120, 120, 128),
            muted: Color::Rgb(90, 90, 98),
            border: Color::Rgb(60, 60, 68),
            border_focus: Color::Rgb(64, 180, 180),
            selection_bg: Color::Rgb(45, 58, 62),
            selection_fg: Color::Rgb(240, 240, 242),
            success: Color::Rgb(120, 190, 120),
            warn: Color::Rgb(210, 180, 90),
            danger: Color::Rgb(200, 110, 110),
            badge_movie: Color::Rgb(120, 150, 200),
            badge_series: Color::Rgb(150, 140, 200),
            badge_anime: Color::Rgb(200, 140, 160),
        }
    }

    /// Light palette: deeper accent, darker secondary text.
    fn light() -> Self {
        Self {
            bg: Color::Rgb(248, 248, 250),
            surface: Color::Rgb(255, 255, 255),
            accent: Color::Rgb(30, 100, 180),
            text: Color::Rgb(28, 28, 32),
            status: Color::Rgb(70, 70, 78),
            dim: Color::Rgb(100, 100, 110),
            muted: Color::Rgb(140, 140, 150),
            border: Color::Rgb(180, 180, 190),
            border_focus: Color::Rgb(30, 100, 180),
            selection_bg: Color::Rgb(210, 225, 240),
            selection_fg: Color::Rgb(20, 20, 24),
            success: Color::Rgb(40, 130, 70),
            warn: Color::Rgb(160, 120, 20),
            danger: Color::Rgb(170, 50, 50),
            badge_movie: Color::Rgb(50, 90, 150),
            badge_series: Color::Rgb(90, 70, 150),
            badge_anime: Color::Rgb(150, 60, 90),
        }
    }

    /// Pick a preset from `cfg.ui.theme` (`"light"` → light; `"dark"`/`"auto"`/
    /// anything else → dark). `"auto"` maps to dark for now; true
    /// terminal-background detection is a follow-up.
    pub(super) fn from_cfg(theme: &str) -> Self {
        match theme.trim().to_ascii_lowercase().as_str() {
            "light" => Self::light(),
            _ => Self::dark(),
        }
    }

    /// Border color for a pane given its focus state.
    pub(super) fn border(&self, focused: bool) -> Color {
        if focused {
            self.border_focus
        } else {
            self.border
        }
    }

    /// The shared list highlight style (selection band).
    pub(super) fn highlight(&self) -> Style {
        Style::new()
            .bg(self.selection_bg)
            .fg(self.selection_fg)
            .bold()
    }
}

/// Top-level destinations shown as tabs. Drill-down screens live on the stack.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Root {
    Home,
    Library,
    Search,
}

impl Root {
    pub(super) const ALL: [Root; 3] = [Root::Home, Root::Library, Root::Search];

    pub(super) fn label(self) -> &'static str {
        match self {
            Root::Home => "Home",
            Root::Library => "Library",
            Root::Search => "Search",
        }
    }

    pub(super) fn short(self) -> &'static str {
        match self {
            Root::Home => "H",
            Root::Library => "L",
            Root::Search => "S",
        }
    }

    pub(super) fn as_screen(self) -> Screen {
        match self {
            Root::Home => Screen::Home,
            Root::Library => Screen::Library,
            Root::Search => Screen::Search,
        }
    }

    pub(super) fn from_digit(c: char) -> Option<Root> {
        match c {
            '1' => Some(Root::Home),
            '2' => Some(Root::Library),
            '3' => Some(Root::Search),
            _ => None,
        }
    }
}

/// Which screen is active (root or drill-down).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Screen {
    Home,
    Library,
    Search,
    Results,
    Seasons,
    Episodes,
    Sources,
}

impl Screen {
    pub(super) fn is_root(self) -> bool {
        matches!(self, Screen::Home | Screen::Library | Screen::Search)
    }

    pub(super) fn is_drill(self) -> bool {
        !self.is_root()
    }
}

/// Root tab + drill-down stack. Esc pops; empty stack stays on the root.
#[derive(Clone, Debug)]
pub(super) struct Nav {
    pub(super) root: Root,
    stack: Vec<Screen>,
}

impl Nav {
    pub(super) fn new(root: Root) -> Self {
        Self {
            root,
            stack: Vec::new(),
        }
    }

    pub(super) fn current(&self) -> Screen {
        self.stack
            .last()
            .copied()
            .unwrap_or_else(|| self.root.as_screen())
    }

    pub(super) fn is_at_root(&self) -> bool {
        self.stack.is_empty()
    }

    /// Jump to a top-level tab and clear any drill-down.
    pub(super) fn go_root(&mut self, root: Root) {
        self.root = root;
        self.stack.clear();
    }

    /// Push a drill-down screen (Results / Seasons / Episodes / Sources).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn push(&mut self, screen: Screen) {
        debug_assert!(screen.is_drill());
        self.stack.push(screen);
    }

    /// Replace the entire drill stack (last entry is current).
    pub(super) fn set_stack(&mut self, screens: impl IntoIterator<Item = Screen>) {
        self.stack = screens.into_iter().filter(|s| s.is_drill()).collect();
    }

    /// Pop one level. Returns `true` if something was popped.
    pub(super) fn pop(&mut self) -> bool {
        self.stack.pop().is_some()
    }
}

/// Rows on the Home screen (continue-watching items + quick actions).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum HomeRow {
    /// Non-selectable section label (e.g. "Anime · 2").
    Section(String),
    /// Index into the continue-watching list.
    Continue(usize),
    /// Browse AniList airing anime catalog.
    CatalogAiring,
    /// Browse AniList current broadcast season catalog.
    CatalogSeasonal,
    OpenLibrary,
    Search,
}

impl HomeRow {
    pub(super) fn is_selectable(&self) -> bool {
        !matches!(self, HomeRow::Section(_))
    }
}

/// Pure home-row layout: kind sections (recent-first groups) + actions.
///
/// `continue_watching` must already be sorted newest-first.
pub(super) fn build_home_rows(
    continue_watching: &[open_media_core::tracking::LibraryItem],
) -> Vec<HomeRow> {
    use open_media_core::model::MediaKind;

    // Partition indices while preserving newest-first order within each kind.
    let mut anime = Vec::new();
    let mut series = Vec::new();
    let mut movies = Vec::new();
    for (i, item) in continue_watching.iter().enumerate() {
        match item.kind {
            MediaKind::Anime => anime.push(i),
            MediaKind::Series => series.push(i),
            MediaKind::Movie => movies.push(i),
        }
    }

    // Group order: by most recent item in the group (first index after global sort).
    let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
    if !anime.is_empty() {
        groups.push(("Anime", anime));
    }
    if !series.is_empty() {
        // Match the kind badge label ("Tv") for consistency.
        groups.push(("Tv", series));
    }
    if !movies.is_empty() {
        groups.push(("Movies", movies));
    }
    groups.sort_by_key(|(_, idxs)| {
        std::cmp::Reverse(
            idxs.first()
                .map(|&i| continue_watching[i].updated_at)
                .unwrap_or(0),
        )
    });

    let mut rows = Vec::new();
    // Discover catalogs first — Amatsu-style airing / seasonal browse.
    rows.push(HomeRow::Section("Discover".into()));
    rows.push(HomeRow::CatalogAiring);
    rows.push(HomeRow::CatalogSeasonal);

    for (label, idxs) in groups {
        rows.push(HomeRow::Section(format!("{label} · {}", idxs.len())));
        for i in idxs {
            rows.push(HomeRow::Continue(i));
        }
    }
    if !continue_watching.is_empty() {
        rows.push(HomeRow::Section("Actions".into()));
    }
    rows.push(HomeRow::OpenLibrary);
    rows.push(HomeRow::Search);
    rows
}

/// Which pane has keyboard focus on the Sources screen.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Focus {
    List,
    Panel,
}

/// How the visible candidates are ordered.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum SortKey {
    Relevance,
    Seeders,
    Quality,
    Size,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum LibraryFilter {
    All,
    Watching,
    Planned,
    Completed,
    Dropped,
}

impl LibraryFilter {
    const ALL: [LibraryFilter; 5] = [
        LibraryFilter::All,
        LibraryFilter::Watching,
        LibraryFilter::Planned,
        LibraryFilter::Completed,
        LibraryFilter::Dropped,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            LibraryFilter::All => "All",
            LibraryFilter::Watching => "Watching",
            LibraryFilter::Planned => "Planned",
            LibraryFilter::Completed => "Completed",
            LibraryFilter::Dropped => "Dropped",
        }
    }

    pub(super) fn status(self) -> Option<ListStatus> {
        match self {
            LibraryFilter::All => None,
            LibraryFilter::Watching => Some(ListStatus::Watching),
            LibraryFilter::Planned => Some(ListStatus::Planning),
            LibraryFilter::Completed => Some(ListStatus::Completed),
            LibraryFilter::Dropped => Some(ListStatus::Dropped),
        }
    }

    pub(super) fn cycle(self, dir: i32) -> Self {
        let pos = Self::ALL.iter().position(|&f| f == self).unwrap_or(0) as i32;
        Self::ALL[(pos + dir).rem_euclid(Self::ALL.len() as i32) as usize]
    }
}

impl SortKey {
    pub(super) fn label(self) -> &'static str {
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
    pub(super) fn cycle(self, dir: i32) -> SortKey {
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
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum PanelControl {
    Sort,
    Quality,
    Language,
    Provider,
    Cached,
    Clear,
}

impl PanelControl {
    pub(super) const ALL: [PanelControl; 6] = [
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
pub(super) struct SourceFilters {
    pub(super) sort: SortKey,
    pub(super) quality: Option<Quality>,
    pub(super) language: Option<String>,
    pub(super) provider: Option<String>,
    pub(super) cached_only: bool,
}

impl SourceFilters {
    /// Seed from persisted config.
    pub(super) fn from_cfg(s: &open_media_config::SourcesUi) -> Self {
        Self {
            sort: SortKey::from_str(&s.sort),
            quality: parse_quality(&s.quality),
            language: parse_all_opt(&s.language),
            provider: parse_all_opt(&s.provider),
            cached_only: s.cached_only,
        }
    }

    /// Write back into config for persistence.
    pub(super) fn write_cfg(&self, s: &mut open_media_config::SourcesUi) {
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

    pub(super) fn clear(&mut self) {
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
pub(super) fn visible_indices(candidates: &[SourceCandidate], f: &SourceFilters) -> Vec<usize> {
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
pub(super) fn distinct_languages(candidates: &[SourceCandidate]) -> Vec<String> {
    let mut v: Vec<String> = candidates
        .iter()
        .flat_map(|c| c.tags.languages.iter().cloned())
        .collect();
    v.sort();
    v.dedup();
    v
}

pub(super) fn distinct_providers(candidates: &[SourceCandidate]) -> Vec<String> {
    let mut v: Vec<String> = candidates.iter().map(|c| c.provider.clone()).collect();
    v.sort();
    v.dedup();
    v
}

/// Cycle an `Option<String>` selection through `[None, options…]` by `dir`.
pub(super) fn cycle_opt(current: &Option<String>, options: &[String], dir: i32) -> Option<String> {
    let mut all: Vec<Option<String>> = vec![None];
    all.extend(options.iter().cloned().map(Some));
    let pos = all.iter().position(|x| x == current).unwrap_or(0) as i32;
    let next = (pos + dir).rem_euclid(all.len() as i32) as usize;
    all[next].clone()
}

/// Cycle the quality filter through `[All, 2160p, 1080p, 720p, 480p, 360p]`.
pub(super) fn cycle_quality(current: Option<Quality>, dir: i32) -> Option<Quality> {
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

#[cfg(test)]
mod nav_tests {
    use super::*;

    #[test]
    fn nav_starts_at_root() {
        let nav = Nav::new(Root::Home);
        assert!(nav.is_at_root());
        assert_eq!(nav.current(), Screen::Home);
    }

    #[test]
    fn nav_push_pop_restores_root() {
        let mut nav = Nav::new(Root::Search);
        nav.push(Screen::Results);
        nav.push(Screen::Sources);
        assert_eq!(nav.current(), Screen::Sources);
        assert!(nav.pop());
        assert_eq!(nav.current(), Screen::Results);
        assert!(nav.pop());
        assert!(nav.is_at_root());
        assert_eq!(nav.current(), Screen::Search);
        assert!(!nav.pop());
    }

    #[test]
    fn go_root_clears_stack() {
        let mut nav = Nav::new(Root::Home);
        nav.push(Screen::Results);
        nav.go_root(Root::Library);
        assert!(nav.is_at_root());
        assert_eq!(nav.current(), Screen::Library);
        assert_eq!(nav.root, Root::Library);
    }
}
