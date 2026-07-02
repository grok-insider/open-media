use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::IdSet;
use open_media_core::ports::{
    Chapter, HistoryStore, PlayOptions, PlaybackControl, PresenceReporter,
};
use open_media_core::stream::{Playback, SourceCandidate};
use open_media_core::subtitle::{SubtitleQuery, SubtitleTrack};
use open_media_core::tracking::{Activity, Interval, SkipTimes, WatchProgress};

use crate::library::unix_now;
use crate::playlist::PlaylistEntry;
use crate::{Engine, PlayRequest};

impl Engine {
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
    /// [`StreamResolver::resolve`]: open_media_core::ports::StreamResolver::resolve
    /// [`StreamResolver::cleanup`]: open_media_core::ports::StreamResolver::cleanup
    /// [`Player::play`]: open_media_core::ports::Player::play
    /// [`Enricher`]: open_media_core::ports::Enricher
    /// [`Tracker::update_progress`]: open_media_core::ports::Tracker::update_progress
    pub async fn play(&self, req: &PlayRequest, candidate: &SourceCandidate) -> CoreResult<()> {
        // Single chosen candidate: the no-fallback entry point. Delegates to the
        // fallback path with a one-element list so both share the binge logic.
        self.play_with_fallback(req, std::slice::from_ref(candidate))
            .await
    }

    /// Like [`Engine::play`] but with **across-candidate failover**: try the
    /// ranked candidates in order, and when resolving or launching the chosen one
    /// fails, fall through to the next [resolvable](SourceCandidate::is_resolvable)
    /// candidate instead of aborting. This is the layer *above* the
    /// [`StreamResolver`]'s own intra-candidate debrid→P2P fallback: it survives a
    /// candidate that can't be resolved at all (dead torrent, debrid rejects the
    /// hash), not just one transport that's down.
    ///
    /// `candidates` is expected pre-ranked (as [`Engine::find_sources`] returns
    /// it); non-resolvable entries are skipped. Returns the last failure if no
    /// candidate could be played, or `NoSource` if the list had nothing
    /// resolvable.
    ///
    /// [`StreamResolver`]: open_media_core::ports::StreamResolver
    pub async fn play_with_fallback(
        &self,
        req: &PlayRequest,
        candidates: &[SourceCandidate],
    ) -> CoreResult<()> {
        // Play the first episode, falling through candidates on failure.
        let first = self.play_episode_with_fallback(req, candidates).await?;
        if first.playlist_autoplay {
            return Ok(());
        }

        // Binge: when enabled and the episode was actually watched to completion,
        // keep advancing to the next (non-filler) episode until we run out of
        // episodes, sources, or the viewer quits early. An owned loop (not
        // recursion) keeps the stack flat across an arbitrarily long binge.
        if !self.autoplay_next || !req.media.kind.is_episodic() || !first.completed {
            return Ok(());
        }

        // Filler/recap episode numbers, fetched at most once for the whole binge.
        let filler = self.filler_episodes(&req.media.ids).await;

        let mut current = req.clone();
        loop {
            let Some(next) = self.next_request(&current, &filler).await else {
                break;
            };
            // The whole ranked candidate list for the next episode, so a failed
            // resolve falls through to the next source rather than ending the
            // binge on a single dud release.
            let next_candidates = self.pick_candidates(&next).await;
            if next_candidates.is_empty() {
                // No playable source for the next episode — stop rather than skip
                // a gap silently; the viewer can resume manually.
                break;
            }
            match self
                .play_episode_with_fallback(&next, &next_candidates)
                .await
            {
                // Quit / low-progress exit ends the binge.
                Ok(outcome) if !outcome.completed || outcome.playlist_autoplay => break,
                Ok(_) => {}
                // Every candidate for this episode failed to play. Stop the binge
                // here rather than aborting the whole session with an error — the
                // viewer keeps what they watched and can resume manually.
                Err(e) => {
                    tracing::warn!(error = %e, "no playable source for next episode; ending binge");
                    break;
                }
            }
            current = next;
        }
        Ok(())
    }

    /// Play a single episode, trying each resolvable candidate in order until one
    /// plays through. Returns whether the played episode crossed the completion
    /// threshold (the binge loop's advance signal). Errors only when *every*
    /// resolvable candidate failed to resolve/launch; the last such error is
    /// returned (or [`CoreError::NoSource`] when the list had nothing resolvable).
    async fn play_episode_with_fallback(
        &self,
        req: &PlayRequest,
        candidates: &[SourceCandidate],
    ) -> CoreResult<PlayOnceOutcome> {
        let mut last_err: Option<CoreError> = None;
        let mut tried = 0usize;
        for candidate in candidates.iter().filter(|c| c.is_resolvable()) {
            tried += 1;
            match self.play_once(req, candidate).await {
                Ok(outcome) => return Ok(outcome),
                Err(e) => {
                    tracing::warn!(
                        provider = %candidate.provider,
                        error = %e,
                        "candidate failed to play; trying next source"
                    );
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            if tried == 0 {
                CoreError::NoSource("no resolvable candidate to play".into())
            } else {
                CoreError::NoSource("all candidates failed to play".into())
            }
        }))
    }

