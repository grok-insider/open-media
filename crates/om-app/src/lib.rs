//! # om-app
//!
//! The **application layer**: use-cases that orchestrate the ports defined in
//! `om-core`. It depends *only* on `om-core` — it cannot name `TmdbProvider`,
//! `RealDebrid`, `MpvPlayer`, etc. Concrete adapters are injected at the
//! composition root (`om-cli`) as `Arc<dyn Port>`. That is the whole point of the
//! Dependency-Inversion boundary: business logic here is testable with fakes and
//! never changes when an adapter is swapped (OCP).
//!
//! ## The [`Engine`]
//! Holds the selected adapters and exposes the use-cases:
//! - [`Engine::search`] — fan out across metadata providers, merge results.
//! - [`Engine::find_sources`] — fan out across source providers, merge + rank.
//! - [`Engine::play`] — the playback orchestrator (see its doc for the full
//!   sequence). Scaffolded; lands in Phase 8.
//!
//! Build one with [`EngineBuilder`].

use std::sync::Arc;

use om_core::error::{CoreError, CoreResult};
use om_core::model::{IdSet, Media, MediaKind};
use om_core::ports::{
    Enricher, HistoryStore, MetadataProvider, Player, PresenceReporter, SourceProvider,
    SourceQuery, StreamResolver, Tracker,
};
use om_core::scoring::{self, ScoringPrefs};
use om_core::stream::{Playback, SourceCandidate};

/// A request to play something, resolved into coordinates the engine understands.
#[derive(Debug, Clone)]
pub struct PlayRequest {
    pub media: Media,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub include_uncached: bool,
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
    presence: Option<Arc<dyn PresenceReporter>>,
    prefs: ScoringPrefs,
}

impl Engine {
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Search every configured metadata provider concurrently and merge results.
    ///
    /// Provider failures are logged and skipped, not fatal — a TMDB outage should
    /// not stop AniList from returning anime.
    pub async fn search(&self, query: &str, kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        if self.metadata.is_empty() {
            return Err(CoreError::Config("no metadata providers configured".into()));
        }
        let calls = self
            .metadata
            .iter()
            .map(|provider| async move { (provider.name(), provider.search(query, kind).await) });
        let mut out = Vec::new();
        for (name, result) in futures::future::join_all(calls).await {
            match result {
                Ok(mut items) => out.append(&mut items),
                Err(e) => tracing::warn!(provider = name, error = %e, "metadata search failed"),
            }
        }
        Ok(out)
    }

