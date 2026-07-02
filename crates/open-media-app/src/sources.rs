use open_media_core::error::CoreResult;
use open_media_core::ports::SourceQuery;
use open_media_core::scoring;
use open_media_core::stream::SourceCandidate;

use crate::{Engine, PlayRequest};

impl Engine {
    /// Find playable candidates across every applicable source provider
    /// concurrently, merge, and rank them with [`scoring`]. Providers that do not
    /// support the media kind (e.g. nyaa for a live-action movie) are skipped.
    pub async fn find_sources(&self, req: &PlayRequest) -> CoreResult<Vec<SourceCandidate>> {
        let absolute_episode = self.absolute_episode(req).await;
        // Enrich anime with an IMDB id (best-effort) *before* the query is built,
        // so the IMDB-keyed providers (Torrentio → debrid) light up for anime
        // that AniList only knows by anilist/mal id. A no-op for everything that
        // already has an imdb id or when no bridge is wired.
        let mut media = req.media.clone();
        self.enrich_imdb(&mut media).await;
        let query = SourceQuery {
            media,
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
