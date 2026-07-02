use futures::stream::{FuturesUnordered, StreamExt};
use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::{Media, MediaKind};

use crate::{Engine, SearchProgress};

impl Engine {
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

    /// Search metadata providers concurrently and report deduplicated snapshots as
    /// each provider finishes.
    ///
    /// This is intended for interactive UIs. Unlike [`Self::search`], result order
    /// follows provider completion so the first visible rows can render
    /// immediately. Once a row appears it is not moved: later duplicates sharing an
    /// IMDB id only donate missing ids to the first-seen row.
    pub async fn search_incremental<F>(
        &self,
        query: &str,
        kind: Option<MediaKind>,
        mut on_progress: F,
    ) -> CoreResult<Vec<Media>>
    where
        F: FnMut(SearchProgress),
    {
        if self.metadata.is_empty() {
            return Err(CoreError::Config("no metadata providers configured".into()));
        }

        let total = self.metadata.len();
        let mut calls = self
            .metadata
            .iter()
            .map(|provider| async move { (provider.name(), provider.search(query, kind).await) })
            .collect::<FuturesUnordered<_>>();
        let mut acc = SearchAccumulator::default();
        let mut completed = 0usize;
        let mut failed = 0usize;

        while let Some((name, result)) = calls.next().await {
            completed += 1;
            match result {
                Ok(items) => acc.extend(items),
                Err(e) => {
                    failed += 1;
                    tracing::warn!(provider = name, error = %e, "metadata search failed");
                }
            }
            on_progress(SearchProgress {
                results: acc.results().to_vec(),
                failed_providers: failed,
                finished: completed == total,
            });
        }

        Ok(acc.into_results())
    }
}

/// Collapse provider results that share an IMDB id, preserving order.
///
/// Cinemeta and TMDB both resolve a live-action title to the same `tt…` id; a
/// keyed user would otherwise see each movie/series twice. First occurrence wins
/// (provider order in the builder is the priority); later duplicates only donate
/// ids the kept entry lacks. Items without an IMDB id (e.g. AniList anime) are
/// never collapsed.
pub(crate) fn dedup_by_imdb(items: Vec<Media>) -> Vec<Media> {
    let mut acc = SearchAccumulator::with_capacity(items.len());
    acc.extend(items);
    acc.into_results()
}

/// Incremental IMDB-keyed deduplicator. First visible row wins to avoid list
/// jumping; duplicate rows only merge ids into that row.
#[derive(Default)]
struct SearchAccumulator {
    out: Vec<Media>,
    seen: std::collections::HashMap<String, usize>,
}

impl SearchAccumulator {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            out: Vec::with_capacity(capacity),
            seen: std::collections::HashMap::new(),
        }
    }

    fn extend(&mut self, items: impl IntoIterator<Item = Media>) {
        for item in items {
            self.push(item);
        }
    }

    fn push(&mut self, item: Media) {
        if let Some(imdb) = item.ids.imdb.clone() {
            if let Some(&idx) = self.seen.get(&imdb) {
                self.out[idx].ids.merge(&item.ids);
                return;
            }
            self.seen.insert(imdb, self.out.len());
        }
        self.out.push(item);
    }

    fn results(&self) -> &[Media] {
        &self.out
    }

    fn into_results(self) -> Vec<Media> {
        self.out
    }
}
