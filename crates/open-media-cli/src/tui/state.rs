//! Screen/focus enums, the theme palette, and the Sources filter/sort state
//! (with its pure filtering/sorting/cycling helpers).

use open_media_core::stream::{CacheState, Quality, SourceCandidate};
use open_media_core::tracking::ListStatus;
use ratatui::prelude::*;

/// Semantic colors the TUI draws with. Replaces scattered `Color::*` literals
/// so the palette can be swapped wholesale by `ui.theme`. Two presets exist:
/// [`Theme::dark`] (the historical hardcoded look) and [`Theme::light`].
#[derive(Clone, Copy)]
pub(super) struct Theme {
    /// Headings, focused borders, primary highlights.
    pub(super) accent: Color,
    /// Footer/status body text.
    pub(super) status: Color,
    /// Secondary/label text and unfocused borders.
    pub(super) dim: Color,
    /// Selection / highlight background.
    pub(super) selection_bg: Color,
    /// Selection / highlight foreground.
    pub(super) selection_fg: Color,
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
    pub(super) fn from_cfg(theme: &str) -> Self {
        match theme.trim().to_ascii_lowercase().as_str() {
            "light" => Self::light(),
            _ => Self::dark(),
        }
    }

    /// Border color for a pane given its focus state.
    pub(super) fn border(&self, focused: bool) -> Color {
        if focused {
            self.accent
        } else {
            self.dim
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

/// Which screen is active.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Screen {
    Home,
    Library,
    Search,
    Results,
    Seasons,
    Episodes,
    Sources,
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
