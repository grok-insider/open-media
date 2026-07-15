//! Domain model: identity, media, seasons, and episodes.
//!
//! These types are the lingua franca between ports. A [`MetadataProvider`] emits
//! [`Media`]; a [`SourceProvider`] consumes a [`MediaId`] + episode coordinates;
//! a [`Tracker`] keys off whichever [`MediaId`] it understands. Nothing here
//! touches the network or disk.
//!
//! [`MetadataProvider`]: crate::ports::MetadataProvider
//! [`SourceProvider`]: crate::ports::SourceProvider
//! [`Tracker`]: crate::ports::Tracker

use serde::{Deserialize, Serialize};

/// What kind of content a [`Media`] is.
///
/// `Anime` is intentionally distinct from `Series`: it routes to anime-aware
/// adapters (AniList metadata/tracking, AniSkip intro/outro, nyaa-heavy source
/// providers) even though structurally it is a seasoned/episodic show. Treating
/// it as a first-class kind keeps that routing explicit instead of guessing from
/// genres at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaKind {
    Movie,
    Series,
    Anime,
}

impl MediaKind {
    /// Short human label for list rendering.
    pub fn label(self) -> &'static str {
        match self {
            MediaKind::Movie => "Movie",
            MediaKind::Series => "Series",
            MediaKind::Anime => "Anime",
        }
    }

    /// Whether this kind is episodic (has seasons/episodes).
    pub fn is_episodic(self) -> bool {
        matches!(self, MediaKind::Series | MediaKind::Anime)
    }
}

/// Curated browse catalogs exposed by metadata providers that support them.
///
/// Used by Home → Airing / This Season. Providers that do not implement a given
/// kind return an empty list from [`MetadataProvider::catalog`].
///
/// [`MetadataProvider::catalog`]: crate::ports::MetadataProvider::catalog
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CatalogKind {
    /// Anime currently airing (`RELEASING` on AniList).
    AiringAnime,
    /// Anime of the current broadcast season (Winter/Spring/Summer/Fall).
    SeasonalAnime,
}

impl CatalogKind {
    /// Short label for list/UI rendering.
    pub fn label(self) -> &'static str {
        match self {
            CatalogKind::AiringAnime => "Airing",
            CatalogKind::SeasonalAnime => "This Season",
        }
    }
}

/// A single external identifier for a piece of media.
///
/// Different ports speak different ID dialects: Torrentio/Comet key off IMDB,
/// AniSkip off MAL, AniList off its own numeric id, TMDB off its own. A [`Media`]
/// therefore carries an [`IdSet`] rather than one id, and adapters select the
/// dialect they need (or fail loudly if absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaId {
    Tmdb(i32),
    Imdb(String),
    AniList(i32),
    Mal(i32),
}

impl MediaId {
    /// A stable `scheme:value` string, useful as a history/cache key.
    pub fn as_key(&self) -> String {
        match self {
            MediaId::Tmdb(v) => format!("tmdb:{v}"),
            MediaId::Imdb(v) => format!("imdb:{v}"),
            MediaId::AniList(v) => format!("anilist:{v}"),
            MediaId::Mal(v) => format!("mal:{v}"),
        }
    }
}

/// The set of identifiers known for a media item.
///
/// Built incrementally: a TMDB search yields `tmdb` + often `imdb`; an
/// AniList lookup adds `anilist` + `mal`. Adapters call the typed accessors and
/// return [`CoreError::NotFound`] when their required dialect is missing.
///
/// [`CoreError::NotFound`]: crate::error::CoreError::NotFound
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdSet {
    pub tmdb: Option<i32>,
    pub imdb: Option<String>,
    pub anilist: Option<i32>,
    pub mal: Option<i32>,
}

impl IdSet {
    pub fn with_tmdb(mut self, id: i32) -> Self {
        self.tmdb = Some(id);
        self
    }
    pub fn with_imdb(mut self, id: impl Into<String>) -> Self {
        self.imdb = Some(id.into());
        self
    }
    pub fn with_anilist(mut self, id: i32) -> Self {
        self.anilist = Some(id);
        self
    }
    pub fn with_mal(mut self, id: i32) -> Self {
        self.mal = Some(id);
        self
    }

    /// Merge another set in, preferring already-present values.
    pub fn merge(&mut self, other: &IdSet) {
        self.tmdb = self.tmdb.or(other.tmdb);
        self.imdb = self.imdb.clone().or_else(|| other.imdb.clone());
        self.anilist = self.anilist.or(other.anilist);
        self.mal = self.mal.or(other.mal);
    }

    /// The preferred stable key for history/resume (imdb > tmdb > anilist > mal).
    pub fn primary_key(&self) -> Option<String> {
        if let Some(v) = &self.imdb {
            Some(format!("imdb:{v}"))
        } else if let Some(v) = self.tmdb {
            Some(format!("tmdb:{v}"))
        } else if let Some(v) = self.anilist {
            Some(format!("anilist:{v}"))
        } else {
            self.mal.map(|v| format!("mal:{v}"))
        }
    }
}

/// A unified media item produced by a [`MetadataProvider`].
///
/// [`MetadataProvider`]: crate::ports::MetadataProvider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Media {
    pub kind: MediaKind,
    pub ids: IdSet,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    /// Aggregate score on a 0.0–10.0 scale, if known.
    pub score: Option<f32>,
    /// **Plain text** synopsis (newlines allowed). Providers whose APIs return
    /// markup (AniList emits inline HTML) must strip/decode it at the adapter
    /// boundary — renderers display this verbatim.
    pub overview: Option<String>,
    /// Poster/cover image URL (for terminals with image protocols).
    pub poster: Option<String>,
    pub genres: Vec<String>,
    /// Release/airing status, e.g. "Released", "Airing", "Finished".
    pub status: Option<String>,
    /// Total episode count, when known (episodic kinds).
    pub episode_count: Option<u32>,
    /// Total season count, when known (episodic kinds).
    pub season_count: Option<u32>,
}

impl Media {
    /// Best display title (falls back to original title, then "Untitled").
    pub fn display_title(&self) -> &str {
        if !self.title.is_empty() {
            &self.title
        } else if let Some(o) = &self.original_title {
            o
        } else {
            "Untitled"
        }
    }
}

/// A season of an episodic [`Media`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Season {
    pub number: u32,
    pub episode_count: u32,
    pub name: Option<String>,
}

/// A single episode. `season` is `1` for anime/movies that are flat-numbered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub season: u32,
    pub number: u32,
    pub title: Option<String>,
    /// ISO date string `YYYY-MM-DD`, if known.
    pub air_date: Option<String>,
    /// **Plain text** synopsis — same contract as [`Media::overview`].
    pub overview: Option<String>,
    pub runtime_minutes: Option<u32>,
    pub rating: Option<f32>,
    /// Episode still/thumbnail image URL (for terminals with image protocols).
    pub still: Option<String>,
}

impl Episode {
    /// The `S{season}E{number}` coordinate Torrentio/Comet use for series.
    pub fn coordinate(&self) -> String {
        format!("S{:02}E{:02}", self.season, self.number)
    }
}
