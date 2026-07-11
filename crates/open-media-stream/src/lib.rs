//! # open-media-stream
//!
//! Resolution: turning a chosen [`SourceCandidate`] into a player-openable
//! [`Playback`], and the local P2P streaming engine that backs the non-debrid
//! path.
//!
//! - [`HybridResolver`] — the [`StreamResolver`] strategy. If the candidate is
//!   cached (or already has a direct URL) it returns the debrid URL; otherwise it
//!   either warms the debrid cache or falls back to [`P2pEngine`] when P2P is
//!   enabled.
//! - [`P2pEngine`] — wraps `librqbit`: adds the magnet, waits for metadata, picks
//!   the largest video file, and exposes librqbit's Range-aware HTTP endpoint
//!   (`/torrents/{id}/stream/{file_idx}`). This is the Rust equivalent of toru's
//!   localhost stream server.
//!
//! Resolution policy: a direct URL plays as-is; with debrid configured, the
//! candidate is warmed/unrestricted via the backend and, on failure (dead torrent
//! or a warm-up that times out), falls back to P2P when a magnet is available
//! **and** a P2P engine was wired. With no debrid and no P2P, resolution fails
//! with an actionable config error.
//!
//! [`SourceCandidate`]: open_media_core::stream::SourceCandidate
//! [`Playback`]: open_media_core::stream::Playback
//! [`StreamResolver`]: open_media_core::ports::StreamResolver

use std::sync::Arc;

use async_trait::async_trait;
use open_media_core::error::{CoreError, CoreResult};
use open_media_core::ports::{DebridProvider, StreamResolver};
use open_media_core::stream::{Playback, PlaybackOrigin, SourceCandidate};

mod p2p;
pub use p2p::P2pEngine;

/// Picks debrid-direct vs P2P for each candidate.
///
/// Holds an optional debrid backend (when a token is configured) and an optional
/// P2P engine (when `streaming.allow_p2p` is enabled). This is the single seam
/// where "cached → instant URL, otherwise → warm cache or stream P2P" is decided.
pub struct HybridResolver {
    debrid: Option<Arc<dyn DebridProvider>>,
    p2p: Option<Arc<P2pEngine>>,
}

impl HybridResolver {
    pub fn new(debrid: Option<Arc<dyn DebridProvider>>, p2p: Option<Arc<P2pEngine>>) -> Self {
        Self { debrid, p2p }
    }

    fn p2p_disabled_error() -> CoreError {
        CoreError::Config(
            "local P2P is disabled (streaming.allow_p2p=false). \
             Configure a debrid token, or set allow_p2p=true after reading docs/LEGAL.md"
                .into(),
        )
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
        let magnet = candidate.magnet_or_from_hash();

        // 2. Debrid backend configured → add/warm/unrestrict via it. This handles
        //    both cached (instant) and uncached (RD downloads to its CDN, bounded
        //    by the adapter's poll timeout) picks. On any failure — a dead torrent
        //    or a warm-up that times out — fall back to P2P when we have a magnet
        //    and P2P is enabled.
        if let Some(debrid) = &self.debrid {
            match debrid.resolve_playback(candidate).await {
                Ok(playback) => return Ok(playback),
                Err(e) => match (&magnet, &self.p2p) {
                    (Some(m), Some(p2p)) => {
                        tracing::warn!(error = %e, "debrid resolve failed; falling back to P2P");
                        return p2p.stream_magnet(m).await;
                    }
                    (Some(_), None) => {
                        tracing::warn!(
                            error = %e,
                            "debrid resolve failed; P2P disabled — not falling back"
                        );
                        return Err(e);
                    }
                    (None, _) => return Err(e),
                },
            }
        }

        // 3. No debrid: stream the torrent locally over P2P when enabled.
        let magnet = magnet
            .ok_or_else(|| CoreError::NoSource("candidate has no magnet or infohash".into()))?;
        match &self.p2p {
            Some(p2p) => p2p.stream_magnet(&magnet).await,
            None => Err(Self::p2p_disabled_error()),
        }
    }

