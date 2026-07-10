use open_media_core::error::CoreResult;
use open_media_core::ports::SourceQuery;
use open_media_core::scoring;
use open_media_core::stream::{CacheState, SourceCandidate};

use crate::{Engine, PlayRequest};

impl Engine {
    /// Find playable candidates across every applicable source provider
    /// concurrently, merge, and rank them with [`scoring`]. Providers that do not
    /// support the media kind (e.g. nyaa for a live-action movie) are skipped.
    pub async fn find_sources(&self, req: &PlayRequest) -> CoreResult<Vec<SourceCandidate>> {
        let absolute_episode = self.absolute_episode(req).await;
        // Bridge anime ids (best-effort) *before* the query is built, so the
        // IMDB-keyed providers (Torrentio → debrid) light up for anime that
        // AniList only knows by anilist/mal id — including the kitsu id for
        // native anime addressing and the real IMDB season for bridged
        // later-season entries. A no-op for everything non-anime or when no
        // bridge is wired.
        let mut media = req.media.clone();
        let bridged = self.bridge_anime_ids(&mut media).await;
        let query = SourceQuery {
            media,
            season: req.season,
            episode: req.episode,
            absolute_episode,
            kitsu: bridged.as_ref().and_then(|b| b.kitsu),
            imdb_season: bridged
                .as_ref()
                .and_then(|b| b.imdb_season)
                .and_then(|s| u32::try_from(s).ok()),
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

        // Drop mis-tagged indexer junk (e.g. Torrentio returning a multi-file
        // Handmaid's Tale pack under Mushoku's IMDB id). Must run *before* rank
        // so a debrid-cached wrong title cannot dominate via prefer_cached.
        let dropped = scoring::filter_title_mismatch(&mut candidates, query.media.display_title());
        if dropped > 0 {
            tracing::debug!(
                dropped,
                media = %query.media.display_title(),
                "dropped source candidates with no title-token overlap"
            );
        }

        // Prefer cached-only when the user opted out of uncached, but never
        // collapse to an empty list after title filtering removed the only
        // (wrong) cached hit — fall back to uncached so something playable
        // remains.
        if !req.include_uncached {
            let cached: Vec<SourceCandidate> = candidates
                .iter()
                .filter(|c| c.cache == CacheState::Cached)
                .cloned()
                .collect();
            if !cached.is_empty() {
                candidates = cached;
            } else if !candidates.is_empty() {
                tracing::debug!(
                    media = %query.media.display_title(),
                    "no title-matching cached sources; keeping uncached"
                );
            }
        }

        scoring::rank(&mut candidates, &self.prefs);
        Ok(candidates)
    }

    /// Find + rank sources for a request and return the resolvable candidates in
    /// ranked order, reusing the same ranking [`Engine::find_sources`] applies.
    /// Empty when the lookup fails or nothing is resolvable. The caller plays
    /// these with failover (first that resolves+plays wins).
    pub(crate) async fn pick_candidates(&self, req: &PlayRequest) -> Vec<SourceCandidate> {
        self.find_sources(req)
            .await
            .map(|cands| {
                cands
                    .into_iter()
                    .filter(|c| c.is_resolvable())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }
}