    /// Resolve a chosen candidate and play it through once. Returns whether the
    /// episode crossed [`Self::complete_threshold`] (i.e. was actually watched to
    /// the end, as opposed to a quit at low progress) — the binge loop uses this
    /// to decide whether to advance.
    async fn play_once(
        &self,
        req: &PlayRequest,
        candidate: &SourceCandidate,
    ) -> CoreResult<PlayOnceOutcome> {
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
        self.mark_library_started(req, season, episode);

        let progress = ProgressMeter::starting_at(resume.unwrap_or(0));

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
            image: req
                .episode_still
                .clone()
                .or_else(|| req.media.poster.clone()),
            chapter_skip_fallback: self.enricher.is_some(),
        };
        if self.autoplay_next && req.media.kind.is_episodic() {
            if let Some(playlist) = session.playlist_control() {
                let completed = self
                    .monitor_playlist_session(
                        &mut session,
                        playlist,
                        PlaylistEntry {
                            req: req.clone(),
                            ctx,
                            skip,
                        },
                        progress.clone(),
                    )
                    .await?;
                cleanup_subtitle_files(&subtitle_files);
                if let Some(resolver) = &self.resolver {
                    resolver.cleanup().await;
                }
                if let Some(presence) = &self.presence {
                    let _ = presence.clear().await;
                }
                return Ok(PlayOnceOutcome {
                    completed,
                    playlist_autoplay: true,
                });
            }
        }
        let monitor = monitor_playback(
            session.control(),
            self.history.clone(),
            self.presence.clone(),
            skip,
            ctx,
            progress.clone(),
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
        let (pos, dur) = progress.snapshot();
        let completed = dur > 0 && (pos as f32 / dur as f32) >= self.complete_threshold;
        if completed {
            if let Some(tracker) = &self.tracker {
                if let Err(e) = tracker.update_progress(&req.media.ids, episode).await {
                    tracing::warn!(error = %e, "tracker progress update failed");
                }
            }
        }
        self.mark_library_after_progress(req, pos, dur, completed);
        if let Some(presence) = &self.presence {
            let _ = presence.clear().await;
        }

        Ok(PlayOnceOutcome {
            completed,
            playlist_autoplay: false,
        })
    }

