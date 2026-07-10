//! # open-media-app
//!
//! The **application layer**: use-cases that orchestrate the ports defined in
//! `open-media-core`. It depends *only* on `open-media-core` — it cannot name `TmdbProvider`,
//! `RealDebrid`, `MpvPlayer`, etc. Concrete adapters are injected at the
//! composition root (`open-media-cli`) as `Arc<dyn Port>`. That is the whole point of the
//! Dependency-Inversion boundary: business logic here is testable with fakes and
//! never changes when an adapter is swapped (OCP).
//!
//! ## The [`Engine`]
//! Holds the selected adapters and exposes the use-cases:
//! - [`Engine::search`] — fan out across metadata providers, merge results.
//! - [`Engine::find_sources`] — fan out across source providers, merge + rank.
//! - [`Engine::seasons`]/[`Engine::episodes`] — episodic navigation.
//! - [`Engine::play`] — the playback orchestrator (see its doc for the full
//!   sequence).
//!
//! Build one with [`EngineBuilder`].

mod builder;
mod library;
mod playback;
mod playlist;
mod search;
mod sources;

#[cfg(test)]
mod tests;

pub use builder::EngineBuilder;

use std::sync::Arc;

use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::{Episode, IdSet, Media, MediaKind, Season};
use open_media_core::ports::{
    Enricher, HistoryStore, IdBridge, LibraryStore, MetadataProvider, Player, PresenceReporter,
    SourceProvider, StreamResolver, SubtitleProvider, Tracker,
};
use open_media_core::scoring::ScoringPrefs;

/// A request to play something, resolved into coordinates the engine understands.
#[derive(Debug, Clone)]
pub struct PlayRequest {
    pub media: Media,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    /// Display title of the selected episode, when the metadata provider supplied
    /// one. Threaded into the player's media-title; `None` degrades gracefully to
    /// just the `S01E01` coordinate. Always `None` for movies.
    pub episode_title: Option<String>,
    /// Selected episode still/thumbnail URL. Presence and other activity UIs use
    /// this before falling back to the series poster. Always `None` for movies.
    pub episode_still: Option<String>,
    /// Selected episode's runtime in minutes, when the metadata provider supplied
    /// one. Forwarded (as seconds) to the [`Enricher`] so AniSkip can validate
    /// skip intervals against the episode length; `None` disables that check.
    pub episode_runtime_minutes: Option<u32>,
    pub include_uncached: bool,
}

/// Snapshot emitted by [`Engine::search_incremental`] as provider batches arrive.
#[derive(Debug, Clone)]
pub struct SearchProgress {
    /// Deduplicated results accumulated so far, in stable first-seen order.
    pub results: Vec<Media>,
    /// Number of providers that failed. Failures are non-fatal when at least one
    /// metadata provider is configured.
    pub failed_providers: usize,
    /// Names of providers that failed, in completion order.
    pub failed_provider_names: Vec<String>,
    /// `true` once every configured provider has completed or failed.
    pub finished: bool,
}

/// The composed application engine. Cheap to clone-share via `Arc` fields.
pub struct Engine {
    metadata: Vec<Arc<dyn MetadataProvider>>,
    sources: Vec<Arc<dyn SourceProvider>>,
    resolver: Option<Arc<dyn StreamResolver>>,
    player: Option<Arc<dyn Player>>,
    tracker: Option<Arc<dyn Tracker>>,
    enricher: Option<Arc<dyn Enricher>>,
    history: Option<Arc<dyn HistoryStore>>,
    library: Option<Arc<dyn LibraryStore>>,
    presence: Option<Arc<dyn PresenceReporter>>,
    subtitles: Option<Arc<dyn SubtitleProvider>>,
    /// Preferred subtitle languages (most-wanted first) for the [`SubtitleQuery`].
    /// Empty when no provider is wired or none are configured.
    ///
    /// [`SubtitleQuery`]: open_media_core::subtitle::SubtitleQuery
    subtitle_languages: Vec<String>,
    /// Bridges an anime's AniList/MAL id to an IMDB id so the IMDB-keyed source
    /// providers (Torrentio → debrid) can serve it. Optional: when unset, anime
    /// without an IMDB id simply keep their anime-native (nyaa) sources.
    id_bridge: Option<Arc<dyn IdBridge>>,
    prefs: ScoringPrefs,
    /// Fraction watched at which an episode counts as complete (e.g. 0.85).
    complete_threshold: f32,
    /// Skip filler/recap episodes when advancing (anime, via the [`Enricher`]).
    skip_filler: bool,
    /// Auto-advance to the next episode after a completed episodic playback.
    autoplay_next: bool,
    /// Run the live-playlist session for episodic playback (players with
    /// [`PlaylistControl`]) so the player's own Next button has a target, even
    /// when `autoplay_next` is off.
    playlist_next: bool,
    /// How many consecutive monitor ticks (~1s) playback may hold at
    /// end-of-file before the session is ended, in manual-Next mode
    /// (`playlist_next` without `autoplay_next`). The grace window lets a
    /// quick Next click still advance.
    eof_grace_ticks: u32,
    /// Seek to the saved position when starting playback. When `false`, history
    /// is still recorded but playback always starts from the beginning.
    resume: bool,
}

