//! # om-track
//!
//! Three related anime-power ports, grouped because they share the AniList/MAL
//! id plumbing:
//!
//! - [`Tracker`] — [`AniListTracker`], [`MalTracker`], and [`CompositeTracker`]
//!   (a real fan-out that writes to several trackers, bridging ids — curd's
//!   dual-write pattern).
//! - [`Enricher`] — [`AniSkipEnricher`] (intro/outro via api.aniskip.com, keyed by
//!   MAL id) and filler/recap via Jikan.
//! - [`PresenceReporter`] — [`DiscordPresence`] (throttled "now watching" RPC).
//!
//! The individual remote adapters are scaffold stubs (Phase 6); the
//! [`CompositeTracker`] is implemented for real to demonstrate the composite.
//!
//! [`Tracker`]: om_core::ports::Tracker
//! [`Enricher`]: om_core::ports::Enricher
//! [`PresenceReporter`]: om_core::ports::PresenceReporter

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::model::IdSet;
use om_core::ports::{Enricher, PresenceReporter, Tracker};
use om_core::tracking::{Activity, ListStatus, SkipTimes};

/// AniList GraphQL tracker.
pub struct AniListTracker {
    #[allow(dead_code)]
    token: String,
}

impl AniListTracker {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }
}

#[async_trait]
impl Tracker for AniListTracker {
    fn name(&self) -> &str {
        "anilist"
    }
    async fn update_progress(&self, _ids: &IdSet, _episode: u32) -> CoreResult<()> {
        Err(CoreError::NotImplemented("anilist.update_progress"))
    }
    async fn set_status(&self, _ids: &IdSet, _status: ListStatus) -> CoreResult<()> {
        Err(CoreError::NotImplemented("anilist.set_status"))
    }
    async fn rate(&self, _ids: &IdSet, _score: f32) -> CoreResult<()> {
        Err(CoreError::NotImplemented("anilist.rate"))
    }
}

/// MyAnimeList tracker.
pub struct MalTracker {
    #[allow(dead_code)]
    token: String,
}

impl MalTracker {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }
}

#[async_trait]
impl Tracker for MalTracker {
    fn name(&self) -> &str {
        "mal"
    }
    async fn update_progress(&self, _ids: &IdSet, _episode: u32) -> CoreResult<()> {
        Err(CoreError::NotImplemented("mal.update_progress"))
    }
    async fn set_status(&self, _ids: &IdSet, _status: ListStatus) -> CoreResult<()> {
        Err(CoreError::NotImplemented("mal.set_status"))
    }
    async fn rate(&self, _ids: &IdSet, _score: f32) -> CoreResult<()> {
        Err(CoreError::NotImplemented("mal.rate"))
    }
}

/// Fans every tracker write out to all configured backends (AniList + MAL).
///
/// This is implemented for real: it runs each member and aggregates failures, so
/// one backend being down does not silently drop the other's update.
pub struct CompositeTracker {
    members: Vec<Box<dyn Tracker>>,
}

impl CompositeTracker {
    pub fn new(members: Vec<Box<dyn Tracker>>) -> Self {
        Self { members }
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

#[async_trait]
impl Tracker for CompositeTracker {
    fn name(&self) -> &str {
        "composite"
    }

    async fn update_progress(&self, ids: &IdSet, episode: u32) -> CoreResult<()> {
        let mut errors = Vec::new();
        for t in &self.members {
            if let Err(e) = t.update_progress(ids, episode).await {
                errors.push(format!("{}: {e}", t.name()));
            }
        }
        aggregate(errors)
    }

    async fn set_status(&self, ids: &IdSet, status: ListStatus) -> CoreResult<()> {
        let mut errors = Vec::new();
        for t in &self.members {
            if let Err(e) = t.set_status(ids, status).await {
                errors.push(format!("{}: {e}", t.name()));
            }
        }
        aggregate(errors)
    }

    async fn rate(&self, ids: &IdSet, score: f32) -> CoreResult<()> {
        let mut errors = Vec::new();
        for t in &self.members {
            if let Err(e) = t.rate(ids, score).await {
                errors.push(format!("{}: {e}", t.name()));
            }
        }
        aggregate(errors)
    }
}

fn aggregate(errors: Vec<String>) -> CoreResult<()> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CoreError::Other(errors.join("; ")))
    }
}

/// AniSkip + Jikan enricher.
pub struct AniSkipEnricher;

impl AniSkipEnricher {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AniSkipEnricher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Enricher for AniSkipEnricher {
    async fn skip_times(&self, _ids: &IdSet, _episode: u32) -> CoreResult<SkipTimes> {
        Err(CoreError::NotImplemented("aniskip.skip_times"))
    }
    async fn filler_episodes(&self, _ids: &IdSet) -> CoreResult<Vec<u32>> {
        Err(CoreError::NotImplemented("jikan.filler_episodes"))
    }
}

/// Discord rich-presence reporter.
pub struct DiscordPresence {
    #[allow(dead_code)]
    client_id: String,
}

impl DiscordPresence {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
        }
    }
}

#[async_trait]
impl PresenceReporter for DiscordPresence {
    async fn update(&self, _activity: &Activity) -> CoreResult<()> {
        Err(CoreError::NotImplemented("discord.update"))
    }
    async fn clear(&self) -> CoreResult<()> {
        Err(CoreError::NotImplemented("discord.clear"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn composite_aggregates_member_errors() {
        // Two stub members both return NotImplemented; the composite should
        // surface an aggregated error rather than panicking or hiding one.
        let composite = CompositeTracker::new(vec![
            Box::new(AniListTracker::new("x")),
            Box::new(MalTracker::new("y")),
        ]);
        let ids = IdSet::default().with_mal(1);
        let err = composite.update_progress(&ids, 1).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("anilist"));
        assert!(msg.contains("mal"));
    }

    #[tokio::test]
    async fn empty_composite_is_ok() {
        let composite = CompositeTracker::new(vec![]);
        assert!(composite.is_empty());
        let ids = IdSet::default();
        assert!(composite.update_progress(&ids, 1).await.is_ok());
    }
}
