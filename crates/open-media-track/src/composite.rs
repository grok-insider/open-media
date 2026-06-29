//! Composite tracker: fan every write out to all configured backends.

use async_trait::async_trait;
use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::IdSet;
use open_media_core::ports::Tracker;
use open_media_core::tracking::ListStatus;

/// Writes each update to every member tracker (AniList + MAL), aggregating
/// failures so one backend being down does not silently drop the other's update.
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Failing;
    #[async_trait]
    impl Tracker for Failing {
        fn name(&self) -> &str {
            "failing"
        }
        async fn update_progress(&self, _: &IdSet, _: u32) -> CoreResult<()> {
            Err(CoreError::Other("boom".into()))
        }
        async fn set_status(&self, _: &IdSet, _: ListStatus) -> CoreResult<()> {
            Ok(())
        }
        async fn rate(&self, _: &IdSet, _: f32) -> CoreResult<()> {
            Ok(())
        }
    }

    struct Ok2;
    #[async_trait]
    impl Tracker for Ok2 {
        fn name(&self) -> &str {
            "ok"
        }
        async fn update_progress(&self, _: &IdSet, _: u32) -> CoreResult<()> {
            Ok(())
        }
        async fn set_status(&self, _: &IdSet, _: ListStatus) -> CoreResult<()> {
            Ok(())
        }
        async fn rate(&self, _: &IdSet, _: f32) -> CoreResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn aggregates_failures_but_runs_all() {
        let c = CompositeTracker::new(vec![Box::new(Failing), Box::new(Ok2)]);
        let err = c.update_progress(&IdSet::default(), 1).await.unwrap_err();
        assert!(err.to_string().contains("failing"));
        // The Ok member's other ops still succeed.
        assert!(c
            .set_status(&IdSet::default(), ListStatus::Watching)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn empty_is_ok() {
        let c = CompositeTracker::new(vec![]);
        assert!(c.is_empty());
        assert!(c.update_progress(&IdSet::default(), 1).await.is_ok());
    }
}
