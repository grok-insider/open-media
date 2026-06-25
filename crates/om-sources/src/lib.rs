//! # om-sources
//!
//! [`SourceProvider`] adapters — they find releasable files for a media item.
//!
//! - [`TorrentioSource`] — the Stremio Torrentio addon (movies, series, anime).
//!   Cached results carry direct debrid URLs + `[RD+]`/`⚡` flags.
//! - [`NyaaSource`] — direct nyaa.si (RSS) for anime, independent of Torrentio.
//!
//! Release-name parsing (quality/codec/HDR/audio/language, cache/seeders/size)
//! lives in [`tags`] because the formats are provider-specific; the parsed shape
//! ([`om_core::stream::ReleaseTags`]) is shared so scoring stays format-agnostic.
//!
//! [`SourceProvider`]: om_core::ports::SourceProvider

pub mod nyaa;
pub mod season;
pub mod tags;
pub mod torrentio;

pub use nyaa::NyaaSource;
pub use season::{parse_title_season, release_season, SeasonMatch};
pub use tags::{parse_release_name, parse_size_to_bytes, parse_torrentio, ParsedRelease};
pub use torrentio::TorrentioSource;
