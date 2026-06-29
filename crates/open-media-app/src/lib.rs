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

use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::{Episode, IdSet, Media, MediaKind, Season};
use open_media_core::ports::{
    Chapter, Enricher, HistoryStore, MetadataProvider, PlayOptions, PlaybackControl, Player,
    PresenceReporter, SourceProvider, SourceQuery, StreamResolver, SubtitleProvider, Tracker,
};
use open_media_core::scoring::{self, ScoringPrefs};
use open_media_core::stream::{Playback, SourceCandidate};
use open_media_core::subtitle::{SubtitleQuery, SubtitleTrack};
use open_media_core::tracking::{Activity, SkipTimes, WatchProgress};

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
    subtitles: Option<Arc<dyn SubtitleProvider>>,
    /// Preferred subtitle languages (most-wanted first) for the [`SubtitleQuery`].
    /// Empty when no provider is wired or none are configured.
    subtitle_languages: Vec<String>,
    prefs: ScoringPrefs,
    /// Fraction watched at which an episode counts as complete (e.g. 0.85).
    complete_threshold: f32,
    /// Skip filler/recap episodes when advancing (anime, via the [`Enricher`]).
    skip_filler: bool,
    /// Auto-advance to the next episode after a completed episodic playback.
    autoplay_next: bool,
    /// Seek to the saved position when starting playback. When `false`, history
    /// is still recorded but playback always starts from the beginning.
    resume: bool,
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
                // The answering provider only knows its own id dialect; fold in
                // the ids discovered during search so cross-provider ids (e.g. a
                // mal id from AniList) survive into the hydrated result.
                Ok(mut media) => {
                    media.ids.merge(ids);
                    return Ok(media);
                }
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
        let absolute_episode = self.absolute_episode(req).await;
        let query = SourceQuery {
            media: req.media.clone(),
            season: req.season,
            episode: req.episode,
            absolute_episode,
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
    /// [`Playback`]: open_media_core::stream::Playback
    /// [`PlaybackControl`]: open_media_core::ports::PlaybackControl
    pub async fn play(&self, req: &PlayRequest, candidate: &SourceCandidate) -> CoreResult<()> {
        // Single-shot playback of the chosen candidate.
        let completed = self.play_once(req, candidate).await?;

        // Binge: when enabled and the episode was actually watched to completion,
        // keep advancing to the next (non-filler) episode until we run out of
        // episodes, sources, or the viewer quits early. An owned loop (not
        // recursion) keeps the stack flat across an arbitrarily long binge.
        if !self.autoplay_next || !req.media.kind.is_episodic() || !completed {
            return Ok(());
        }

        // Filler/recap episode numbers, fetched at most once for the whole binge.
        let filler = self.filler_episodes(&req.media.ids).await;

        let mut current = req.clone();
        loop {
            let Some(next) = self.next_request(&current, &filler).await else {
                break;
            };
            let Some(candidate) = self.pick_candidate(&next).await else {
                // No playable source for the next episode — stop rather than skip
                // a gap silently; the viewer can resume manually.
                break;
            };
            if !self.play_once(&next, &candidate).await? {
                break; // quit / low-progress exit ends the binge.
            }
            current = next;
        }
        Ok(())
    }

    /// Resolve a chosen candidate and play it through once. Returns whether the
    /// episode crossed [`Self::complete_threshold`] (i.e. was actually watched to
    /// the end, as opposed to a quit at low progress) — the binge loop uses this
    /// to decide whether to advance.
    async fn play_once(&self, req: &PlayRequest, candidate: &SourceCandidate) -> CoreResult<bool> {
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

        // 2. Resume position (best-effort — disabled if no history port, or when
        //    `resume` is off in config). `resume = false` gates only the
        //    start-position seek; progress is still recorded by the monitor.
        let resume = self
            .resume
            .then(|| {
                self.history
                    .as_ref()
                    .and_then(|h| h.resume(&media_key, season, episode).ok().flatten())
                    .map(|p| p.position_secs)
                    .filter(|s| *s > 5)
            })
            .flatten();

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

        // 4. External subtitles (best-effort — anime/series/movies, via the
        //    subtitle provider). Fetched tracks are written to temp `.srt`/`.vtt`
        //    files and handed to the player as `--sub-file=PATH` args. Any failure
        //    (no provider, lookup error, write error) degrades to no subtitles and
        //    must never block playback. The temp files are cleaned up after exit.
        let subtitle_files = self.fetch_subtitle_files(req).await;
        let sub_args: Vec<String> = subtitle_files
            .iter()
            .map(|p| format!("--sub-file={}", p.display()))
            .collect();

        // 5. Launch the player with title + resume + subtitles. The media-title
        //    carries the series name, the S01E01 coordinate, and the episode title
        //    when known (see `open_media_core::title`); movies get just the name (+ year).
        let opts = PlayOptions {
            title: Some(open_media_core::title::media_title(
                &req.media,
                season,
                episode,
                req.episode_title.as_deref(),
            )),
            start_at_secs: resume,
            extra_args: sub_args,
        };
        let session = player.play(&playback, &opts).await;
        // Even if launch fails, drop the temp subtitle files we wrote.
        let mut session = match session {
            Ok(s) => s,
            Err(e) => {
                cleanup_subtitle_files(&subtitle_files);
                return Err(e);
            }
        };

        let last_pos = Arc::new(AtomicU32::new(resume.unwrap_or(0)));
        let last_dur = Arc::new(AtomicU32::new(0));

        // 6. Monitor over the IPC channel while the player runs. The monitor
        //    future runs forever; `select!` cancels it the moment the player
        //    exits, so it doubles as the "until playback ends" signal.
        let ctx = MonitorCtx {
            media_key: media_key.clone(),
            season,
            episode,
            title: req.media.display_title().to_string(),
            detail: open_media_core::title::episode_detail(
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
        let wait_result = tokio::select! {
            r = session.wait() => r,
            _ = monitor => Ok(()),
        };

        // 7. Teardown: remove the temp subtitle files and any transient P2P
        //    state. Done before propagating a `wait` error so a player crash
        //    never leaks temp files.
        cleanup_subtitle_files(&subtitle_files);
        if let Some(resolver) = &self.resolver {
            resolver.cleanup().await;
        }
        wait_result?;

        // 8. Mark complete + sync the tracker if we got far enough (best-effort).
        let pos = last_pos.load(Ordering::Relaxed);
        let dur = last_dur.load(Ordering::Relaxed);
        let completed = dur > 0 && (pos as f32 / dur as f32) >= self.complete_threshold;
        if completed {
            if let Some(tracker) = &self.tracker {
                if let Err(e) = tracker.update_progress(&req.media.ids, episode).await {
                    tracing::warn!(error = %e, "tracker progress update failed");
                }
            }
        }
        if let Some(presence) = &self.presence {
            let _ = presence.clear().await;
        }

        Ok(completed)
    }

    /// Filler/recap episode numbers to skip when bingeing, or an empty set when
    /// `skip_filler` is off, no [`Enricher`] is wired, or the lookup fails.
    /// Best-effort: a filler-list failure must never abort a binge.
    async fn filler_episodes(&self, ids: &IdSet) -> Vec<u32> {
        if !self.skip_filler {
            return Vec::new();
        }
        match &self.enricher {
            Some(e) => e.filler_episodes(ids).await.unwrap_or_else(|err| {
                tracing::debug!(error = %err, "filler_episodes lookup failed");
                Vec::new()
            }),
            None => Vec::new(),
        }
    }

    /// Build the [`PlayRequest`] for the next episode after `current`, skipping
    /// `filler` numbers and stopping at the season's last episode. Returns `None`
    /// when there is no next episode (end of season / non-episodic / no coordinate).
    ///
    /// The season's episode list is fetched fresh so the per-episode title and
    /// runtime travel with the request (the player's media-title + AniSkip
    /// interval validation depend on them).
    async fn next_request(&self, current: &PlayRequest, filler: &[u32]) -> Option<PlayRequest> {
        let season = current.season.unwrap_or(1);
        let episode = current.episode?;

        // Upper bound: the season's real episode list, else the season pack's
        // count, else the media-level total. Without any bound we refuse to
        // advance past the current episode (better than looping forever).
        let episodes = self.episodes(&current.media.ids, season).await.ok();
        let last = episodes
            .as_ref()
            .filter(|e| !e.is_empty())
            .map(|e| e.iter().map(|ep| ep.number).max().unwrap_or(episode))
            .or(current.media.episode_count)?;

        // Walk forward past any filler/recap numbers, staying within the season.
        let mut next = episode + 1;
        while filler.contains(&next) {
            next += 1;
        }
        if next > last {
            return None;
        }

        // Carry the next episode's title/runtime when the season list knows them.
        let (episode_title, episode_runtime_minutes) = episodes
            .and_then(|eps| eps.into_iter().find(|ep| ep.number == next))
            .map(|ep| (ep.title, ep.runtime_minutes))
            .unwrap_or((None, None));

        Some(PlayRequest {
            media: current.media.clone(),
            season: Some(season),
            episode: Some(next),
            episode_title,
            episode_runtime_minutes,
            include_uncached: current.include_uncached,
        })
    }

    /// Find + rank sources for a request and return the top resolvable candidate,
    /// reusing the same ranking [`Engine::find_sources`] applies. `None` when the
    /// lookup fails or nothing is resolvable.
    async fn pick_candidate(&self, req: &PlayRequest) -> Option<SourceCandidate> {
        self.find_sources(req)
            .await
            .ok()?
            .into_iter()
            .find(|c| c.is_resolvable())
    }

    /// Fetch external subtitles for the request and materialize each returned
    /// track to a temp file, returning the paths to hand the player as
    /// `--sub-file=` args.
    ///
    /// Best-effort end to end: returns an empty vec when no [`SubtitleProvider`]
    /// is wired, the provider errors, or a track can't be written — playback must
    /// never be blocked by subtitles. Errors are logged at `debug`. The caller is
    /// responsible for deleting the files via [`cleanup_subtitle_files`] after the
    /// player exits.
    async fn fetch_subtitle_files(&self, req: &PlayRequest) -> Vec<std::path::PathBuf> {
        let Some(provider) = &self.subtitles else {
            return Vec::new();
        };

        let query = SubtitleQuery {
            media: req.media.clone(),
            season: req.season,
            episode: req.episode,
            languages: self.subtitle_languages.clone(),
        };

        let tracks = match provider.fetch(&query).await {
            Ok(tracks) => tracks,
            Err(e) => {
                tracing::debug!(error = %e, "subtitle fetch failed; continuing without subtitles");
                return Vec::new();
            }
        };

        let mut paths = Vec::new();
        for (idx, track) in tracks.iter().enumerate() {
            match write_subtitle_track(idx, track) {
                Ok(path) => paths.push(path),
                Err(e) => {
                    tracing::debug!(error = %e, "writing subtitle temp file failed; skipping track");
                }
            }
        }
        paths
    }
}

/// Write a single [`SubtitleTrack`] to a uniquely-named temp file and return its
/// path. The name is process/instance-unique (pid + nanos + index) so concurrent
/// or repeated playbacks never collide, mirroring the mpv IPC socket naming.
fn write_subtitle_track(idx: usize, track: &SubtitleTrack) -> std::io::Result<std::path::PathBuf> {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Default to `srt` when the format is blank; both mpv and vlc key the parser
    // off the extension.
    let ext = if track.format.is_empty() {
        "srt"
    } else {
        track.format.as_str()
    };
    let path = std::env::temp_dir().join(format!("om-sub-{pid}-{nanos}-{idx}.{ext}"));
    std::fs::write(&path, &track.text)?;
    Ok(path)
}

/// Delete temp subtitle files after playback. Best-effort: a missing/locked file
/// is ignored — these live under the OS temp dir and are reaped anyway.
fn cleanup_subtitle_files(paths: &[std::path::PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
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
    subtitles: Option<Arc<dyn SubtitleProvider>>,
    subtitle_languages: Vec<String>,
    prefs: ScoringPrefs,
    complete_threshold: f32,
    skip_filler: bool,
    autoplay_next: bool,
    resume: Option<bool>,
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
    /// External subtitle provider (optional). When set, [`Engine::play`] fetches
    /// subtitles before launching the player and passes them as `--sub-file=`.
    pub fn subtitles(mut self, s: Arc<dyn SubtitleProvider>) -> Self {
        self.subtitles = Some(s);
        self
    }
    /// Preferred subtitle languages (most-wanted first, e.g. `["en", "ja"]`) used
    /// for the subtitle search. Default empty; only meaningful with a
    /// [`subtitles`](Self::subtitles) provider wired.
    pub fn subtitle_languages(mut self, languages: Vec<String>) -> Self {
        self.subtitle_languages = languages;
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
    /// Skip filler/recap episodes when auto-advancing (default false). Only has an
    /// effect when an [`Enricher`] is also wired.
    pub fn skip_filler(mut self, skip: bool) -> Self {
        self.skip_filler = skip;
        self
    }
    /// Auto-advance to the next episode after a completed episodic playback
    /// (binge mode; default false).
    pub fn autoplay_next(mut self, enabled: bool) -> Self {
        self.autoplay_next = enabled;
        self
    }
    /// Seek to the saved resume position when starting playback (default true).
    /// When `false`, playback always starts from the beginning, but progress is
    /// still recorded to the [`HistoryStore`] for later sessions.
    pub fn resume(mut self, enabled: bool) -> Self {
        self.resume = Some(enabled);
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
            subtitles: self.subtitles,
            subtitle_languages: self.subtitle_languages,
            prefs: self.prefs,
            complete_threshold: if self.complete_threshold > 0.0 {
                self.complete_threshold
            } else {
                0.85
            },
            skip_filler: self.skip_filler,
            autoplay_next: self.autoplay_next,
            resume: self.resume.unwrap_or(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use open_media_core::model::{Episode, IdSet, Season};
    use open_media_core::ports::{PlayOptions, PlaySession};
    use open_media_core::stream::{CacheState, Playback, PlaybackOrigin, Quality, SourceCandidate};
    use std::sync::Mutex;

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

    // A metadata provider whose `details` answers with only the ids it knows
    // (here: anilist), dropping any others the caller already discovered.
    struct PartialDetailsMeta;

    #[async_trait]
    impl MetadataProvider for PartialDetailsMeta {
        fn name(&self) -> &str {
            "partial"
        }
        async fn search(&self, _query: &str, _kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
            Ok(vec![])
        }
        async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
            // Note: no mal id — the provider only speaks anilist.
            Ok(media_with_ids(
                "Frieren",
                IdSet::default().with_anilist(154587),
            ))
        }
        async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
            Ok(vec![])
        }
        async fn episodes(&self, _ids: &IdSet, _season: u32) -> CoreResult<Vec<Episode>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn details_merges_input_ids_into_result() {
        let engine = Engine::builder()
            .add_metadata(Arc::new(PartialDetailsMeta))
            .build();
        // Caller carries a mal id discovered during search; the provider's
        // result lacks it and must not drop it.
        let media = engine
            .details(&IdSet::default().with_anilist(154587).with_mal(52991))
            .await
            .unwrap();
        assert_eq!(media.ids.mal, Some(52991));
        assert_eq!(media.ids.anilist, Some(154587));
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

    // ---- Binge / auto-advance ------------------------------------------------
    //
    // These fakes exercise the play→advance loop with zero I/O. The episode
    // number flows end-to-end through the chain so the player can record exactly
    // which episodes were binged: the source provider stamps the requested
    // episode into the candidate title, the resolver copies it into the
    // `Playback.file_name`, and the fake player parses + records it.

    /// Episodic metadata with a fixed-size single season (numbers `1..=count`).
    struct SeasonMeta {
        count: u32,
    }

    #[async_trait]
    impl MetadataProvider for SeasonMeta {
        fn name(&self) -> &str {
            "season-meta"
        }
        async fn search(&self, _q: &str, _k: Option<MediaKind>) -> CoreResult<Vec<Media>> {
            Ok(vec![])
        }
        async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
            Err(CoreError::NotImplemented("season-meta.details"))
        }
        async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
            Ok(vec![Season {
                number: 1,
                episode_count: self.count,
                name: None,
            }])
        }
        async fn episodes(&self, _ids: &IdSet, season: u32) -> CoreResult<Vec<Episode>> {
            Ok((1..=self.count)
                .map(|n| Episode {
                    season,
                    number: n,
                    title: Some(format!("Episode {n}")),
                    air_date: None,
                    overview: None,
                    runtime_minutes: Some(24),
                    rating: None,
                    still: None,
                })
                .collect())
        }
    }

    /// One always-resolvable candidate per request, tagged with the episode so the
    /// rest of the chain can carry it through to the player.
    struct StampSource;

    #[async_trait]
    impl SourceProvider for StampSource {
        fn name(&self) -> &str {
            "stamp"
        }
        async fn find(&self, query: &SourceQuery) -> CoreResult<Vec<SourceCandidate>> {
            let ep = query.episode.unwrap_or(0);
            Ok(vec![SourceCandidate {
                provider: "stamp".into(),
                title: format!("ep={ep}"),
                quality: Quality::P1080,
                size_bytes: 1,
                seeders: Some(1),
                info_hash: Some("0".repeat(40)),
                magnet: None,
                direct_url: None,
                file_index: None,
                cache: CacheState::Cached,
                tags: Default::default(),
            }])
        }
    }

    /// Copies the candidate title (the `ep=N` stamp) into the playback file name.
    struct EchoResolver;

    #[async_trait]
    impl StreamResolver for EchoResolver {
        async fn resolve(&self, candidate: &SourceCandidate) -> CoreResult<Playback> {
            Ok(Playback {
                url: "http://localhost/stream".into(),
                origin: PlaybackOrigin::LocalP2p,
                file_name: candidate.title.clone(),
            })
        }
    }

    /// A player that "completes" instantly: it records the episode number parsed
    /// from the playback file name, then its session reports a watched position
    /// (>= threshold) and exits. The control reports position synchronously so the
    /// monitor stores it on its first tick, before `wait()` resolves.
    struct RecordingPlayer {
        played: Arc<Mutex<Vec<u32>>>,
        /// fraction watched to report (controls "completed").
        fraction: f32,
    }

    #[async_trait]
    impl Player for RecordingPlayer {
        fn name(&self) -> &str {
            "recording"
        }
        fn is_available(&self) -> bool {
            true
        }
        async fn play(
            &self,
            playback: &Playback,
            _opts: &PlayOptions,
        ) -> CoreResult<Box<dyn PlaySession>> {
            let ep = playback
                .file_name
                .strip_prefix("ep=")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            self.played.lock().unwrap().push(ep);
            Ok(Box::new(RecordingSession {
                control: Arc::new(RecordingControl {
                    pos: (100.0 * self.fraction) as u32,
                    dur: 100,
                }),
            }))
        }
    }

    struct RecordingSession {
        control: Arc<RecordingControl>,
    }

    #[async_trait]
    impl PlaySession for RecordingSession {
        async fn wait(&mut self) -> CoreResult<()> {
            // Outlive one monitor tick (1s) so progress is stored before exit.
            tokio::time::sleep(Duration::from_millis(1100)).await;
            Ok(())
        }
        fn control(&self) -> Option<Arc<dyn PlaybackControl>> {
            Some(self.control.clone())
        }
    }

    struct RecordingControl {
        pos: u32,
        dur: u32,
    }

    #[async_trait]
    impl PlaybackControl for RecordingControl {
        async fn position(&self) -> CoreResult<Option<u32>> {
            Ok(Some(self.pos))
        }
        async fn duration(&self) -> CoreResult<Option<u32>> {
            Ok(Some(self.dur))
        }
        async fn is_paused(&self) -> CoreResult<Option<bool>> {
            Ok(Some(false))
        }
        async fn seek_absolute(&self, _secs: u32) -> CoreResult<()> {
            Ok(())
        }
        async fn set_chapters(&self, _chapters: &[Chapter]) -> CoreResult<()> {
            Ok(())
        }
        async fn quit(&self) -> CoreResult<()> {
            Ok(())
        }
    }

    /// An enricher that reports a fixed filler set (and no skip windows).
    struct FillerEnricher {
        filler: Vec<u32>,
    }

    #[async_trait]
    impl Enricher for FillerEnricher {
        async fn skip_times(
            &self,
            _ids: &IdSet,
            _episode: u32,
            _len: Option<u32>,
        ) -> CoreResult<SkipTimes> {
            Ok(SkipTimes::default())
        }
        async fn filler_episodes(&self, _ids: &IdSet) -> CoreResult<Vec<u32>> {
            Ok(self.filler.clone())
        }
    }

    fn episodic_media() -> Media {
        Media {
            kind: MediaKind::Anime,
            ids: IdSet::default().with_anilist(1),
            title: "Test Show".into(),
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

    fn play_request(media: Media) -> PlayRequest {
        PlayRequest {
            media,
            season: Some(1),
            episode: Some(1),
            episode_title: None,
            episode_runtime_minutes: None,
            include_uncached: false,
        }
    }

    fn resolvable_candidate() -> SourceCandidate {
        SourceCandidate {
            provider: "stamp".into(),
            title: "ep=1".into(),
            quality: Quality::P1080,
            size_bytes: 1,
            seeders: Some(1),
            info_hash: Some("0".repeat(40)),
            magnet: None,
            direct_url: None,
            file_index: None,
            cache: CacheState::Cached,
            tags: Default::default(),
        }
    }

    #[tokio::test]
    async fn binge_advances_to_season_end_then_stops() {
        let played = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::builder()
            .add_metadata(Arc::new(SeasonMeta { count: 3 }))
            .add_source(Arc::new(StampSource))
            .resolver(Arc::new(EchoResolver))
            .player(Arc::new(RecordingPlayer {
                played: played.clone(),
                fraction: 0.95, // every episode completes → keep advancing.
            }))
            .autoplay_next(true)
            .build();

        engine
            .play(&play_request(episodic_media()), &resolvable_candidate())
            .await
            .unwrap();

        // Played E1 (initial) then auto-advanced E2, E3, and stopped at the
        // season's last episode (no E4).
        assert_eq!(*played.lock().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn binge_disabled_does_not_advance() {
        let played = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::builder()
            .add_metadata(Arc::new(SeasonMeta { count: 3 }))
            .add_source(Arc::new(StampSource))
            .resolver(Arc::new(EchoResolver))
            .player(Arc::new(RecordingPlayer {
                played: played.clone(),
                fraction: 0.95,
            }))
            .autoplay_next(false)
            .build();

        engine
            .play(&play_request(episodic_media()), &resolvable_candidate())
            .await
            .unwrap();

        // Only the initial episode played; no auto-advance.
        assert_eq!(*played.lock().unwrap(), vec![1]);
    }

    #[tokio::test]
    async fn binge_stops_when_episode_not_completed() {
        let played = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::builder()
            .add_metadata(Arc::new(SeasonMeta { count: 3 }))
            .add_source(Arc::new(StampSource))
            .resolver(Arc::new(EchoResolver))
            .player(Arc::new(RecordingPlayer {
                played: played.clone(),
                fraction: 0.10, // quit at low progress → no advance.
            }))
            .autoplay_next(true)
            .build();

        engine
            .play(&play_request(episodic_media()), &resolvable_candidate())
            .await
            .unwrap();

        assert_eq!(*played.lock().unwrap(), vec![1]);
    }

    #[tokio::test]
    async fn binge_skips_filler_episodes() {
        let played = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::builder()
            .add_metadata(Arc::new(SeasonMeta { count: 4 }))
            .add_source(Arc::new(StampSource))
            .resolver(Arc::new(EchoResolver))
            .player(Arc::new(RecordingPlayer {
                played: played.clone(),
                fraction: 0.95,
            }))
            .enricher(Arc::new(FillerEnricher { filler: vec![2, 3] }))
            .skip_filler(true)
            .autoplay_next(true)
            .build();

        engine
            .play(&play_request(episodic_media()), &resolvable_candidate())
            .await
            .unwrap();

        // E2 and E3 are filler → bridged over: E1 then E4.
        assert_eq!(*played.lock().unwrap(), vec![1, 4]);
    }

    // ---- behavior.resume gating ---------------------------------------------
    //
    // A history store that always has a saved position, plus a player that
    // captures the `start_at_secs` it was handed, isolate exactly what the
    // `resume` flag controls: whether the saved position becomes a start seek.

    /// History with a fixed saved position; records every `save` it receives so
    /// the test can assert progress is still recorded when resume is off.
    struct SavedHistory {
        position: u32,
        saved: Arc<Mutex<Vec<u32>>>,
    }

    impl HistoryStore for SavedHistory {
        fn save(&self, progress: &WatchProgress) -> CoreResult<()> {
            self.saved.lock().unwrap().push(progress.position_secs);
            Ok(())
        }
        fn resume(
            &self,
            media_key: &str,
            season: u32,
            episode: u32,
        ) -> CoreResult<Option<WatchProgress>> {
            Ok(Some(WatchProgress {
                media_key: media_key.into(),
                season,
                episode,
                position_secs: self.position,
                duration_secs: 1000,
                updated_at: 0,
            }))
        }
        fn recent(&self, _limit: usize) -> CoreResult<Vec<WatchProgress>> {
            Ok(vec![])
        }
    }

    /// A player that records the `start_at_secs` it was asked to start at, then
    /// exits immediately. Used to observe how the engine gated the resume seek.
    struct StartCapturePlayer {
        start: Arc<Mutex<Option<Option<u32>>>>,
    }

    #[async_trait]
    impl Player for StartCapturePlayer {
        fn name(&self) -> &str {
            "start-capture"
        }
        fn is_available(&self) -> bool {
            true
        }
        async fn play(
            &self,
            _playback: &Playback,
            opts: &PlayOptions,
        ) -> CoreResult<Box<dyn PlaySession>> {
            *self.start.lock().unwrap() = Some(opts.start_at_secs);
            Ok(Box::new(InstantSession))
        }
    }

    /// A session with no control channel that exits at once: the play path runs
    /// through to teardown without waiting on a monitor tick.
    struct InstantSession;

    #[async_trait]
    impl PlaySession for InstantSession {
        async fn wait(&mut self) -> CoreResult<()> {
            Ok(())
        }
        fn control(&self) -> Option<Arc<dyn PlaybackControl>> {
            None
        }
    }

    fn movie_request() -> PlayRequest {
        PlayRequest {
            media: media_with_ids("Interstellar", IdSet::default().with_imdb("tt0816692")),
            season: None,
            episode: None,
            episode_title: None,
            episode_runtime_minutes: None,
            include_uncached: false,
        }
    }

    #[tokio::test]
    async fn resume_disabled_starts_from_zero_despite_saved_position() {
        let start = Arc::new(Mutex::new(None));
        let saved = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::builder()
            .resolver(Arc::new(EchoResolver))
            .player(Arc::new(StartCapturePlayer {
                start: start.clone(),
            }))
            .history(Arc::new(SavedHistory {
                position: 120,
                saved: saved.clone(),
            }))
            .resume(false)
            .build();

        engine
            .play(&movie_request(), &resolvable_candidate())
            .await
            .unwrap();

        // History has a 120s position, but resume is off → no start seek.
        assert_eq!(*start.lock().unwrap(), Some(None));
    }

    #[tokio::test]
    async fn resume_enabled_starts_from_saved_position() {
        let start = Arc::new(Mutex::new(None));
        let saved = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::builder()
            .resolver(Arc::new(EchoResolver))
            .player(Arc::new(StartCapturePlayer {
                start: start.clone(),
            }))
            .history(Arc::new(SavedHistory {
                position: 120,
                saved: saved.clone(),
            }))
            .resume(true)
            .build();

        engine
            .play(&movie_request(), &resolvable_candidate())
            .await
            .unwrap();

        // resume on (the default) → the saved 120s becomes the start seek.
        assert_eq!(*start.lock().unwrap(), Some(Some(120)));
    }

    // ---- subtitle auto-fetch -------------------------------------------------
    //
    // A fake provider returns canned tracks; a player captures the `extra_args`
    // it was handed so the test can assert each track became a `--sub-file=`
    // pointing at a real temp file. After `play` returns, the temp files must be
    // gone (best-effort cleanup ran).

    use open_media_core::ports::SubtitleProvider;
    use open_media_core::subtitle::{SubtitleQuery, SubtitleTrack};

    /// A subtitle provider returning a fixed set of tracks, recording the query it
    /// was asked (so the test can assert the languages were threaded through).
    struct FakeSubs {
        tracks: Vec<SubtitleTrack>,
        seen_langs: Arc<Mutex<Option<Vec<String>>>>,
    }

    #[async_trait]
    impl SubtitleProvider for FakeSubs {
        fn name(&self) -> &str {
            "fake-subs"
        }
        async fn fetch(&self, query: &SubtitleQuery) -> CoreResult<Vec<SubtitleTrack>> {
            *self.seen_langs.lock().unwrap() = Some(query.languages.clone());
            Ok(self.tracks.clone())
        }
    }

    /// A subtitle provider that always errors — proves playback continues with no
    /// subtitles when the fetch fails.
    struct FailingSubs;

    #[async_trait]
    impl SubtitleProvider for FailingSubs {
        fn name(&self) -> &str {
            "failing-subs"
        }
        async fn fetch(&self, _query: &SubtitleQuery) -> CoreResult<Vec<SubtitleTrack>> {
            Err(CoreError::Network("boom".into()))
        }
    }

    /// A player that records the `extra_args` it was handed, then exits at once.
    struct ArgsCapturePlayer {
        args: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Player for ArgsCapturePlayer {
        fn name(&self) -> &str {
            "args-capture"
        }
        fn is_available(&self) -> bool {
            true
        }
        async fn play(
            &self,
            _playback: &Playback,
            opts: &PlayOptions,
        ) -> CoreResult<Box<dyn PlaySession>> {
            *self.args.lock().unwrap() = opts.extra_args.clone();
            Ok(Box::new(InstantSession))
        }
    }

    fn sub_track(format: &str, text: &str) -> SubtitleTrack {
        SubtitleTrack {
            language: "en".into(),
            format: format.into(),
            text: text.into(),
            title: None,
        }
    }

    #[tokio::test]
    async fn subtitles_become_sub_file_args_and_are_cleaned_up() {
        let args = Arc::new(Mutex::new(Vec::new()));
        let seen_langs = Arc::new(Mutex::new(None));
        let engine = Engine::builder()
            .resolver(Arc::new(EchoResolver))
            .player(Arc::new(ArgsCapturePlayer { args: args.clone() }))
            .subtitles(Arc::new(FakeSubs {
                tracks: vec![sub_track("srt", "1\n00:00:01,000 --> 00:00:02,000\nHi\n")],
                seen_langs: seen_langs.clone(),
            }))
            .subtitle_languages(vec!["en".into(), "ja".into()])
            .build();

        engine
            .play(&movie_request(), &resolvable_candidate())
            .await
            .unwrap();

        // One track → one --sub-file arg, pointing at a real .srt path.
        let captured = args.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        let path = captured[0]
            .strip_prefix("--sub-file=")
            .expect("arg is a --sub-file=");
        assert!(path.ends_with(".srt"), "extension follows the track format");
        // The configured languages were threaded into the query.
        assert_eq!(
            seen_langs.lock().unwrap().clone(),
            Some(vec!["en".to_string(), "ja".to_string()])
        );
        // Cleanup ran after the player exited.
        assert!(
            !std::path::Path::new(path).exists(),
            "temp subtitle file removed after playback"
        );
    }

    #[tokio::test]
    async fn no_subtitle_provider_means_no_extra_args() {
        let args = Arc::new(Mutex::new(vec!["sentinel".to_string()]));
        let engine = Engine::builder()
            .resolver(Arc::new(EchoResolver))
            .player(Arc::new(ArgsCapturePlayer { args: args.clone() }))
            .build();

        engine
            .play(&movie_request(), &resolvable_candidate())
            .await
            .unwrap();

        // The player was handed an empty extra_args (overwrote the sentinel).
        assert!(args.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn subtitle_fetch_error_does_not_block_playback() {
        let args = Arc::new(Mutex::new(vec!["sentinel".to_string()]));
        let engine = Engine::builder()
            .resolver(Arc::new(EchoResolver))
            .player(Arc::new(ArgsCapturePlayer { args: args.clone() }))
            .subtitles(Arc::new(FailingSubs))
            .subtitle_languages(vec!["en".into()])
            .build();

        // Playback still succeeds; no subtitle args were added.
        engine
            .play(&movie_request(), &resolvable_candidate())
            .await
            .unwrap();
        assert!(args.lock().unwrap().is_empty());
    }
}
