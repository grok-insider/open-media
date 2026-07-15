//! # open-media-sources
//!
//! [`SourceProvider`] adapters — they find releasable files for a media item.
//!
//! - [`TorrentioSource`] — the Stremio Torrentio addon (movies, series, anime).
//!   Cached results carry direct debrid URLs + `[RD+]`/`⚡` flags.
//! - [`NyaaSource`] — direct nyaa.si (RSS) for anime, independent of Torrentio.
//!
//! Release-name parsing (quality/codec/HDR/audio/language, cache/seeders/size)
//! lives in [`tags`] because the formats are provider-specific; the parsed shape
//! ([`open_media_core::stream::ReleaseTags`]) is shared so scoring stays format-agnostic.
//!
//! [`SourceProvider`]: open_media_core::ports::SourceProvider

pub mod nyaa;
pub mod season;
pub mod tags;
pub mod torrentio;

pub use nyaa::NyaaSource;
pub use season::{
    discover_max_episode, parse_title_season, release_episode, release_season,
    resolve_season_ordinal, EpisodeMatch, SeasonMatch,
};
pub use tags::{parse_release_name, parse_size_to_bytes, parse_torrentio, ParsedRelease};
pub use torrentio::TorrentioSource;
