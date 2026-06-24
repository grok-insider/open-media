//! # om-debrid
//!
//! [`DebridProvider`] adapters. A debrid service turns a magnet into an instant
//! HTTPS link served from its own CDN — no P2P on the user's machine, no seeding,
//! no VPN needed.
//!
//! - [`RealDebrid`] — the canonical flow (`addMagnet` → poll `info` →
//!   `selectFiles` → poll → `unrestrict`), generalized from littlejohn + rdbatch.
//!
//! Future backends (AllDebrid, Torbox, Premiumize) are *new impls of the same
//! trait* — the provider-agnostic [`AddedTorrent`]/[`DebridFile`] shapes mean the
//! resolver and UI never change (OCP).
//!
//! Scaffold stub: the [`DebridProvider`] contract is implemented; bodies return
//! [`CoreError::NotImplemented`] until Phase 3 (see `docs/ROADMAP.md`).
//!
//! [`DebridProvider`]: om_core::ports::DebridProvider
//! [`AddedTorrent`]: om_core::ports::AddedTorrent
//! [`DebridFile`]: om_core::ports::DebridFile
//! [`CoreError::NotImplemented`]: om_core::error::CoreError::NotImplemented

use std::collections::HashMap;

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::ports::{AddedTorrent, DebridFile, DebridProvider};
use om_core::stream::{Playback, SourceCandidate};

/// Real-Debrid REST client (`https://api.real-debrid.com/rest/1.0`).
pub struct RealDebrid {
    #[allow(dead_code)]
    token: String,
}

impl RealDebrid {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }
}

#[async_trait]
impl DebridProvider for RealDebrid {
    fn name(&self) -> &str {
        "real-debrid"
    }

    async fn account_summary(&self) -> CoreResult<String> {
        Err(CoreError::NotImplemented("real-debrid.account_summary"))
    }

    async fn check_cached(&self, _info_hashes: &[String]) -> CoreResult<HashMap<String, bool>> {
        Err(CoreError::NotImplemented("real-debrid.check_cached"))
    }

    async fn add_magnet(&self, _magnet: &str) -> CoreResult<AddedTorrent> {
        Err(CoreError::NotImplemented("real-debrid.add_magnet"))
    }

    async fn list_files(&self, _torrent_id: &str) -> CoreResult<Vec<DebridFile>> {
        Err(CoreError::NotImplemented("real-debrid.list_files"))
    }

    async fn select_files(&self, _torrent_id: &str, _file_ids: &[String]) -> CoreResult<()> {
        Err(CoreError::NotImplemented("real-debrid.select_files"))
    }

    async fn unrestrict(&self, _link: &str) -> CoreResult<String> {
        Err(CoreError::NotImplemented("real-debrid.unrestrict"))
    }

    async fn resolve_playback(&self, _candidate: &SourceCandidate) -> CoreResult<Playback> {
        Err(CoreError::NotImplemented("real-debrid.resolve_playback"))
    }
}
