//! # om-stream
//!
//! Resolution: turning a chosen [`SourceCandidate`] into a player-openable
//! [`Playback`], and the local P2P streaming engine that backs the non-debrid
//! path.
//!
//! - [`HybridResolver`] — the [`StreamResolver`] strategy. If the candidate is
//!   cached (or already has a direct URL) it returns the debrid URL; otherwise it
//!   either warms the debrid cache or falls back to [`P2pEngine`].
//! - [`P2pEngine`] — wraps `librqbit`: adds the magnet, waits for metadata, picks
//!   the largest video file, and exposes librqbit's Range-aware HTTP endpoint
//!   (`/torrents/{id}/stream/{file_idx}`). This is the Rust equivalent of toru's
//!   localhost stream server.
//!
//! Scaffold stub: the [`StreamResolver`] contract is implemented; bodies return
//! [`CoreError::NotImplemented`] until Phase 4 (see `docs/ROADMAP.md`).
//!
//! [`SourceCandidate`]: om_core::stream::SourceCandidate
//! [`Playback`]: om_core::stream::Playback
//! [`StreamResolver`]: om_core::ports::StreamResolver
//! [`CoreError::NotImplemented`]: om_core::error::CoreError::NotImplemented

use std::sync::Arc;

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::ports::{DebridProvider, StreamResolver};
use om_core::stream::{Playback, SourceCandidate};

/// Local P2P streaming engine (librqbit). Serves torrent bytes over HTTP with
/// Range support so the player can seek without a full download.
pub struct P2pEngine {
    #[allow(dead_code)]
    http_port: u16,
    #[allow(dead_code)]
    cleanup_after_playback: bool,
}

impl P2pEngine {
    pub fn new(http_port: u16, cleanup_after_playback: bool) -> Self {
        Self {
            http_port,
            cleanup_after_playback,
        }
    }

    /// Add a magnet and return its local stream URL. (Phase 4.)
    pub async fn stream_magnet(&self, _magnet: &str) -> CoreResult<Playback> {
        Err(CoreError::NotImplemented("p2p.stream_magnet"))
    }
}

/// Picks debrid-direct vs P2P for each candidate.
///
/// Holds an optional debrid backend (when a token is configured) and the P2P
/// engine fallback. This is the single seam where "cached → instant URL,
/// otherwise → warm cache or stream P2P" is decided.
pub struct HybridResolver {
    #[allow(dead_code)]
    debrid: Option<Arc<dyn DebridProvider>>,
    #[allow(dead_code)]
    p2p: Arc<P2pEngine>,
}

impl HybridResolver {
    pub fn new(debrid: Option<Arc<dyn DebridProvider>>, p2p: Arc<P2pEngine>) -> Self {
        Self { debrid, p2p }
    }
}

#[async_trait]
impl StreamResolver for HybridResolver {
    async fn resolve(&self, _candidate: &SourceCandidate) -> CoreResult<Playback> {
        Err(CoreError::NotImplemented("hybrid.resolve"))
    }

    async fn cleanup(&self) {
        // Phase 4: tear down the active P2P torrent when cleanup is enabled.
    }
}
