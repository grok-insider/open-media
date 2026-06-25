//! # om-metadata
//!
//! [`MetadataProvider`] adapters.
//!
//! - [`TmdbProvider`] — The Movie Database (v3 API). Primary discovery for movies
//!   and live-action series; hydrates IMDB ids (needed by Torrentio/Comet).
//! - [`AniListProvider`] — AniList GraphQL. Anime discovery + MAL-id bridging
//!   (needed by AniSkip and MAL tracking).
//!
//! Both take an injectable base URL so integration tests can point them at a
//! mock server (see `tests/`).
//!
//! [`MetadataProvider`]: om_core::ports::MetadataProvider

pub mod anilist;
pub mod tmdb;

pub use anilist::AniListProvider;
pub use tmdb::TmdbProvider;

use om_core::error::CoreError;

/// Map a transport error into the right [`CoreError`] category.
pub(crate) fn map_net(service: &str, e: reqwest::Error) -> CoreError {
    if e.is_timeout() {
        CoreError::Timeout(format!("{service}: {e}"))
    } else {
        CoreError::Network(format!("{service}: {e}"))
    }
}
