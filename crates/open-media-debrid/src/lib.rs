//! # open-media-debrid
//!
//! [`DebridProvider`] adapters. A debrid service turns a magnet into an HTTPS
//! link served from its CDN when the backend has the release (or can obtain it).
//! That path does not open a local BitTorrent swarm; it is not a copyright license
//! to the underlying work (see `docs/LEGAL.md`).
//!
//! - [`RealDebrid`] — the canonical flow (`addMagnet` → poll `info` →
//!   `selectFiles` → poll → `unrestrict`).
//! - [`Torbox`] — envelope-wrapped REST (`createtorrent` → poll `mylist` →
//!   `requestdl`), no file-selection step, and a *working* bulk cache check.
//!
//! Future backends (AllDebrid, Premiumize) are *new impls of the same trait* —
//! the provider-agnostic [`AddedTorrent`]/[`DebridFile`] shapes mean the
//! resolver and UI never change (OCP).
//!
//! [`DebridProvider`]: open_media_core::ports::DebridProvider
//! [`AddedTorrent`]: open_media_core::ports::AddedTorrent
//! [`DebridFile`]: open_media_core::ports::DebridFile

pub mod realdebrid;
pub mod torbox;

pub use realdebrid::RealDebrid;
pub use torbox::Torbox;
