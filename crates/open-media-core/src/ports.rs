//! Ports — the trait boundaries of the hexagon.
//!
//! Every external capability open-media needs is expressed here as a small,
//! focused trait. The application layer ([`open-media-app`]) depends only on these
//! traits; concrete adapters (`open-media-metadata`, `open-media-sources`,
//! `open-media-debrid`, ...) implement them and are injected at the composition
//! root (`open-media-cli`). This is
//! the Dependency-Inversion boundary: core/app never name a concrete HTTP client
//! or database.
//!
//! Design rules:
//! - **ISP**: many narrow traits, not one god interface. A debrid backend should
//!   not have to know what a tracker is.
//! - **OCP**: adding a provider = a new `impl`, never an edit to core/app.
//! - **Object-safe**: all traits are usable as `Arc<dyn Trait>` so the engine can
//!   hold heterogeneous, runtime-selected adapters.
//!
//! [`open-media-app`]: ../../open_media_app/index.html

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::CoreResult;
use crate::model::{Episode, IdSet, Media, MediaKind, Season};
use crate::stream::{Playback, SourceCandidate};
use crate::subtitle::{SubtitleQuery, SubtitleTrack};
use crate::tracking::{Activity, LibraryItem, ListStatus, SkipTimes, WatchProgress};
use crate::usage::UsageInfo;

/// Discovers and describes media (TMDB, AniList).
///
/// Responsible only for *metadata* — never sources or playback. Returns a
/// [`Media`] carrying whatever ids the backend knows; downstream ports pick the
/// id dialect they require.
#[async_trait]
pub trait MetadataProvider: Send + Sync {
    fn name(&self) -> &str;

    /// Free-text search, optionally constrained to a kind.
    async fn search(&self, query: &str, kind: Option<MediaKind>) -> CoreResult<Vec<Media>>;

    /// Hydrate full details (and additional ids) for a known item.
    async fn details(&self, ids: &IdSet) -> CoreResult<Media>;

    /// List seasons for an episodic item.
    async fn seasons(&self, ids: &IdSet) -> CoreResult<Vec<Season>>;

    /// List episodes within a season.
    async fn episodes(&self, ids: &IdSet, season: u32) -> CoreResult<Vec<Episode>>;

    /// The absolute-numbering offset for this title: the number of episodes that
    /// aired in prior, continuously-numbered seasons of the same franchise.
    ///
    /// AniList models each season as its own flat-numbered entry, but some
    /// release groups number a sequel continuously (S2E01 published as `… - 21`
    /// when S1 had 20 episodes). For those, `absolute_episode = offset + episode`
    /// recovers the on-disk number. The default is `Ok(None)` — only providers
    /// that expose a franchise relation graph (AniList) override it; the rest
    /// disable absolute matching by returning nothing.
    async fn episode_offset(&self, _ids: &IdSet) -> CoreResult<Option<u32>> {
        Ok(None)
    }
}

/// What a [`SourceProvider`] is being asked to find.
#[derive(Debug, Clone)]
pub struct SourceQuery {
    pub media: Media,
    /// `None` for movies; `Some` for episodic content.
    pub season: Option<u32>,
    pub episode: Option<u32>,
    /// The episode's *absolute* (franchise-continuous) number, when known —
    /// `offset + episode`, where the offset is the episode count of all prior
    /// seasons (see [`MetadataProvider::episode_offset`]). Lets a source provider
    /// also match a sequel release that numbers continuously (S2E01 as `… - 21`).
    /// `None` for movies, season 1, and providers without a relation graph.
    pub absolute_episode: Option<u32>,
    /// Include candidates that are *not* cached on the debrid service.
    pub include_uncached: bool,
}

/// Finds releasable files for a media item (Torrentio, direct nyaa, Comet, ...).
///
/// A provider returns *candidates*, not playable URLs — resolution is a separate
/// concern ([`StreamResolver`]). Multiple providers run concurrently and their
/// results are merged + scored by the application layer.
#[async_trait]
pub trait SourceProvider: Send + Sync {
    fn name(&self) -> &str;

    /// Whether this provider is appropriate for a media kind (e.g. a nyaa-only
    /// provider returns `false` for live-action movies).
    fn supports(&self, _kind: MediaKind) -> bool {
        true
    }

