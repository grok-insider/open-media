//! Ports — the trait boundaries of the hexagon.
//!
//! Every external capability open-media needs is expressed here as a small,
//! focused trait. The application layer ([`om-app`]) depends only on these
//! traits; concrete adapters (`om-metadata`, `om-sources`, `om-debrid`, ...)
//! implement them and are injected at the composition root (`om-cli`). This is
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
//! [`om-app`]: ../../om_app/index.html

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::CoreResult;
use crate::model::{Episode, IdSet, Media, MediaKind, Season};
use crate::stream::{Playback, SourceCandidate};
use crate::tracking::{Activity, ListStatus, SkipTimes, WatchProgress};

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
}

/// What a [`SourceProvider`] is being asked to find.
#[derive(Debug, Clone)]
pub struct SourceQuery {
    pub media: Media,
    /// `None` for movies; `Some` for episodic content.
    pub season: Option<u32>,
    pub episode: Option<u32>,
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
    async fn skip_times(&self, ids: &IdSet, episode: u32) -> CoreResult<SkipTimes>;

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

/// Reports a "now watching" activity to a presence service (Discord RPC).
#[async_trait]
pub trait PresenceReporter: Send + Sync {
    async fn update(&self, activity: &Activity) -> CoreResult<()>;
    async fn clear(&self) -> CoreResult<()>;
}