    /// Filler/recap episode numbers to skip when bingeing, or an empty set when
    /// `skip_filler` is off, no [`Enricher`] is wired, or the lookup fails.
    /// Best-effort: a filler-list failure must never abort a binge.
    ///
    /// [`Enricher`]: open_media_core::ports::Enricher
    pub(crate) async fn filler_episodes(&self, ids: &IdSet) -> Vec<u32> {
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
    pub(crate) async fn next_request(
        &self,
        current: &PlayRequest,
        filler: &[u32],
    ) -> Option<PlayRequest> {
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
        let (episode_title, episode_runtime_minutes, episode_still) = episodes
            .and_then(|eps| eps.into_iter().find(|ep| ep.number == next))
            .map(|ep| (ep.title, ep.runtime_minutes, ep.still))
            .unwrap_or((None, None, None));

        Some(PlayRequest {
            media: current.media.clone(),
            season: Some(season),
            episode: Some(next),
            episode_title,
            episode_still,
            episode_runtime_minutes,
            include_uncached: current.include_uncached,
        })
    }

    pub(crate) async fn resolve_first_playback(&self, req: &PlayRequest) -> Option<Playback> {
        for candidate in self.pick_candidates(req).await {
            match self.resolve(&candidate).await {
                Ok(playback) => return Some(playback),
                Err(e) => tracing::warn!(error = %e, "next episode source failed; trying fallback"),
            }
        }
        None
    }

    pub(crate) async fn mark_completed_if_needed(
        &self,
        req: &PlayRequest,
        pos: u32,
        dur: u32,
    ) -> bool {
        let completed = dur > 0 && (pos as f32 / dur as f32) >= self.complete_threshold;
        if completed {
            if let (Some(tracker), Some(episode)) = (&self.tracker, req.episode) {
                if let Err(e) = tracker.update_progress(&req.media.ids, episode).await {
                    tracing::warn!(error = %e, "tracker progress update failed");
                }
            }
        }
        self.mark_library_after_progress(req, pos, dur, completed);
        completed
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
    ///
    /// [`SubtitleProvider`]: open_media_core::ports::SubtitleProvider
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
pub(crate) struct MonitorCtx {
    pub(crate) media_key: String,
    pub(crate) season: u32,
    pub(crate) episode: u32,
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) image: Option<String>,
    /// When external skip data (AniSkip) is empty, derive OP/ED windows from
    /// the file's own embedded chapter names. Mirrors the auto-skip intent:
    /// set when the enricher is wired (`behavior.skip_intro_outro`).
    pub(crate) chapter_skip_fallback: bool,
}

struct PlayOnceOutcome {
    completed: bool,
    playlist_autoplay: bool,
}

/// Last-seen playback position/duration, written by a monitor task and read by
/// the caller after the player exits (for completion + resume bookkeeping).
#[derive(Clone)]
pub(crate) struct ProgressMeter {
    pub(crate) pos: Arc<AtomicU32>,
    pub(crate) dur: Arc<AtomicU32>,
}

impl ProgressMeter {
    pub(crate) fn starting_at(pos: u32) -> Self {
        Self {
            pos: Arc::new(AtomicU32::new(pos)),
            dur: Arc::new(AtomicU32::new(0)),
        }
    }

    /// The last observed `(position, duration)` in seconds.
    pub(crate) fn snapshot(&self) -> (u32, u32) {
        (
            self.pos.load(Ordering::Relaxed),
            self.dur.load(Ordering::Relaxed),
        )
    }
}

/// Poll the player's IPC channel ~1×/s: auto-skip OP/ED, persist progress, and
/// push presence. Returns only if there is no control channel (it then waits
/// forever, deferring to the player-exit branch of the caller's `select!`).
async fn monitor_playback(
    control: Option<Arc<dyn PlaybackControl>>,
    history: Option<Arc<dyn HistoryStore>>,
    presence: Option<Arc<dyn PresenceReporter>>,
    mut skip: SkipTimes,
    ctx: MonitorCtx,
    progress: ProgressMeter,
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
    // Probe the file's embedded chapters once (after metadata is loaded) when
    // no external skip data exists.
    let mut chapter_probe_pending = ctx.chapter_skip_fallback && skip.is_empty();

    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let pos = ctrl.position().await.ok().flatten().unwrap_or(0);
        let dur = ctrl.duration().await.ok().flatten().unwrap_or(0);
        if pos > 0 {
            progress.pos.store(pos, Ordering::Relaxed);
        }
        if dur > 0 {
            progress.dur.store(dur, Ordering::Relaxed);
        }

        if chapter_probe_pending && dur > 0 {
            chapter_probe_pending = false;
            if let Ok(embedded) = ctrl.chapters().await {
                let derived = skip_from_chapters(&embedded, Some(dur));
                if !derived.is_empty() {
                    tracing::debug!("derived OP/ED skip windows from embedded chapters");
                    skip = derived;
                }
            }
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

/// OP/ED windows must look like actual credits sequences to be trusted for
/// auto-skipping: standard openings/endings run ~90s; anything outside this
/// range is more likely a mislabeled or coarse chapter, and a bad auto-skip is
/// far worse than no auto-skip.
const CHAPTER_SKIP_MIN_SECS: u32 = 10;
const CHAPTER_SKIP_MAX_SECS: u32 = 180;

/// Derive OP/ED skip windows from a file's **embedded chapter names** — the
/// fallback for episodes AniSkip's community database doesn't cover. Anime
/// releases commonly ship chapters titled `OP`/`Opening`/`Intro` and
/// `ED`/`Ending`/`Outro`/`Credits`; each window runs from that chapter's start
/// to the next chapter (or the end of the file), and is only trusted when its
/// length is plausible for a credits sequence. Matching is token-based so
/// prose titles ("Operation Z") never false-positive.
pub(crate) fn skip_from_chapters(chapters: &[Chapter], duration_secs: Option<u32>) -> SkipTimes {
    let mut sorted: Vec<&Chapter> = chapters.iter().collect();
    sorted.sort_by_key(|c| c.time_secs);

    let mut skip = SkipTimes::default();
    for (i, chapter) in sorted.iter().enumerate() {
        let end = sorted
            .get(i + 1)
            .map(|next| next.time_secs)
            .or(duration_secs)
            .unwrap_or(chapter.time_secs);
        let interval = Interval {
            start: chapter.time_secs,
            end,
        };
        let len = interval.end.saturating_sub(interval.start);
        if !(CHAPTER_SKIP_MIN_SECS..=CHAPTER_SKIP_MAX_SECS).contains(&len) {
            continue;
        }
        let lower = chapter.title.to_lowercase();
        let mut tokens = lower.split(|c: char| !c.is_ascii_alphanumeric());
        if skip.opening.is_none()
            && tokens
                .clone()
                .any(|t| matches!(t, "op" | "opening" | "intro"))
        {
            skip.opening = Some(interval);
        } else if skip.ending.is_none()
            && tokens.any(|t| matches!(t, "ed" | "ending" | "outro" | "credits"))
        {
            skip.ending = Some(interval);
        }
    }
    skip
}

pub(crate) fn chapters_from_skip(skip: &SkipTimes) -> Vec<Chapter> {
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
