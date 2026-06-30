//! # open-media-debrid
//!
//! [`DebridProvider`] adapters. A debrid service turns a magnet into an instant
//! HTTPS link served from its own CDN — no P2P on the user's machine, no seeding,
//! no VPN needed.
//!
//! - [`RealDebrid`] — the canonical flow (`addMagnet` → poll `info` →
//!   `selectFiles` → poll → `unrestrict`).
//!
//! Future backends (AllDebrid, Torbox, Premiumize) are *new impls of the same
//! trait* — the provider-agnostic [`AddedTorrent`]/[`DebridFile`] shapes mean the
//! resolver and UI never change (OCP).
//!
//! [`DebridProvider`]: open_media_core::ports::DebridProvider
//! [`AddedTorrent`]: open_media_core::ports::AddedTorrent
//! [`DebridFile`]: open_media_core::ports::DebridFile

pub mod realdebrid;

pub use realdebrid::RealDebrid;
