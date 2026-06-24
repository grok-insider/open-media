//! # om-sources
//!
//! [`SourceProvider`] adapters — they find releasable files for a media item.
//!
//! - [`TorrentioSource`] — the Stremio Torrentio addon. One request returns
//!   ranked streams across every tracker (incl. `nyaasi`), and — when a debrid
//!   token is in the config string — pre-marks cached results (`[RD+]`/`⚡`) and
//!   yields direct URLs. Covers movies, series, and anime.
//! - [`NyaaSource`] — direct nyaa.si (RSS feed) for anime. Independent of
//!   Torrentio so anime keeps working if the addon is down, and exposes
//!   nyaa-only releases. Magnet/infohash only (resolution happens later).
//!
//! Scaffold stubs: the [`SourceProvider`] contract is implemented; bodies return
//! [`CoreError::NotImplemented`] until Phase 2 (see `docs/ROADMAP.md`).
//!
//! Title/filename → [`ReleaseTags`] parsing (quality, codec, HDR, audio,
//! language) lives in this crate because the formats are provider-specific; the
//! parsed shape is shared via `om-core`.
//!
//! [`SourceProvider`]: om_core::ports::SourceProvider
//! [`CoreError::NotImplemented`]: om_core::error::CoreError::NotImplemented
//! [`ReleaseTags`]: om_core::stream::ReleaseTags

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::model::MediaKind;
use om_core::ports::{SourceProvider, SourceQuery};
use om_core::stream::SourceCandidate;

/// Torrentio addon source (all media kinds).
pub struct TorrentioSource {
    #[allow(dead_code)]
    config_string: String,
}

impl TorrentioSource {
    /// `config_string` is the Torrentio path segment, e.g.
    /// `providers=...|sort=qualitysize|debridoptions=nodownloadlinks|realdebrid=KEY`.
    pub fn new(config_string: impl Into<String>) -> Self {
        Self {
            config_string: config_string.into(),
        }
    }
}

#[async_trait]
impl SourceProvider for TorrentioSource {
    fn name(&self) -> &str {
        "torrentio"
    }

    async fn find(&self, _query: &SourceQuery) -> CoreResult<Vec<SourceCandidate>> {
        Err(CoreError::NotImplemented("torrentio.find"))
    }
}

/// Direct nyaa.si source (anime only).
pub struct NyaaSource {
    /// Optional mirror/proxy base (e.g. a nyaa proxy or `sukebei.nyaa.si`).
    #[allow(dead_code)]
    base_url: String,
}

impl NyaaSource {
    pub fn new() -> Self {
        Self {
            base_url: "https://nyaa.si".to_string(),
        }
    }

    pub fn with_base(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }
}

impl Default for NyaaSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SourceProvider for NyaaSource {
    fn name(&self) -> &str {
        "nyaa"
    }

    /// nyaa is anime-only; live-action movies/series are out of scope.
    fn supports(&self, kind: MediaKind) -> bool {
        matches!(kind, MediaKind::Anime)
    }

    async fn find(&self, _query: &SourceQuery) -> CoreResult<Vec<SourceCandidate>> {
        Err(CoreError::NotImplemented("nyaa.find"))
    }
}
