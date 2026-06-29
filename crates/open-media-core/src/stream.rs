//! Source candidates and resolved playback.
//!
//! A [`SourceCandidate`] is one releasable file a [`SourceProvider`] found
//! (a Torrentio stream, a nyaa torrent row, a Comet result). It is *not* yet
//! playable — a [`StreamResolver`] turns the chosen candidate into a
//! [`Playback`] (a concrete URL the [`Player`] can open), either by unrestricting
//! it through a debrid service or by serving it from the local P2P engine.
//!
//! [`SourceProvider`]: crate::ports::SourceProvider
//! [`StreamResolver`]: crate::ports::StreamResolver
//! [`Player`]: crate::ports::Player

use serde::{Deserialize, Serialize};

/// Video resolution tier, ordered worst → best by [`Quality::rank`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Quality {
    Unknown,
    P360,
    P480,
    P720,
    P1080,
    P2160,
}

impl Quality {
    /// Higher is better; `Unknown` is 0.
    pub fn rank(self) -> u8 {
        match self {
            Quality::Unknown => 0,
            Quality::P360 => 1,
            Quality::P480 => 2,
            Quality::P720 => 3,
            Quality::P1080 => 4,
            Quality::P2160 => 5,
        }
    }

    /// Parse a label such as `"1080p"`, `"2160p"`, `"4K"`.
    pub fn from_label(s: &str) -> Quality {
        match s.to_ascii_lowercase().as_str() {
            "2160p" | "4k" | "uhd" => Quality::P2160,
            "1080p" | "fhd" => Quality::P1080,
            "720p" | "hd" => Quality::P720,
            "480p" => Quality::P480,
            "360p" => Quality::P360,
            _ => Quality::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Unknown => "?",
            Quality::P360 => "360p",
            Quality::P480 => "480p",
            Quality::P720 => "720p",
            Quality::P1080 => "1080p",
            Quality::P2160 => "2160p",
        }
    }
}

/// Whether a candidate is instantly available on the configured debrid service.
///
/// Mirrors the real-world signal: Torrentio prefixes cached results with `[RD+]`
/// / `⚡`, Comet uses a `⚡` emoji. `Unknown` means we have not (or cannot)
/// check — the resolver then decides whether to warm the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheState {
    Cached,
    Uncached,
    Unknown,
}

/// Release tags parsed out of a candidate's raw title.
///
/// Parsing lives in the *adapters* (the Torrentio title format differs from a
/// nyaa filename), but the parsed shape is shared here so scoring and the UI are
/// format-agnostic.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseTags {
    /// e.g. `"HEVC"`, `"AVC"`, `"AV1"`.
    pub video_codec: Option<String>,
    /// e.g. `"DTS-HD MA 7.1"`, `"TrueHD Atmos"`.
    pub audio: Option<String>,
    /// e.g. `"HDR10+"`, `"DV"`.
    pub hdr: Option<String>,
    /// e.g. `"BluRay"`, `"WEB-DL"`, `"REMUX"`.
    pub source_type: Option<String>,
    /// Human language names parsed from flags/tags.
    pub languages: Vec<String>,
}

/// One candidate file from a source provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCandidate {
    /// Provider that surfaced this (e.g. `"nyaasi"`, `"1337x"`, `"torrentio"`).
    pub provider: String,
    /// Raw release title/filename (kept verbatim for scoring + display).
    pub title: String,
    pub quality: Quality,
    /// Size in bytes, if known (`0` when unknown).
    pub size_bytes: u64,
    pub seeders: Option<u32>,
    /// 40-char BitTorrent infohash, if known.
    pub info_hash: Option<String>,
    /// Pre-built magnet link, if the provider supplied one.
    pub magnet: Option<String>,
    /// A ready-to-play direct URL (e.g. a debrid CDN link), if the provider or a
    /// prior resolve step already produced one.
    pub direct_url: Option<String>,
    /// File index inside a multi-file torrent, if pinpointed.
    pub file_index: Option<usize>,
    pub cache: CacheState,
    pub tags: ReleaseTags,
}

impl SourceCandidate {
    /// Derive a magnet from the infohash when no explicit magnet was provided.
    pub fn magnet_or_from_hash(&self) -> Option<String> {
        if let Some(m) = &self.magnet {
            Some(m.clone())
        } else {
            self.info_hash
                .as_ref()
                .map(|h| format!("magnet:?xt=urn:btih:{h}"))
        }
    }

    /// A candidate is resolvable if we can either play a direct URL or obtain a
    /// magnet/infohash to hand to a debrid service or the P2P engine.
    pub fn is_resolvable(&self) -> bool {
        self.direct_url.is_some() || self.info_hash.is_some() || self.magnet.is_some()
    }

    pub fn human_size(&self) -> String {
        human_bytes(self.size_bytes)
    }
}

/// Where a resolved [`Playback`] URL comes from — affects cleanup + UI hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackOrigin {
    /// A direct HTTP(S) URL from a debrid service's CDN. Nothing to clean up.
    Debrid,
    /// Served by our local librqbit HTTP server. Torrent state must be torn down
    /// after playback (subject to the cleanup-after-playback setting).
    LocalP2p,
}

/// A concrete, player-openable stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playback {
    /// URL the media player opens (`http(s)://...`).
    pub url: String,
    pub origin: PlaybackOrigin,
    /// Display name of the file being played.
    pub file_name: String,
}

/// Convert a byte count into a compact human string (`1.4 GiB`, `727 MiB`).
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}