    /// Hydrate full details (and extra ids, e.g. IMDB) for a known item by trying
    /// each metadata provider until one understands the id dialect.
    pub async fn details(&self, ids: &IdSet) -> CoreResult<Media> {
        let mut last_err = None;
        for provider in &self.metadata {
            match provider.details(ids).await {
                Ok(media) => return Ok(media),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err
            .unwrap_or_else(|| CoreError::NotFound("no metadata provider resolved details".into())))
    }

    /// Find playable candidates across every applicable source provider
    /// concurrently, merge, and rank them with [`scoring`]. Providers that do not
    /// support the media kind (e.g. nyaa for a live-action movie) are skipped.
    pub async fn find_sources(&self, req: &PlayRequest) -> CoreResult<Vec<SourceCandidate>> {
        let query = SourceQuery {
            media: req.media.clone(),
            season: req.season,
            episode: req.episode,
            include_uncached: req.include_uncached,
        };

        let calls = self
            .sources
            .iter()
            .filter(|s| s.supports(req.media.kind))
            .map(|source| {
                let query = &query;
                async move { (source.name(), source.find(query).await) }
            });

        let mut candidates = Vec::new();
        for (name, result) in futures::future::join_all(calls).await {
            match result {
                Ok(mut found) => candidates.append(&mut found),
                Err(e) => tracing::warn!(source = name, error = %e, "source lookup failed"),
            }
        }

        scoring::rank(&mut candidates, &self.prefs);
        Ok(candidates)
    }

    /// Resolve a chosen candidate into a player-openable [`Playback`].
    pub async fn resolve(&self, candidate: &SourceCandidate) -> CoreResult<Playback> {
        let resolver = self
            .resolver
            .as_ref()
            .ok_or_else(|| CoreError::Config("no stream resolver configured".into()))?;
        resolver.resolve(candidate).await
    }

    /// Resolve a chosen candidate and play it end-to-end.
    ///
    /// The full sequence (Phase 8):
    /// 1. [`StreamResolver::resolve`] → a [`Playback`] URL (debrid-direct or P2P).
    /// 2. [`Player::play`] → spawn mpv with `force-media-title` + resume position.
    /// 3. If the player exposes [`PlaybackControl`]: start the concurrent tasks —
    ///    - **resume**: seek to the saved position once playback starts,
    ///    - **skip**: poll `time-pos`; when inside an [`Enricher`] OP/ED window,
    ///      `seek_absolute` past it,
    ///    - **progress/presence**: poll position → persist to [`HistoryStore`],
    ///      push [`PresenceReporter`] updates,
    ///    - on ≥ complete-threshold: fire [`Tracker::update_progress`].
    /// 4. On player exit: persist final position, run [`StreamResolver::cleanup`],
    ///    and (for binge mode) advance to the next non-filler episode.
    ///
    /// [`Playback`]: om_core::stream::Playback
    /// [`PlaybackControl`]: om_core::ports::PlaybackControl
    pub async fn play(&self, _req: &PlayRequest, _candidate: &SourceCandidate) -> CoreResult<()> {
        // Touch the optional ports so the field wiring is exercised by the type
        // checker even while the orchestration is stubbed.
        let _ = (
            self.resolver.as_ref(),
            self.player.as_ref(),
            self.tracker.as_ref(),
            self.enricher.as_ref(),
            self.history.as_ref(),
            self.presence.as_ref(),
        );
        Err(CoreError::NotImplemented("engine.play"))
    }
}

/// Builder for [`Engine`]. The composition root adds whichever adapters the
/// user's config selects; unset capabilities simply disable their features.
#[derive(Default)]
pub struct EngineBuilder {
    metadata: Vec<Arc<dyn MetadataProvider>>,
    sources: Vec<Arc<dyn SourceProvider>>,
    resolver: Option<Arc<dyn StreamResolver>>,
    player: Option<Arc<dyn Player>>,
    tracker: Option<Arc<dyn Tracker>>,
    enricher: Option<Arc<dyn Enricher>>,
    history: Option<Arc<dyn HistoryStore>>,
    presence: Option<Arc<dyn PresenceReporter>>,
    prefs: ScoringPrefs,
}

impl EngineBuilder {
    pub fn add_metadata(mut self, p: Arc<dyn MetadataProvider>) -> Self {
        self.metadata.push(p);
        self
    }
    pub fn add_source(mut self, p: Arc<dyn SourceProvider>) -> Self {
        self.sources.push(p);
        self
    }
    pub fn resolver(mut self, r: Arc<dyn StreamResolver>) -> Self {
        self.resolver = Some(r);
        self
    }
    pub fn player(mut self, p: Arc<dyn Player>) -> Self {
        self.player = Some(p);
        self
    }
    pub fn tracker(mut self, t: Arc<dyn Tracker>) -> Self {
        self.tracker = Some(t);
        self
    }
    pub fn enricher(mut self, e: Arc<dyn Enricher>) -> Self {
        self.enricher = Some(e);
        self
    }
    pub fn history(mut self, h: Arc<dyn HistoryStore>) -> Self {
        self.history = Some(h);
        self
    }
    pub fn presence(mut self, p: Arc<dyn PresenceReporter>) -> Self {
        self.presence = Some(p);
        self
    }
    pub fn scoring_prefs(mut self, prefs: ScoringPrefs) -> Self {
        self.prefs = prefs;
        self
    }

    pub fn build(self) -> Engine {
        Engine {
            metadata: self.metadata,
            sources: self.sources,
            resolver: self.resolver,
            player: self.player,
            tracker: self.tracker,
            enricher: self.enricher,
            history: self.history,
            presence: self.presence,
            prefs: self.prefs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use om_core::model::{Episode, IdSet, Season};

    // A fake metadata provider proves the app layer works against the *port*,
    // with zero network and no concrete adapter crate in scope (DIP).
    struct FakeMeta;

    #[async_trait]
    impl MetadataProvider for FakeMeta {
        fn name(&self) -> &str {
            "fake"
        }
        async fn search(&self, query: &str, kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
            Ok(vec![Media {
                kind: kind.unwrap_or(MediaKind::Movie),
                ids: IdSet::default().with_imdb("tt0000000"),
                title: format!("Result for {query}"),
                original_title: None,
                year: Some(2026),
                score: None,
                overview: None,
                poster: None,
                genres: vec![],
                status: None,
                episode_count: None,
                season_count: None,
            }])
        }
        async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
            Err(CoreError::NotImplemented("fake.details"))
        }
        async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
            Ok(vec![])
        }
        async fn episodes(&self, _ids: &IdSet, _season: u32) -> CoreResult<Vec<Episode>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn search_merges_provider_results() {
        let engine = Engine::builder().add_metadata(Arc::new(FakeMeta)).build();
        let results = engine
            .search("frieren", Some(MediaKind::Anime))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, MediaKind::Anime);
    }

    #[tokio::test]
    async fn search_without_providers_errors() {
        let engine = Engine::builder().build();
        assert!(engine.search("x", None).await.is_err());
    }
}
