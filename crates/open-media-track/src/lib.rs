//! # open-media-track
//!
//! Anime-power ports grouped because they share the AniList/MAL id plumbing:
//!
//! - [`Tracker`] — [`AniListTracker`], [`MalTracker`], and [`CompositeTracker`]
//!   (a real fan-out that writes to several trackers — curd's dual-write pattern).
//! - [`Enricher`] — [`AniSkipEnricher`]: opening/ending via api.aniskip.com (by
//!   MAL id) + filler/recap via Jikan.
//! - [`PresenceReporter`] — [`DiscordPresence`] (best-effort RPC over the Discord
//!   IPC socket).
//!
//! Remote adapters take an injectable base URL so integration tests can target a
//! mock server (see `tests/`).
//!
//! [`Tracker`]: open_media_core::ports::Tracker
//! [`Enricher`]: open_media_core::ports::Enricher
//! [`PresenceReporter`]: open_media_core::ports::PresenceReporter

pub mod anilist;
pub mod aniskip;
pub mod composite;
pub mod discord;
mod ipc;
pub mod mal;

pub use anilist::AniListTracker;
pub use aniskip::AniSkipEnricher;
pub use composite::CompositeTracker;
pub use discord::DiscordPresence;
pub use mal::MalTracker;

use open_media_core::error::CoreError;

pub(crate) fn map_net(service: &str, e: reqwest::Error) -> CoreError {
    if e.is_timeout() {
        CoreError::Timeout(format!("{service}: {e}"))
    } else {
        CoreError::Network(format!("{service}: {e}"))
    }
}
