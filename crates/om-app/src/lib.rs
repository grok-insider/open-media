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
//! - [`Engine::seasons`]/[`Engine::episodes`] — episodic navigation.
//! - [`Engine::play`] — the playback orchestrator (see its doc for the full
//!   sequence).
//!
//! Build one with [`EngineBuilder`].

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use om_core::error::{CoreError, CoreResult};
use om_core::model::{Episode, IdSet, Media, MediaKind, Season};
use om_core::ports::{
    Chapter, Enricher, HistoryStore, MetadataProvider, PlayOptions, PlaybackControl, Player,
    PresenceReporter, SourceProvider, SourceQuery, StreamResolver, Tracker,
};
use om_core::scoring::{self, ScoringPrefs};
use om_core::stream::{Playback, SourceCandidate};
use om_core::tracking::{Activity, SkipTimes, WatchProgress};

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
    /// Selected episode's runtime in minutes, when the metadata provider supplied
    /// one. Forwarded (as seconds) to the [`Enricher`] so AniSkip can validate
    /// skip intervals against the episode length; `None` disables that check.
    pub episode_runtime_minutes: Option<u32>,
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
    /// Fraction watched at which an episode counts as complete (e.g. 0.85).
    complete_threshold: f32,
}

impl Engine {
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Search every configured metadata provider concurrently and merge results.
    ///
    /// Provider failures are logged and skipped, not fatal — a TMDB outage should
    /// not stop AniList from returning anime. Results sharing an IMDB id (e.g. the
    /// same film from both Cinemeta and TMDB) are collapsed into one.
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
        Ok(dedup_by_imdb(out))
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
    pub async fn episodes(&self, ids: &IdSet, season: u32) -> CoreResult<Vec<Episode>> {
        for provider in &self.metadata {
            if let Ok(eps) = provider.episodes(ids, season).await {
                if !eps.is_empty() {
                    return Ok(eps);
                }
            }
        }
        Ok(Vec::new())
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
    /// The full sequence:
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
    pub async fn play(&self, req: &PlayRequest, candidate: &SourceCandidate) -> CoreResult<()> {
        let player = self
            .player
            .as_ref()
            .ok_or_else(|| CoreError::Config("no player configured".into()))?;

        // 1. Resolve to a playable URL (debrid-direct or P2P).
        let playback = self.resolve(candidate).await?;

        let media_key = req
            .media
            .ids
            .primary_key()
            .unwrap_or_else(|| req.media.display_title().to_string());
        let season = req.season.unwrap_or(1);
        let episode = req.episode.unwrap_or(1);

        // 2. Resume position (best-effort — disabled if no history port).
        let resume = self
            .history
            .as_ref()
            .and_then(|h| h.resume(&media_key, season, episode).ok().flatten())
            .map(|p| p.position_secs)
            .filter(|s| *s > 5);

        // 3. Skip windows (best-effort — anime, via the enricher). Forward the
        //    episode runtime (minutes → seconds) when known so AniSkip can validate
        //    intervals against the episode length.
        let episode_length_secs = req.episode_runtime_minutes.map(|m| m * 60);
        let skip = match &self.enricher {
            Some(e) => e
                .skip_times(&req.media.ids, episode, episode_length_secs)
                .await
                .unwrap_or_default(),
            None => SkipTimes::default(),
        };

        // 4. Launch the player with title + resume. The media-title carries the
        //    series name, the S01E01 coordinate, and the episode title when known
        //    (see `om_core::title`); movies get just the name (+ year).
        let opts = PlayOptions {
            title: Some(om_core::title::media_title(
                &req.media,
                season,
                episode,
                req.episode_title.as_deref(),
            )),
            start_at_secs: resume,
            extra_args: Vec::new(),
        };
        let mut session = player.play(&playback, &opts).await?;

        let last_pos = Arc::new(AtomicU32::new(resume.unwrap_or(0)));
        let last_dur = Arc::new(AtomicU32::new(0));

        // 5. Monitor over the IPC channel while the player runs. The monitor
        //    future runs forever; `select!` cancels it the moment the player
        //    exits, so it doubles as the "until playback ends" signal.
        let ctx = MonitorCtx {
            media_key: media_key.clone(),
            season,
            episode,
            title: req.media.display_title().to_string(),
            detail: om_core::title::episode_detail(
                &req.media,
                season,
                episode,
                req.episode_title.as_deref(),
            ),
            image: req.media.poster.clone(),
        };
        let monitor = monitor_playback(
            session.control(),
            self.history.clone(),
            self.presence.clone(),
            skip,
            ctx,
            last_pos.clone(),
            last_dur.clone(),
        );
        tokio::select! {
            r = session.wait() => { r?; }
            _ = monitor => {}
        }

        // 6. Teardown any transient P2P state.
        if let Some(resolver) = &self.resolver {
            resolver.cleanup().await;
        }

        // 7. Mark complete + sync the tracker if we got far enough (best-effort).
        let pos = last_pos.load(Ordering::Relaxed);
        let dur = last_dur.load(Ordering::Relaxed);
        if dur > 0 && (pos as f32 / dur as f32) >= self.complete_threshold {
            if let Some(tracker) = &self.tracker {
                if let Err(e) = tracker.update_progress(&req.media.ids, episode).await {
                    tracing::warn!(error = %e, "tracker progress update failed");
                }
            }
        }
        if let Some(presence) = &self.presence {
            let _ = presence.clear().await;
        }

        Ok(())
    }
}

/// Static context the monitor needs for history + presence.
struct MonitorCtx {
    media_key: String,
    season: u32,
    episode: u32,
    title: String,
    detail: String,
    image: Option<String>,
}

/// Poll the player's IPC channel ~1×/s: auto-skip OP/ED, persist progress, and
/// push presence. Returns only if there is no control channel (it then waits
/// forever, deferring to the player-exit branch of the caller's `select!`).
#[allow(clippy::too_many_arguments)]
async fn monitor_playback(
    control: Option<Arc<dyn PlaybackControl>>,
    history: Option<Arc<dyn HistoryStore>>,
    presence: Option<Arc<dyn PresenceReporter>>,
    skip: SkipTimes,
    ctx: MonitorCtx,
    last_pos: Arc<AtomicU32>,
    last_dur: Arc<AtomicU32>,
) {
    let Some(ctrl) = control else {
        // Launch-only player (vlc): nothing to monitor; wait for exit.
        std::future::pending::<()>().await;
        return;
    };

    let chapters = chapters_from_skip(&skip);
    if !chapters.is_empty() {
        let _ = ctrl.set_chapters(&chapters).await;
    }

    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let pos = ctrl.position().await.ok().flatten().unwrap_or(0);
        let dur = ctrl.duration().await.ok().flatten().unwrap_or(0);
        if pos > 0 {
            last_pos.store(pos, Ordering::Relaxed);
        }
        if dur > 0 {
            last_dur.store(dur, Ordering::Relaxed);
        }

        // Auto-skip: fire once on entry into the window (2s trigger, curd's rule).
        if let Some(op) = skip.opening {
            if op.is_meaningful() && pos >= op.start && pos < op.start + 2 {
                let _ = ctrl.seek_absolute(op.end).await;
            }
        }
        if let Some(ed) = skip.ending {
            if ed.is_meaningful() && pos >= ed.start && pos < ed.start + 2 {
                let _ = ctrl.seek_absolute(ed.end).await;
            }
        }

        if let Some(h) = &history {
            let _ = h.save(&WatchProgress {
                media_key: ctx.media_key.clone(),
                season: ctx.season,
                episode: ctx.episode,
                position_secs: pos,
                duration_secs: dur,
                updated_at: unix_now(),
            });
        }

        if let Some(p) = &presence {
            let paused = ctrl.is_paused().await.ok().flatten().unwrap_or(false);
            let _ = p
                .update(&Activity {
                    title: ctx.title.clone(),
                    detail: ctx.detail.clone(),
                    paused,
                    position_secs: pos,
                    duration_secs: dur,
                    image_url: ctx.image.clone(),
                })
                .await;
        }
    }
}

