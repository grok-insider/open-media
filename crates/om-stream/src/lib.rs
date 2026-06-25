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
use om_core::stream::{Playback, PlaybackOrigin, SourceCandidate};

mod p2p;
pub use p2p::P2pEngine;

/// Picks debrid-direct vs P2P for each candidate.
///
/// Holds an optional debrid backend (when a token is configured) and the P2P
/// engine fallback. This is the single seam where "cached → instant URL,
/// otherwise → warm cache or stream P2P" is decided.
pub struct HybridResolver {
    debrid: Option<Arc<dyn DebridProvider>>,
    p2p: Arc<P2pEngine>,
}

impl HybridResolver {
    pub fn new(debrid: Option<Arc<dyn DebridProvider>>, p2p: Arc<P2pEngine>) -> Self {
        Self { debrid, p2p }
    }
}

#[async_trait]
impl StreamResolver for HybridResolver {
    async fn resolve(&self, candidate: &SourceCandidate) -> CoreResult<Playback> {
        // 1. The source already handed us a direct URL (cached debrid stream from
        //    the addon). Nothing to do — play it.
        if let Some(url) = &candidate.direct_url {
            return Ok(Playback {
                url: url.clone(),
                origin: PlaybackOrigin::Debrid,
                file_name: file_name_for(candidate, url),
            });
        }

        // 2. A debrid backend is configured → add/warm/unrestrict via it.
        if let Some(debrid) = &self.debrid {
            return debrid.resolve_playback(candidate).await;
        }

        // 3. No debrid: stream the torrent locally over P2P.
        let magnet = candidate
            .magnet_or_from_hash()
            .ok_or_else(|| CoreError::NoSource("candidate has no magnet or infohash".into()))?;
        self.p2p.stream_magnet(&magnet).await
    }

    async fn cleanup(&self) {
        self.p2p.cleanup().await;
    }
}

/// Best-effort display file name: last URL path segment, else the release title.
fn file_name_for(candidate: &SourceCandidate, url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| candidate.title.clone())
}