    async fn find(&self, query: &SourceQuery) -> CoreResult<Vec<SourceCandidate>>;
}

/// A torrent added to a debrid account.
#[derive(Debug, Clone)]
pub struct AddedTorrent {
    pub id: String,
    pub name: String,
    pub status: String,
}

/// A file inside a debrid torrent.
#[derive(Debug, Clone)]
pub struct DebridFile {
    pub id: String,
    pub path: String,
    pub bytes: u64,
}

/// Converts magnets/torrents into instant HTTP links (Real-Debrid, AllDebrid,
/// Torbox, Premiumize). Provider-agnostic by design — see `rdbatch`'s `Provider`
/// interface, generalized.
#[async_trait]
pub trait DebridProvider: Send + Sync {
    fn name(&self) -> &str;

    /// Human one-line account summary (e.g. "premium, expires 2026-09-01").
    async fn account_summary(&self) -> CoreResult<String>;

    /// Bulk cache check, keyed by infohash. Backends without a cache-check API
    /// may return an empty map (callers treat missing as [`CacheState::Unknown`]).
    ///
    /// [`CacheState::Unknown`]: crate::stream::CacheState::Unknown
    async fn check_cached(&self, info_hashes: &[String]) -> CoreResult<HashMap<String, bool>>;

    async fn add_magnet(&self, magnet: &str) -> CoreResult<AddedTorrent>;

    async fn list_files(&self, torrent_id: &str) -> CoreResult<Vec<DebridFile>>;

    async fn select_files(&self, torrent_id: &str, file_ids: &[String]) -> CoreResult<()>;

    /// Turn a restricted hoster link into a direct CDN URL.
    async fn unrestrict(&self, link: &str) -> CoreResult<String>;

    /// End-to-end: take a candidate and return a directly-playable [`Playback`].
    /// The canonical flow (add → poll → select → poll → unrestrict) lives in the
    /// adapter so the resolver stays backend-neutral.
    async fn resolve_playback(&self, candidate: &SourceCandidate) -> CoreResult<Playback>;
}

/// Turns a chosen [`SourceCandidate`] into a concrete [`Playback`].
///
/// This is the Strategy seam between "cached → debrid direct URL" and
/// "uncached/no-debrid → local P2P stream". Implementations compose a
/// [`DebridProvider`] and/or the librqbit engine.
#[async_trait]
pub trait StreamResolver: Send + Sync {
    async fn resolve(&self, candidate: &SourceCandidate) -> CoreResult<Playback>;

    /// Tear down any transient state (e.g. a P2P torrent) after playback.
    async fn cleanup(&self) {}
}

/// Options for launching a player.
#[derive(Debug, Clone, Default)]
pub struct PlayOptions {
    /// On-screen title override (mpv `force-media-title`).
    pub title: Option<String>,
    /// Resume position in seconds.
    pub start_at_secs: Option<u32>,
    /// Extra player args appended after configured args.
    pub extra_args: Vec<String>,
}

/// Metadata for an item appended to a live player playlist.
#[derive(Debug, Clone)]
pub struct PlaylistItem {
    pub url: String,
    pub title: Option<String>,
}

/// A chapter marker (used to expose AniSkip OP/ED segments in the player UI).
#[derive(Debug, Clone)]
pub struct Chapter {
    pub title: String,
    pub time_secs: u32,
}

/// Launches an external media player for a [`Playback`].
///
/// Launching is separate from controlling ([ISP]): a basic player (vlc) only
/// launches, while mpv additionally exposes a [`PlaybackControl`] via IPC.
#[async_trait]
pub trait Player: Send + Sync {
    fn name(&self) -> &str;

    /// Whether the player binary is on `PATH`.
    fn is_available(&self) -> bool;

    /// Spawn the player and return a session handle to await/control.
    async fn play(
        &self,
        playback: &Playback,
        opts: &PlayOptions,
    ) -> CoreResult<Box<dyn PlaySession>>;
}

/// A running player process.
#[async_trait]
pub trait PlaySession: Send {
    /// Resolve when the player exits.
    async fn wait(&mut self) -> CoreResult<()>;

    /// A control handle, if the player supports IPC (mpv). `None` for players
    /// that can only be launched (vlc), which disables resume/auto-skip for them.
    fn control(&self) -> Option<Arc<dyn PlaybackControl>>;