impl Engine {
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Hydrate full details (and extra ids, e.g. IMDB) for a known item by trying
    /// each metadata provider until one understands the id dialect.
    ///
    /// After a successful hydrate, kind may be refined Series/Movie → Anime when
    /// the wired [`IdBridge`] reverse-maps the title into the anime catalog
    /// (Fribb). Anime is never downgraded.
    pub async fn details(&self, ids: &IdSet) -> CoreResult<Media> {
        let mut last_err = None;
        for provider in &self.metadata {
            match provider.details(ids).await {
                // The answering provider only knows its own id dialect; fold in
                // the ids discovered during search so cross-provider ids (e.g. a
                // mal id from AniList) survive into the hydrated result.
                Ok(mut media) => {
                    media.ids.merge(ids);
                    self.refine_kind_from_bridge(&mut media).await;
                    return Ok(media);
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err
            .unwrap_or_else(|| CoreError::NotFound("no metadata provider resolved details".into())))
    }

    /// Upgrade Series/Movie → Anime when Fribb (or another [`IdBridge`]) knows
    /// this identity as anime. Never downgrades Anime. Best-effort: bridge
    /// miss/error leaves `media` unchanged.
    async fn refine_kind_from_bridge(&self, media: &mut Media) {
        if media.kind == MediaKind::Anime {
            return;
        }
        let Some(bridge) = &self.id_bridge else {
            return;
        };
        match bridge.resolve(&media.ids).await {
            Ok(Some(bridged)) => {
                tracing::debug!(
                    title = %media.title,
                    from = ?media.kind,
                    "reclassified as Anime via id bridge"
                );
                media.kind = MediaKind::Anime;
                // Fold any missing IMDB so sources light up without a second hop.
                if media.ids.imdb.is_none() {
                    if let Some(imdb) = bridged.imdb {
                        media.ids.imdb = Some(imdb);
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::debug!(error = %e, "id bridge kind refine failed");
            }
        }
    }

    /// List the seasons of an episodic item. Returns the first provider that knows
    /// the id dialect and reports any seasons; `Ok(vec![])` when none do (the
    /// caller treats that as a single flat season).
    pub async fn seasons(&self, ids: &IdSet) -> CoreResult<Vec<Season>> {
        for provider in &self.metadata {
            if let Ok(seasons) = provider.seasons(ids).await {
                if !seasons.is_empty() {
                    return Ok(seasons);
                }
            }
        }
        Ok(Vec::new())
    }

    /// List the episodes of a season. Returns the first provider that knows the id
    /// dialect and reports episodes; `Ok(vec![])` when none do.
    ///
    /// For AniList/MAL-keyed media (anime), the picked list is then best-effort
    /// **overlaid** with stills/synopses from the TMDB/IMDB-keyed providers via
    /// the [`IdBridge`]: AniList knows titles but carries no per-episode art or
    /// synopsis, while TMDB (key configured) or Cinemeta (keyless) usually do —
    /// they just can't be queried without the bridged series id and the season
    /// the entry occupies in that series' numbering.
    ///
    /// [`IdBridge`]: open_media_core::ports::IdBridge
    pub async fn episodes(&self, ids: &IdSet, season: u32) -> CoreResult<Vec<Episode>> {
        let mut eps = Vec::new();
        for provider in &self.metadata {
            if let Ok(found) = provider.episodes(ids, season).await {
                if !found.is_empty() {
                    eps = found;
                    break;
                }
            }
        }
        self.overlay_bridged_episode_details(ids, &mut eps).await;
        Ok(eps)
    }

    /// Fill missing per-episode details (still, synopsis, air date, runtime,
    /// rating, title) from a bridged TMDB/IMDB identity. Best-effort: any miss
    /// (no bridge, no mapping, providers can't serve the bridged id) leaves the
    /// list untouched. Never fails the caller.
    async fn overlay_bridged_episode_details(&self, ids: &IdSet, eps: &mut [Episode]) {
        // Only for media the TMDB/IMDB-keyed providers could NOT have served
        // directly (anime discovered via AniList/MAL), and only when something
        // is actually missing.
        if eps.is_empty() || ids.tmdb.is_some() || ids.imdb.is_some() {
            return;
        }
        if ids.anilist.is_none() && ids.mal.is_none() {
            return;
        }
        if eps
            .iter()
            .all(|e| e.still.is_some() && e.overview.is_some())
        {
            return;
        }
        let Some(bridge) = &self.id_bridge else {
            return;
        };
        let bridged = match bridge.resolve(ids).await {
            Ok(Some(b)) => b,
            Ok(None) => return,
            Err(e) => {
                tracing::debug!(error = %e, "id bridge lookup failed; keeping bare episode list");
                return;
            }
        };

        // Two attempts, in provider-registration preference order: TMDB ids
        // first (the TMDB provider is registered ahead of Cinemeta and is the
        // richer source when a key is configured), then IMDB ids (keyless
        // Cinemeta). Each uses ITS database's season/offset coordinates for
        // this entry; a negative season means absolute numbering upstream,
        // which can't be mapped per-season.
        let attempts = [
            (
                bridged
                    .tmdb_tv
                    .and_then(|id| i32::try_from(id).ok())
                    .map(|id| IdSet::default().with_tmdb(id)),
                bridged.tmdb_season,
                bridged.tmdb_episode_offset.unwrap_or(0),
            ),
            (
                bridged
                    .imdb
                    .clone()
                    .map(|id| IdSet::default().with_imdb(id)),
                bridged.imdb_season,
                bridged.imdb_episode_offset.unwrap_or(0),
            ),
        ];
        for (bridged_ids, bridged_season, offset) in attempts {
            let (Some(bridged_ids), Some(season)) = (bridged_ids, bridged_season) else {
                continue;
            };
            let Ok(season) = u32::try_from(season) else {
                continue;
            };
            let mut overlay = Vec::new();
            for provider in &self.metadata {
                if let Ok(found) = provider.episodes(&bridged_ids, season).await {
                    if !found.is_empty() {
                        overlay = found;
                        break;
                    }
                }
            }
            if overlay.is_empty() {
                continue;
            }
            let by_number: std::collections::HashMap<u32, &Episode> =
                overlay.iter().map(|e| (e.number, e)).collect();
            let mut applied = false;
            for ep in eps.iter_mut() {
                let Some(src) = by_number.get(&(ep.number + offset)) else {
                    continue;
                };
                if ep.still.is_none() {
                    ep.still = src.still.clone();
                }
                if ep.overview.is_none() {
                    ep.overview = src.overview.clone();
                }
                if ep.air_date.is_none() {
                    ep.air_date = src.air_date.clone();
                }
                if ep.runtime_minutes.is_none() {
                    ep.runtime_minutes = src.runtime_minutes;
                }
                if ep.rating.is_none() {
                    ep.rating = src.rating;
                }
                if ep.title.is_none() {
                    ep.title = src.title.clone();
                }
                applied = true;
            }
            if applied {
                tracing::debug!(season, offset, "overlaid bridged episode details");
                return;
            }
        }
    }

    /// Bridge an anime's ids: populate `media.ids.imdb` when it is missing and
    /// return the full [`BridgedIds`] for the source query.
    ///
    /// This is the hinge of anime source enrichment: the IMDB-keyed source
    /// providers (Torrentio, and through them the debrid cache) short-circuit
    /// when `ids.imdb` is `None`, so AniList anime only ever reach nyaa.
    /// Looking up the bridged identity here — right before the [`SourceQuery`]
    /// is built — lets those providers serve anime; the returned kitsu id and
    /// IMDB season additionally let Torrentio address the entry natively /
    /// at the correct season.
    ///
    /// Best-effort and side-effect-free beyond the passed `media`:
    /// - only runs for [`MediaKind::Anime`] with an anilist/mal id to bridge
    ///   from, and only when a bridge is wired;
    /// - a bridge error or a `None` result leaves `media` untouched (the title
    ///   keeps its nyaa sources). It must never fail the caller.
    ///
    /// [`SourceQuery`]: open_media_core::ports::SourceQuery
    /// [`BridgedIds`]: open_media_core::ports::BridgedIds
    async fn bridge_anime_ids(
        &self,
        media: &mut Media,
    ) -> Option<open_media_core::ports::BridgedIds> {
        if media.kind != MediaKind::Anime {
            return None;
        }
        let bridge = self.id_bridge.as_ref()?;
        match bridge.resolve(&media.ids).await {
            Ok(Some(bridged)) => {
                if media.ids.imdb.is_none() {
                    if let Some(imdb) = &bridged.imdb {
                        tracing::debug!(imdb = %imdb, "bridged anime to IMDB id for source lookup");
                        media.ids.imdb = Some(imdb.clone());
                    }
                }
                Some(bridged)
            }
            Ok(None) => {
                tracing::debug!("no id mapping for anime; keeping anime-native sources");
                None
            }
            Err(e) => {
                tracing::debug!(error = %e, "id bridge lookup failed; keeping anime-native sources");
                None
            }
        }
    }

    /// Compute the episode's *absolute* (franchise-continuous) number, when a
    /// metadata provider knows the prior-seasons offset.
    ///
    /// Only meaningful for anime with an episode coordinate: AniList numbers each
    /// season from 1, but some release groups number a sequel continuously (S2E01
    /// as `… - 21`). `offset + episode` recovers that on-disk number so a source
    /// provider can also match the absolute-numbered release. Returns `None` for
    /// movies, when no provider exposes an offset (the default), or when the
    /// offset query fails — absolute matching is a best-effort *addition*, never a
    /// reason to fail the whole lookup. The first provider returning `Some` wins.
    async fn absolute_episode(&self, req: &PlayRequest) -> Option<u32> {
        if req.media.kind != MediaKind::Anime {
            return None;
        }
        let episode = req.episode?;
        for provider in &self.metadata {
            match provider.episode_offset(&req.media.ids).await {
                Ok(Some(offset)) => return Some(offset + episode),
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!(provider = provider.name(), error = %e, "episode_offset failed")
                }
            }
        }
        None
    }
}
