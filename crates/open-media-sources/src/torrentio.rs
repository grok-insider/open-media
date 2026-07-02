//! Torrentio (Stremio addon) source adapter — all media kinds.
//!
//! One request returns ranked streams across every configured tracker (incl.
//! `nyaasi`). With a debrid token in the config string, cached results carry a
//! `[RD+]`/`⚡` flag and a direct URL; otherwise we get infohashes for P2P.
//! Keyed by IMDB id (movies/series via TMDB).

use async_trait::async_trait;
use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::MediaKind;
use open_media_core::ports::{SourceProvider, SourceQuery};
use open_media_core::stream::{CacheState, SourceCandidate};
use reqwest::Client;
use serde::Deserialize;

use crate::tags::parse_torrentio;

const DEFAULT_BASE: &str = "https://torrentio.strem.fun";

/// Torrentio addon source.
pub struct TorrentioSource {
    client: Client,
    config_string: String,
    base_url: String,
}

impl TorrentioSource {
    pub fn new(config_string: impl Into<String>) -> Self {
        Self {
            client: open_media_net::client(),
            config_string: config_string.into(),
            base_url: DEFAULT_BASE.to_string(),
        }
    }

    pub fn with_base_url(config_string: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            client: open_media_net::client(),
            config_string: config_string.into(),
            base_url: base_url.into(),
        }
    }

    /// The stream paths to try, most-specific first.
    ///
    /// Anime with a bridged **kitsu id** are addressed natively
    /// (`kitsu:{id}:{ep}`): kitsu mirrors AniList's per-entry numbering, so no
    /// season arithmetic is needed and franchises whose seasons all share one
    /// IMDB id resolve correctly. The IMDB path stays as a fallback (some
    /// releases are only indexed under the IMDB id) — using [`SourceQuery::
    /// imdb_season`] when present, because for a bridged later-season entry
    /// `season` is AniList's flat `1` while the release lives at the real one.
    fn stream_paths(&self, query: &SourceQuery) -> Vec<String> {
        let mut paths = Vec::new();
        let episodic = query.media.kind != MediaKind::Movie;
        if let Some(kitsu) = query.kitsu {
            if episodic {
                let episode = query.episode.unwrap_or(1);
                paths.push(format!("stream/series/kitsu:{kitsu}:{episode}.json"));
            } else {
                paths.push(format!("stream/movie/kitsu:{kitsu}.json"));
            }
        }
        if let Some(imdb) = query.media.ids.imdb.as_deref() {
            if episodic {
                let season = query.imdb_season.or(query.season).unwrap_or(1);
                let episode = query.episode.unwrap_or(1);
                paths.push(format!("stream/series/{imdb}:{season}:{episode}.json"));
            } else {
                paths.push(format!("stream/movie/{imdb}.json"));
            }
        }
        paths
    }

    async fn fetch_streams(&self, path: &str) -> CoreResult<Vec<StreamItem>> {
        let url = format!("{}/{}/{}", self.base_url, self.config_string, path);
        tracing::debug!(%url, "torrentio request");

        let resp = open_media_net::retry(|| async {
            self.client.get(&url).send().await.map_err(|e| {
                if e.is_timeout() {
                    CoreError::Timeout(format!("torrentio: {e}"))
                } else {
                    CoreError::Network(format!("torrentio: {e}"))
                }
            })
        })
        .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(CoreError::Remote {
                service: "torrentio".into(),
                message: format!("HTTP {status}"),
            });
        }
        let body: TorrentioResponse = resp.json().await.map_err(|e| CoreError::Parse {
            what: "torrentio response".into(),
            message: e.to_string(),
        })?;
        Ok(body.streams)
    }
}

#[async_trait]
impl SourceProvider for TorrentioSource {
    fn name(&self) -> &str {
        "torrentio"
    }

    async fn find(&self, query: &SourceQuery) -> CoreResult<Vec<SourceCandidate>> {
        // Torrentio is id-keyed (IMDB, or kitsu for anime). Titles with neither
        // contribute nothing — the anime-native providers (nyaa) serve those —
        // rather than erroring per call (which the engine would only log).
        let paths = self.stream_paths(query);
        if paths.is_empty() {
            tracing::debug!("torrentio: no IMDB/kitsu id; skipping (anime handled by nyaa)");
            return Ok(Vec::new());
        }

        // Try most-specific first (kitsu for anime), falling back to the next
        // addressing on an empty result. A hard failure on the *last* path is
        // surfaced; earlier ones only log (the fallback may still serve).
        let last = paths.len() - 1;
        for (i, path) in paths.iter().enumerate() {
            match self.fetch_streams(path).await {
                Ok(streams) if !streams.is_empty() => {
                    return Ok(streams.into_iter().map(|s| s.into_candidate()).collect());
                }
                Ok(_) => {
                    tracing::debug!(path, "torrentio: no streams; trying next addressing");
                }
                Err(e) if i < last => {
                    tracing::debug!(path, error = %e, "torrentio request failed; trying next addressing");
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Vec::new())
    }
}

#[derive(Debug, Deserialize)]
struct TorrentioResponse {
    #[serde(default)]
    streams: Vec<StreamItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    info_hash: Option<String>,
    #[serde(default)]
    file_idx: Option<usize>,
}

impl StreamItem {
    fn into_candidate(self) -> SourceCandidate {
        let parsed = parse_torrentio(&self.name, &self.title);
        // A direct URL implies a cached debrid stream even if the flag parsing
        // missed it.
        let cache = if self.url.is_some() && parsed.cache == CacheState::Unknown {
            CacheState::Cached
        } else {
            parsed.cache
        };
        let magnet = self
            .info_hash
            .as_ref()
            .map(|h| format!("magnet:?xt=urn:btih:{h}"));
        SourceCandidate {
            provider: parsed.provider,
            title: self.title,
            quality: parsed.quality,
            size_bytes: parsed.size_bytes,
            seeders: parsed.seeders,
            info_hash: self.info_hash,
            magnet,
            direct_url: self.url,
            file_index: self.file_idx,
            cache,
            tags: parsed.tags,
        }
    }
}