    /// Optional live-playlist support. Players that expose this can keep one
    /// process alive and append the next episode so the player's own Next button
    /// has a target; launch-only players keep returning `None`.
    fn playlist_control(&self) -> Option<Arc<dyn PlaylistControl>> {
        None
    }
}

/// Live control of a playing session over the player's IPC channel (mpv).
///
/// This is the universal control plane: resume (seek), progress (position),
/// presence (pause), and AniSkip (seek + chapters) all flow through it.
#[async_trait]
pub trait PlaybackControl: Send + Sync {
    async fn position(&self) -> CoreResult<Option<u32>>;
    async fn duration(&self) -> CoreResult<Option<u32>>;
    async fn is_paused(&self) -> CoreResult<Option<bool>>;
    async fn seek_absolute(&self, secs: u32) -> CoreResult<()>;
    async fn set_chapters(&self, chapters: &[Chapter]) -> CoreResult<()>;
    async fn quit(&self) -> CoreResult<()>;
}

/// Optional live playlist operations for players that support them (mpv IPC).
#[async_trait]
pub trait PlaylistControl: Send + Sync {
    /// Append an item without interrupting current playback.
    async fn append(&self, item: &PlaylistItem) -> CoreResult<()>;

    /// Zero-based active playlist index, when the player exposes it.
    async fn active_index(&self) -> CoreResult<Option<usize>>;
}

/// Syncs watch state to a remote list service (AniList, MyAnimeList).
///
/// A `Composite` implementation fans out to several trackers (curd's dual-write
/// pattern). Each tracker keys off whichever [`IdSet`] dialect it understands.
#[async_trait]
pub trait Tracker: Send + Sync {
    fn name(&self) -> &str;
    async fn update_progress(&self, ids: &IdSet, episode: u32) -> CoreResult<()>;
    async fn set_status(&self, ids: &IdSet, status: ListStatus) -> CoreResult<()>;
    async fn rate(&self, ids: &IdSet, score: f32) -> CoreResult<()>;
}

/// Augments an episode with skip windows and filler/recap flags (AniSkip, Jikan).
#[async_trait]
pub trait Enricher: Send + Sync {
    /// Opening/ending intervals for a given episode.
    ///
    /// `episode_length_secs` is the episode's runtime in **seconds** when known;
    /// AniSkip uses it to validate that returned skip intervals fall within the
    /// episode. Pass `None` when the runtime is unknown — adapters then disable
    /// that validation (AniSkip's `episodeLength=0`) rather than guessing.
    async fn skip_times(
        &self,
        ids: &IdSet,
        episode: u32,
        episode_length_secs: Option<u32>,
    ) -> CoreResult<SkipTimes>;

    /// Episode numbers that are filler/recap (so the engine can skip them).
    async fn filler_episodes(&self, ids: &IdSet) -> CoreResult<Vec<u32>>;
}

/// Persists local watch progress for resume + recent-history (SQLite).
///
/// Sync by design — the backing store (rusqlite) is synchronous and the engine
/// calls it off the hot path / via `spawn_blocking`.
pub trait HistoryStore: Send + Sync {
    fn save(&self, progress: &WatchProgress) -> CoreResult<()>;
    fn resume(
        &self,
        media_key: &str,
        season: u32,
        episode: u32,
    ) -> CoreResult<Option<WatchProgress>>;
    fn recent(&self, limit: usize) -> CoreResult<Vec<WatchProgress>>;
}

/// Persists the user's local library/watchlist.
///
/// Kept separate from [`HistoryStore`] because this is a media-level list with
/// display metadata and user status, not just per-episode resume positions.
pub trait LibraryStore: Send + Sync {
    fn upsert(&self, item: &LibraryItem) -> CoreResult<()>;
    fn list(&self, status: Option<ListStatus>) -> CoreResult<Vec<LibraryItem>>;
}

/// Finds external subtitles for a media item (OpenSubtitles, …).
///
/// Metadata-only by design: open-media plays a stream URL, not a local file, so a
/// provider searches by title + season/episode ([`SubtitleQuery`]) rather than by
/// file hash, and returns decoded [`SubtitleTrack`]s. Like the other discovery
/// ports, multiple providers can run concurrently and have their results merged.
///
/// [`SubtitleQuery`]: crate::subtitle::SubtitleQuery
/// [`SubtitleTrack`]: crate::subtitle::SubtitleTrack
#[async_trait]
pub trait SubtitleProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn fetch(&self, query: &SubtitleQuery) -> CoreResult<Vec<SubtitleTrack>>;
}

