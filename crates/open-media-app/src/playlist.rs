use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use open_media_core::error::CoreResult;
use open_media_core::ports::{
    HistoryStore, PlaySession, PlaybackControl, PlaylistControl, PlaylistItem, PresenceReporter,
    Tracker,
};
use open_media_core::tracking::{Activity, SkipTimes, WatchProgress};

use crate::library::unix_now;
use crate::playback::{chapters_from_skip, MonitorCtx};
use crate::{Engine, PlayRequest};

impl Engine {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn monitor_playlist_session(
        &self,
        session: &mut Box<dyn PlaySession>,
        playlist: Arc<dyn PlaylistControl>,
        first_req: PlayRequest,
        first_ctx: MonitorCtx,
        first_skip: SkipTimes,
        last_pos: Arc<AtomicU32>,
        last_dur: Arc<AtomicU32>,
    ) -> CoreResult<bool> {
        let filler = self.filler_episodes(&first_req.media.ids).await;
        let completed = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(PlaylistMonitorState {
            entries: vec![PlaylistEntry {
                req: first_req,
                ctx: first_ctx,
                skip: first_skip,
            }],
            active: 0,
            appended_until: 0,
            marked_complete: Vec::new(),
        }));

        self.append_next_playlist_item(&playlist, &state, &filler)
            .await;

        let monitor = monitor_playlist_playback(
            session.control(),
            playlist.clone(),
            self.history.clone(),
            self.presence.clone(),
            self.tracker.clone(),
            self.complete_threshold,
            completed.clone(),
            state.clone(),
            last_pos.clone(),
            last_dur.clone(),
            self,
            filler,
        );

        let wait_result = tokio::select! {
            r = session.wait() => r,
            _ = monitor => Ok(()),
        };
        wait_result?;

        let (req, pos, dur) = {
            let guard = state.lock().unwrap();
            let active = guard.active.min(guard.entries.len().saturating_sub(1));
            (
                guard.entries[active].req.clone(),
                last_pos.load(Ordering::Relaxed),
                last_dur.load(Ordering::Relaxed),
            )
        };
        if self.mark_completed_if_needed(&req, pos, dur).await {
            completed.store(true, Ordering::Relaxed);
        }
        Ok(completed.load(Ordering::Relaxed))
    }

    async fn append_next_playlist_item(
        &self,
        playlist: &Arc<dyn PlaylistControl>,
        state: &Arc<Mutex<PlaylistMonitorState>>,
        filler: &[u32],
    ) {
        let current = {
            let guard = state.lock().unwrap();
            if guard.appended_until + 1 < guard.entries.len() {
                return;
            }
            guard.entries[guard.appended_until].req.clone()
        };
        let Some(next) = self.next_request(&current, filler).await else {
            return;
        };
        let Some(playback) = self.resolve_first_playback(&next).await else {
            return;
        };
        let title = Some(open_media_core::title::media_title(
            &next.media,
            next.season.unwrap_or(1),
            next.episode.unwrap_or(1),
            next.episode_title.as_deref(),
        ));
        if playlist
            .append(&PlaylistItem {
                url: playback.url,
                title: title.clone(),
            })
            .await
            .is_err()
        {
            return;
        }
        let season = next.season.unwrap_or(1);
        let episode = next.episode.unwrap_or(1);
        let skip = match &self.enricher {
            Some(e) => e
                .skip_times(
                    &next.media.ids,
                    episode,
                    next.episode_runtime_minutes.map(|m| m * 60),
                )
                .await
                .unwrap_or_default(),
            None => SkipTimes::default(),
        };
        let media_key = next
            .media
            .ids
            .primary_key()
            .unwrap_or_else(|| next.media.display_title().to_string());
        let ctx = MonitorCtx {
            media_key,
            season,
            episode,
            title: next.media.display_title().to_string(),
            detail: open_media_core::title::episode_detail(
                &next.media,
                season,
                episode,
                next.episode_title.as_deref(),
            ),
            image: next
                .episode_still
                .clone()
                .or_else(|| next.media.poster.clone()),
        };
        let mut guard = state.lock().unwrap();
        guard.entries.push(PlaylistEntry {
            req: next,
            ctx,
            skip,
        });
        guard.appended_until += 1;
    }
}

