//! # om-metadata
//!
//! [`MetadataProvider`] adapters.
//!
//! - [`CinemetaProvider`] — Stremio Cinemeta. **Keyless**, IMDB-native discovery
//!   for movies and live-action series; the default when no TMDB key is set.
//! - [`TmdbProvider`] — The Movie Database (v3 API). Richer movie/series metadata
//!   when an API key is configured; also hydrates IMDB ids (needed by
//!   Torrentio/Comet).
//! - [`AniListProvider`] — AniList GraphQL. Anime discovery + MAL-id bridging
//!   (needed by AniSkip and MAL tracking).
//!
//! All take an injectable base URL so integration tests can point them at a
//! mock server (see `tests/`).
//!
//! [`MetadataProvider`]: open_media_core::ports::MetadataProvider

pub mod anilist;
pub mod cinemeta;
mod jikan;
pub mod tmdb;

pub use anilist::AniListProvider;
pub use cinemeta::CinemetaProvider;
pub use tmdb::TmdbProvider;

use open_media_core::error::CoreError;

/// Map a transport error into the right [`CoreError`] category.
pub(crate) fn map_net(service: &str, e: reqwest::Error) -> CoreError {
    if e.is_timeout() {
        CoreError::Timeout(format!("{service}: {e}"))
    } else {
        CoreError::Network(format!("{service}: {e}"))
    }
}