    async fn cleanup(&self) {
        if let Some(p2p) = &self.p2p {
            p2p.cleanup().await;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use open_media_core::ports::{AddedTorrent, DebridFile};
    use open_media_core::stream::{CacheState, Quality, ReleaseTags};
    use std::collections::HashMap;

    /// A debrid backend whose `resolve_playback` outcome is fixed per test. Other
    /// methods are unused by the resolver and return inert values.
    struct FakeDebrid {
        ok: bool,
    }

    #[async_trait]
    impl DebridProvider for FakeDebrid {
        fn name(&self) -> &str {
            "fake"
        }
        async fn account_summary(&self) -> CoreResult<String> {
            Ok("fake".into())
        }
        async fn check_cached(&self, _: &[String]) -> CoreResult<HashMap<String, bool>> {
            Ok(HashMap::new())
        }
        async fn add_magnet(&self, _: &str) -> CoreResult<AddedTorrent> {
            Err(CoreError::NotImplemented("fake.add_magnet"))
        }
        async fn list_files(&self, _: &str) -> CoreResult<Vec<DebridFile>> {
            Ok(vec![])
        }
        async fn select_files(&self, _: &str, _: &[String]) -> CoreResult<()> {
            Ok(())
        }
        async fn unrestrict(&self, _: &str) -> CoreResult<String> {
            Err(CoreError::NotImplemented("fake.unrestrict"))
        }
        async fn resolve_playback(&self, _: &SourceCandidate) -> CoreResult<Playback> {
            if self.ok {
                Ok(Playback {
                    url: "https://cdn.example/stream.mkv".into(),
                    origin: PlaybackOrigin::Debrid,
                    file_name: "stream.mkv".into(),
                })
            } else {
                Err(CoreError::Timeout("fake: not ready".into()))
            }
        }
    }

    fn candidate(
        cache: CacheState,
        info_hash: Option<&str>,
        direct_url: Option<&str>,
    ) -> SourceCandidate {
        SourceCandidate {
            provider: "test".into(),
            title: "Test.Release.1080p".into(),
            quality: Quality::P1080,
            size_bytes: 0,
            seeders: None,
            info_hash: info_hash.map(str::to_string),
            magnet: None,
            direct_url: direct_url.map(str::to_string),
            file_index: None,
            cache,
            tags: ReleaseTags::default(),
        }
    }

    fn resolver(debrid: Option<Arc<dyn DebridProvider>>) -> HybridResolver {
        HybridResolver::new(debrid, Some(Arc::new(P2pEngine::new(0, true))))
    }

    #[tokio::test]
    async fn direct_url_plays_as_debrid_without_touching_backends() {
        let r = resolver(None);
        let c = candidate(CacheState::Cached, None, Some("https://cdn/x/movie.mkv"));
        let pb = r.resolve(&c).await.unwrap();
        assert_eq!(pb.origin, PlaybackOrigin::Debrid);
        assert_eq!(pb.file_name, "movie.mkv");
    }

    #[tokio::test]
    async fn cached_candidate_uses_debrid_result() {
        let r = resolver(Some(Arc::new(FakeDebrid { ok: true })));
        let c = candidate(CacheState::Cached, Some("abc"), None);
        let pb = r.resolve(&c).await.unwrap();
        assert_eq!(pb.url, "https://cdn.example/stream.mkv");
    }

    #[tokio::test]
    async fn debrid_error_without_magnet_propagates() {
        // Debrid fails and there's no magnet/hash to fall back to → surface error
        // (no P2P attempt).
        let r = resolver(Some(Arc::new(FakeDebrid { ok: false })));
        let c = candidate(CacheState::Cached, None, None);
        assert!(r.resolve(&c).await.is_err());
    }

    #[tokio::test]
    async fn magnet_without_debrid_or_p2p_returns_config_error() {
        let r = HybridResolver::new(None, None);
        let c = candidate(CacheState::Uncached, Some("aabbccdd"), None);
        let err = r.resolve(&c).await.unwrap_err();
        match err {
            CoreError::Config(msg) => {
                assert!(msg.contains("allow_p2p"), "unexpected message: {msg}");
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn debrid_failure_without_p2p_propagates_debrid_error() {
        let r = HybridResolver::new(Some(Arc::new(FakeDebrid { ok: false })), None);
        let c = candidate(CacheState::Uncached, Some("aabbccdd"), None);
        let err = r.resolve(&c).await.unwrap_err();
        assert!(matches!(err, CoreError::Timeout(_)));
    }
}
