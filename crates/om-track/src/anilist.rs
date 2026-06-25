//! AniList progress tracker (GraphQL mutations, bearer token).

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::model::IdSet;
use om_core::ports::Tracker;
use om_core::tracking::ListStatus;
use reqwest::Client;
use serde::Deserialize;

use crate::map_net;

const DEFAULT_BASE: &str = "https://graphql.anilist.co";

/// AniList-backed tracker.
pub struct AniListTracker {
    client: Client,
    token: String,
    base_url: String,
}

impl AniListTracker {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            token: token.into(),
            base_url: DEFAULT_BASE.to_string(),
        }
    }

    pub fn with_base_url(token: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::new(token)
        }
    }

    async fn mutate(&self, variables: serde_json::Value) -> CoreResult<()> {
        const MUTATION: &str = r#"
mutation ($mediaId: Int, $progress: Int, $status: MediaListStatus, $score: Float) {
  SaveMediaListEntry(mediaId: $mediaId, progress: $progress, status: $status, score: $score) {
    id progress status score
  }
}"#;
        let body = serde_json::json!({ "query": MUTATION, "variables": variables });
        let resp = self
            .client
            .post(&self.base_url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| map_net("anilist", e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(CoreError::Auth("anilist: invalid token".into()));
        }
        if !status.is_success() {
            return Err(CoreError::Remote {
                service: "anilist".into(),
                message: format!("HTTP {status}"),
            });
        }
        let parsed: GqlResponse = resp.json().await.map_err(|e| CoreError::Parse {
            what: "anilist mutation".into(),
            message: e.to_string(),
        })?;
        if let Some(errors) = parsed.errors {
            if !errors.is_empty() {
                return Err(CoreError::Remote {
                    service: "anilist".into(),
                    message: errors
                        .into_iter()
                        .map(|e| e.message)
                        .collect::<Vec<_>>()
                        .join("; "),
                });
            }
        }
        Ok(())
    }

    fn media_id(ids: &IdSet) -> CoreResult<i32> {
        ids.anilist
            .ok_or_else(|| CoreError::NotFound("AniList id required".into()))
    }
}

#[async_trait]
impl Tracker for AniListTracker {
    fn name(&self) -> &str {
        "anilist"
    }

    async fn update_progress(&self, ids: &IdSet, episode: u32) -> CoreResult<()> {
        let id = Self::media_id(ids)?;
        self.mutate(serde_json::json!({ "mediaId": id, "progress": episode }))
            .await
    }

    async fn set_status(&self, ids: &IdSet, status: ListStatus) -> CoreResult<()> {
        let id = Self::media_id(ids)?;
        self.mutate(serde_json::json!({ "mediaId": id, "status": status_str(status) }))
            .await
    }

    async fn rate(&self, ids: &IdSet, score: f32) -> CoreResult<()> {
        let id = Self::media_id(ids)?;
        self.mutate(serde_json::json!({ "mediaId": id, "score": score }))
            .await
    }
}

fn status_str(status: ListStatus) -> &'static str {
    match status {
        ListStatus::Watching => "CURRENT",
        ListStatus::Completed => "COMPLETED",
        ListStatus::Planning => "PLANNING",
        ListStatus::Paused => "PAUSED",
        ListStatus::Dropped => "DROPPED",
        ListStatus::Repeating => "REPEATING",
    }
}

#[derive(Debug, Deserialize)]
struct GqlResponse {
    #[serde(default)]
    errors: Option<Vec<GqlError>>,
}

#[derive(Debug, Deserialize)]
struct GqlError {
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_status_to_anilist_enum() {
        assert_eq!(status_str(ListStatus::Watching), "CURRENT");
        assert_eq!(status_str(ListStatus::Completed), "COMPLETED");
        assert_eq!(status_str(ListStatus::Repeating), "REPEATING");
    }

    #[test]
    fn requires_anilist_id() {
        assert!(AniListTracker::media_id(&IdSet::default().with_mal(1)).is_err());
        assert!(AniListTracker::media_id(&IdSet::default().with_anilist(5)).is_ok());
    }
}