/// Bridges an anime's id dialect (AniList/MAL) to an IMDB id.
///
/// Anime is discovered via AniList, which carries no IMDB id — but the
/// IMDB-keyed source providers ([`SourceProvider`] backends like Torrentio/Comet)
/// and, through them, the debrid cache only light up when [`IdSet::imdb`] is
/// populated. This port closes that gap: given the ids known for an anime, it
/// returns the matching `tt…` id (when one exists), which the application layer
/// merges into the [`IdSet`] before building a source query — no change to the
/// source providers themselves (they already key off `imdb`).
///
/// Contract:
/// - **Best-effort and non-fatal.** A fetch/parse/cache failure must surface as
///   `Ok(None)` (no enrichment), never an `Err` that would abort the user's
///   action. The whole capability is an *addition*: without it, anime simply
///   keeps getting its anime-native (nyaa) sources as before.
/// - **Partial coverage is expected.** The backing dataset carries an IMDB id
///   mainly for standalone anime *movies* and a subset of series; many TV
///   entries have none. Returning `None` for those is correct, not an error.
/// - Object-safe and optional — the [`Engine`] works without an `IdBridge` wired.
///
/// [`IdSet::imdb`]: crate::model::IdSet::imdb
/// [`Engine`]: ../../open_media_app/struct.Engine.html
#[async_trait]
pub trait IdBridge: Send + Sync {
    fn name(&self) -> &str;

    /// Resolve the cross-database ids for the given ids (keyed off the
    /// anilist/mal dialect), or `Ok(None)` when no mapping exists or the lookup
    /// could not be performed.
    async fn resolve(&self, ids: &IdSet) -> CoreResult<Option<BridgedIds>>;

    /// Convenience: just the IMDB id from [`IdBridge::resolve`].
    async fn imdb_for(&self, ids: &IdSet) -> CoreResult<Option<String>> {
        Ok(self.resolve(ids).await?.and_then(|b| b.imdb))
    }
}

/// The cross-database ids one anime entry bridges to.
///
/// AniList numbers every season as its own entry starting at episode 1, while
/// IMDB/TVDB and TMDB number seasons within one series id. `imdb`/`tmdb_tv`
/// are therefore **series-level** ids, and the `*_season`/`*_episode_offset`
/// fields say where this entry lands inside them: entry episode `n` maps to
/// season `*_season`, episode `n + *_episode_offset`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgedIds {
    pub imdb: Option<String>,
    /// TMDB series id (episodic entries).
    pub tmdb_tv: Option<u64>,
    /// TMDB movie id (film entries).
    pub tmdb_movie: Option<u64>,
    /// Kitsu id — per-entry like AniList (episode numbering already aligns).
    pub kitsu: Option<u64>,
    /// Season within the IMDB/TVDB-numbered series. `0` = specials; a negative
    /// value means the upstream dataset uses absolute numbering for the entry.
    pub imdb_season: Option<i32>,
    /// Season within the TMDB-numbered series (same semantics).
    pub tmdb_season: Option<i32>,
    pub imdb_episode_offset: Option<u32>,
    pub tmdb_episode_offset: Option<u32>,
}

/// Reports a "now watching" activity to a presence service (Discord RPC).
#[async_trait]
pub trait PresenceReporter: Send + Sync {
    async fn update(&self, activity: &Activity) -> CoreResult<()>;
    async fn clear(&self) -> CoreResult<()>;
}

/// Emits an anonymous [`UsageInfo`] snapshot for active-install analytics.
///
/// Contract: this is **best-effort and non-identifying**. Implementations must
/// never block or fail the caller (a dead endpoint is a no-op, not an error), and
/// must transmit only the fields in [`UsageInfo`] — never anything about what the
/// user watches. The reporter is wired only when the user has telemetry enabled.
#[async_trait]
pub trait UsageReporter: Send + Sync {
    async fn report(&self, info: &UsageInfo) -> CoreResult<()>;
}