struct PlaylistEntry {
    req: PlayRequest,
    ctx: MonitorCtx,
    skip: SkipTimes,
}

struct PlaylistMonitorState {
    entries: Vec<PlaylistEntry>,
    active: usize,
    appended_until: usize,
    marked_complete: Vec<usize>,
}

#[allow(clippy::too_many_arguments)]
async fn monitor_playlist_playback(
    control: Option<Arc<dyn PlaybackControl>>,
    playlist: Arc<dyn PlaylistControl>,
    history: Option<Arc<dyn HistoryStore>>,
    presence: Option<Arc<dyn PresenceReporter>>,
    tracker: Option<Arc<dyn Tracker>>,
    complete_threshold: f32,
    any_completed: Arc<AtomicBool>,
    state: Arc<Mutex<PlaylistMonitorState>>,
    last_pos: Arc<AtomicU32>,
    last_dur: Arc<AtomicU32>,
    engine: &Engine,
    filler: Vec<u32>,
) {
    let Some(ctrl) = control else {
        std::future::pending::<()>().await;
        return;
    };

    let initial_chapters = {
        let guard = state.lock().unwrap();
        chapters_from_skip(&guard.entries[guard.active].skip)
    };
    if !initial_chapters.is_empty() {
        let _ = ctrl.set_chapters(&initial_chapters).await;
    }

    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let playlist_idx = playlist
            .active_index()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                let guard = state.lock().unwrap();
                guard.active
            });
        let active_changed = {
            let mut guard = state.lock().unwrap();
            let bounded = playlist_idx.min(guard.entries.len().saturating_sub(1));
            if bounded != guard.active {
                let previous = guard.active;
                guard.active = bounded;
                Some(previous)
            } else {
                None
            }
        };
        if let Some(previous) = active_changed {
            let (req, already_marked, pos, dur) = {
                let mut guard = state.lock().unwrap();
                let already = guard.marked_complete.contains(&previous);
                if !already {
                    guard.marked_complete.push(previous);
                }
                (
                    guard.entries[previous].req.clone(),
                    already,
                    last_pos.load(Ordering::Relaxed),
                    last_dur.load(Ordering::Relaxed),
                )
            };
            if !already_marked {
                let completed = dur > 0 && (pos as f32 / dur as f32) >= complete_threshold;
                if completed {
                    any_completed.store(true, Ordering::Relaxed);
                    if let (Some(t), Some(ep)) = (&tracker, req.episode) {
                        let _ = t.update_progress(&req.media.ids, ep).await;
                    }
                }
            }
            last_pos.store(0, Ordering::Relaxed);
            last_dur.store(0, Ordering::Relaxed);
            let chapters = {
                let guard = state.lock().unwrap();
                chapters_from_skip(&guard.entries[guard.active].skip)
            };
            let _ = ctrl.set_chapters(&chapters).await;
            let active_req = {
                let guard = state.lock().unwrap();
                guard.entries[guard.active].req.clone()
            };
            engine.mark_library_started(
                &active_req,
                active_req.season.unwrap_or(1),
                active_req.episode.unwrap_or(1),
            );
            engine
                .append_next_playlist_item(&playlist, &state, &filler)
                .await;
        }

        let pos = ctrl.position().await.ok().flatten().unwrap_or(0);
        let dur = ctrl.duration().await.ok().flatten().unwrap_or(0);
        if pos > 0 {
            last_pos.store(pos, Ordering::Relaxed);
        }
        if dur > 0 {
            last_dur.store(dur, Ordering::Relaxed);
        }

        let (ctx, skip) = {
            let guard = state.lock().unwrap();
            let entry = &guard.entries[guard.active];
            (
                MonitorCtx {
                    media_key: entry.ctx.media_key.clone(),
                    season: entry.ctx.season,
                    episode: entry.ctx.episode,
                    title: entry.ctx.title.clone(),
                    detail: entry.ctx.detail.clone(),
                    image: entry.ctx.image.clone(),
                },
                entry.skip,
            )
        };

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
                    title: ctx.title,
                    detail: ctx.detail,
                    paused,
                    position_secs: pos,
                    duration_secs: dur,
                    image_url: ctx.image,
                })
                .await;
        }
    }
}
