//! # om-metadata
//!
//! [`MetadataProvider`] adapters.
//!
//! - [`TmdbProvider`] — The Movie Database (v3 API). Primary discovery for movies
//!   and live-action series; yields IMDB ids (needed by Torrentio/Comet).
//! - [`AniListProvider`] — AniList GraphQL. Anime discovery + MAL-id bridging
//!   (needed by AniSkip and MAL tracking).
//!
//! Both are scaffold stubs: the trait is implemented end-to-end so the contract
//! is compile-checked, with method bodies returning [`CoreError::NotImplemented`]
//! until Phase 1 (see `docs/ROADMAP.md`).
//!
//! [`MetadataProvider`]: om_core::ports::MetadataProvider
//! [`CoreError::NotImplemented`]: om_core::error::CoreError::NotImplemented

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::model::{Episode, IdSet, Media, MediaKind, Season};
use om_core::ports::MetadataProvider;

/// TMDB-backed metadata provider.
pub struct TmdbProvider {
    #[allow(dead_code)]
    api_key: String,
}

impl TmdbProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }
}

#[async_trait]
impl MetadataProvider for TmdbProvider {
    fn name(&self) -> &str {
        "tmdb"
    }

    async fn search(&self, _query: &str, _kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        Err(CoreError::NotImplemented("tmdb.search"))
    }

    async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
        Err(CoreError::NotImplemented("tmdb.details"))
    }

    async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
        Err(CoreError::NotImplemented("tmdb.seasons"))
    }

    async fn episodes(&self, _ids: &IdSet, _season: u32) -> CoreResult<Vec<Episode>> {
        Err(CoreError::NotImplemented("tmdb.episodes"))
    }
}

/// AniList-backed metadata provider (anime).
pub struct AniListProvider;

impl AniListProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AniListProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MetadataProvider for AniListProvider {
    fn name(&self) -> &str {
        "anilist"
    }

    async fn search(&self, _query: &str, _kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        Err(CoreError::NotImplemented("anilist.search"))
    }

    async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
        Err(CoreError::NotImplemented("anilist.details"))
    }

    async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
        Err(CoreError::NotImplemented("anilist.seasons"))
    }

    async fn episodes(&self, _ids: &IdSet, _season: u32) -> CoreResult<Vec<Episode>> {
        Err(CoreError::NotImplemented("anilist.episodes"))
    }
}