fn chapters_from_skip(skip: &SkipTimes) -> Vec<Chapter> {
    let mut chapters = Vec::new();
    if let Some(op) = skip.opening {
        if op.is_meaningful() {
            chapters.push(Chapter {
                title: "Opening".into(),
                time_secs: op.start,
            });
            chapters.push(Chapter {
                title: "Episode".into(),
                time_secs: op.end,
            });
        }
    }
    if let Some(ed) = skip.ending {
        if ed.is_meaningful() {
            chapters.push(Chapter {
                title: "Ending".into(),
                time_secs: ed.start,
            });
        }
    }
    chapters
}

/// Collapse provider results that share an IMDB id, preserving order.
///
/// Cinemeta and TMDB both resolve a live-action title to the same `tt…` id; a
/// keyed user would otherwise see each movie/series twice. First occurrence wins
/// (provider order in the builder is the priority); later duplicates only donate
/// ids the kept entry lacks. Items without an IMDB id (e.g. AniList anime) are
/// never collapsed.
fn dedup_by_imdb(items: Vec<Media>) -> Vec<Media> {
    let mut out: Vec<Media> = Vec::with_capacity(items.len());
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for item in items {
        if let Some(imdb) = item.ids.imdb.clone() {
            if let Some(&idx) = seen.get(&imdb) {
                out[idx].ids.merge(&item.ids);
                continue;
            }
            seen.insert(imdb, out.len());
        }
        out.push(item);
    }
    out
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
    complete_threshold: f32,
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
    /// Fraction watched at which an episode is marked complete (default 0.85).
    pub fn complete_threshold(mut self, threshold: f32) -> Self {
        self.complete_threshold = threshold;
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
            complete_threshold: if self.complete_threshold > 0.0 {
                self.complete_threshold
            } else {
                0.85
            },
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

    fn media_with_ids(title: &str, ids: IdSet) -> Media {
        Media {
            kind: MediaKind::Movie,
            ids,
            title: title.into(),
            original_title: None,
            year: None,
            score: None,
            overview: None,
            poster: None,
            genres: vec![],
            status: None,
            episode_count: None,
            season_count: None,
        }
    }

    #[test]
    fn dedup_collapses_shared_imdb_and_folds_ids() {
        // Same film from Cinemeta (imdb only) and TMDB (imdb + tmdb).
        let items = vec![
            media_with_ids("Interstellar", IdSet::default().with_imdb("tt0816692")),
            media_with_ids(
                "Interstellar",
                IdSet::default().with_imdb("tt0816692").with_tmdb(157336),
            ),
        ];
        let out = dedup_by_imdb(items);
        assert_eq!(out.len(), 1);
        // First occurrence kept; the later duplicate donated its tmdb id.
        assert_eq!(out[0].ids.tmdb, Some(157336));
    }

    #[test]
    fn dedup_keeps_items_without_imdb() {
        // Two AniList anime (no imdb) must not collapse into one.
        let items = vec![
            media_with_ids("Frieren", IdSet::default().with_anilist(154587)),
            media_with_ids("Bocchi", IdSet::default().with_anilist(140960)),
        ];
        let out = dedup_by_imdb(items);
        assert_eq!(out.len(), 2);
    }
}
