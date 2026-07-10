use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::{Media, MediaKind};
use open_media_core::tracking::{LibraryItem, ListStatus};

use crate::{Engine, PlayRequest};

impl Engine {
    /// List locally persisted library items, optionally filtered by status.
    pub fn list_library(&self, status: Option<ListStatus>) -> CoreResult<Vec<LibraryItem>> {
        let store = self
            .library
            .as_ref()
            .ok_or_else(|| CoreError::Config("no library store configured".into()))?;
        store.list(status)
    }

    /// Add or update a local library entry and best-effort sync the status to any
    /// configured remote tracker.
    pub async fn set_library_status(
        &self,
        media: &Media,
        status: ListStatus,
    ) -> CoreResult<LibraryItem> {
        let item = self.library_item(media, status, None, None, 0, 0);
        self.save_library_item(&item)?;
        if let Some(tracker) = &self.tracker {
            if let Err(e) = tracker.set_status(&media.ids, status).await {
                tracing::warn!(error = %e, "tracker status sync failed");
            }
        }
        Ok(item)
    }

    pub(crate) fn mark_library_started(&self, req: &PlayRequest, season: u32, episode: u32) {
        let item = self.library_item(
            &req.media,
            ListStatus::Watching,
            req.media.kind.is_episodic().then_some(season),
            req.media.kind.is_episodic().then_some(episode),
            0,
            0,
        );
        if let Err(e) = self.save_library_item(&item) {
            tracing::debug!(error = %e, "local library start update failed");
        }
    }

    pub(crate) fn mark_library_after_progress(
        &self,
        req: &PlayRequest,
        pos: u32,
        dur: u32,
        completed: bool,
    ) {
        let status = if completed && self.media_completed_by_coordinate(req) {
            ListStatus::Completed
        } else {
            ListStatus::Watching
        };
        let item = self.library_item(
            &req.media,
            status,
            req.media
                .kind
                .is_episodic()
                .then_some(req.season.unwrap_or(1)),
            req.media
                .kind
                .is_episodic()
                .then_some(req.episode.unwrap_or(1)),
            pos,
            dur,
        );
        if let Err(e) = self.save_library_item(&item) {
            tracing::debug!(error = %e, "local library progress update failed");
        }
    }

    fn media_completed_by_coordinate(&self, req: &PlayRequest) -> bool {
        if !req.media.kind.is_episodic() {
            return true;
        }
        let Some(last_episode) = req.media.episode_count else {
            return false;
        };
        let season_done = req
            .media
            .season_count
            .is_none_or(|count| req.season.unwrap_or(1) >= count);
        season_done && req.episode.unwrap_or(1) >= last_episode
    }

    fn library_item(
        &self,
        media: &Media,
        status: ListStatus,
        last_season: Option<u32>,
        last_episode: Option<u32>,
        position_secs: u32,
        duration_secs: u32,
    ) -> LibraryItem {
        LibraryItem {
            media_key: media
                .ids
                .primary_key()
                .unwrap_or_else(|| media.display_title().to_string()),
            ids: media.ids.clone(),
            title: media.display_title().to_string(),
            kind: media.kind,
            poster: media.poster.clone(),
            year: media.year,
            status,
            last_season,
            last_episode,
            position_secs,
            duration_secs,
            updated_at: unix_now(),
        }
    }

    fn save_library_item(&self, item: &LibraryItem) -> CoreResult<()> {
        let store = self
            .library
            .as_ref()
            .ok_or_else(|| CoreError::Config("no library store configured".into()))?;
        store.upsert(item)
    }

    /// If a library row already exists for `media`, update its `kind` (and fold
    /// any newly known ids) without resetting status or progress.
    ///
    /// Used when details reclassifies Series → Anime via the id bridge so Home
    /// and Library badges update without waiting for the next play session.
    pub fn sync_library_kind(&self, media: &Media) -> CoreResult<()> {
        let Some(store) = &self.library else {
            return Ok(());
        };
        let key = media
            .ids
            .primary_key()
            .unwrap_or_else(|| media.display_title().to_string());
        let items = store.list(None)?;
        let Some(mut item) = items.into_iter().find(|i| {
            i.media_key == key
                || (media.ids.imdb.is_some() && i.ids.imdb == media.ids.imdb)
                || (media.ids.anilist.is_some() && i.ids.anilist == media.ids.anilist)
        }) else {
            return Ok(());
        };
        if item.kind == media.kind
            && item.ids.imdb == media.ids.imdb
            && item.ids.anilist == media.ids.anilist
            && item.ids.mal == media.ids.mal
            && item.ids.tmdb == media.ids.tmdb
        {
            return Ok(());
        }
        item.kind = media.kind;
        item.ids.merge(&media.ids);
        // Prefer the hydrated title when the library row was sparse.
        if !media.title.is_empty() {
            item.title = media.display_title().to_string();
        }
        store.upsert(&item)
    }

    /// Walk the local library and upgrade Series/Movie → Anime when the id
    /// bridge reverse-maps the row into the anime catalog (Fribb).
    ///
    /// Intended for TUI boot so Home/Library badges are correct without the
    /// user opening every Cinemeta-sourced anime once. Never downgrades Anime.
    /// Returns how many rows were rewritten.
    pub async fn refine_library_kinds(&self) -> CoreResult<usize> {
        let Some(store) = &self.library else {
            return Ok(0);
        };
        let Some(bridge) = &self.id_bridge else {
            return Ok(0);
        };
        let items = store.list(None)?;
        let mut upgraded = 0usize;
        for mut item in items {
            if item.kind == MediaKind::Anime {
                continue;
            }
            // Need at least one dialect the bridge can look up.
            if item.ids.imdb.is_none()
                && item.ids.tmdb.is_none()
                && item.ids.anilist.is_none()
                && item.ids.mal.is_none()
            {
                continue;
            }
            match bridge.resolve(&item.ids).await {
                Ok(Some(_)) => {
                    tracing::debug!(
                        title = %item.title,
                        key = %item.media_key,
                        "library row reclassified as Anime via id bridge"
                    );
                    item.kind = MediaKind::Anime;
                    store.upsert(&item)?;
                    upgraded += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        key = %item.media_key,
                        "library kind refine bridge lookup failed"
                    );
                }
            }
        }
        Ok(upgraded)
    }
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
