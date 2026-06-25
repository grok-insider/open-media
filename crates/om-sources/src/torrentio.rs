//! Torrentio (Stremio addon) source adapter — all media kinds.
//!
//! One request returns ranked streams across every configured tracker (incl.
//! `nyaasi`). With a debrid token in the config string, cached results carry a
//! `[RD+]`/`⚡` flag and a direct URL; otherwise we get infohashes for P2P.
//! Keyed by IMDB id (movies/series via TMDB).

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::model::MediaKind;
use om_core::ports::{SourceProvider, SourceQuery};
use om_core::stream::{CacheState, SourceCandidate};
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
            client: Client::new(),
            config_string: config_string.into(),
            base_url: DEFAULT_BASE.to_string(),
        }
    }

    pub fn with_base_url(config_string: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            config_string: config_string.into(),
            base_url: base_url.into(),
        }
    }

    fn build_url(&self, query: &SourceQuery) -> CoreResult<String> {
        let imdb = query
            .media
            .ids
            .imdb
            .as_deref()
            .ok_or_else(|| CoreError::NoSource("torrentio requires an IMDB id".into()))?;

        let path = if query.media.kind == MediaKind::Movie {
            format!("stream/movie/{imdb}.json")
        } else {
            let season = query.season.unwrap_or(1);
            let episode = query.episode.unwrap_or(1);
            format!("stream/series/{imdb}:{season}:{episode}.json")
        };
        Ok(format!("{}/{}/{}", self.base_url, self.config_string, path))
    }
}

#[async_trait]
impl SourceProvider for TorrentioSource {
    fn name(&self) -> &str {
        "torrentio"
    }

    async fn find(&self, query: &SourceQuery) -> CoreResult<Vec<SourceCandidate>> {
        let url = self.build_url(query)?;
        tracing::debug!(%url, "torrentio request");

        let resp = self.client.get(&url).send().await.map_err(|e| {
            if e.is_timeout() {
                CoreError::Timeout(format!("torrentio: {e}"))
            } else {
                CoreError::Network(format!("torrentio: {e}"))
            }
        })?;
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

        let mut out = Vec::with_capacity(body.streams.len());
        for s in body.streams {
            out.push(s.into_candidate());
        }
        Ok(out)
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
