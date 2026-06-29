//! MyAnimeList progress tracker (REST v2, bearer token).
//!
//! Updates go to `PUT /v2/anime/{id}/my_list_status` as form fields
//! (`num_watched_episodes`, `status`, `score`).

use async_trait::async_trait;
use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::IdSet;
use open_media_core::ports::Tracker;
use open_media_core::tracking::ListStatus;
use reqwest::Client;

use crate::map_net;

const DEFAULT_BASE: &str = "https://api.myanimelist.net/v2";

/// MyAnimeList-backed tracker.
pub struct MalTracker {
    client: Client,
    token: String,
    base_url: String,
}

impl MalTracker {
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

    async fn update(&self, ids: &IdSet, form: &[(&str, String)]) -> CoreResult<()> {
        let id = ids
            .mal
            .ok_or_else(|| CoreError::NotFound("MAL id required".into()))?;
        let resp = self
            .client
            .put(format!("{}/anime/{id}/my_list_status", self.base_url))
            .bearer_auth(&self.token)
            .form(form)
            .send()
            .await
            .map_err(|e| map_net("mal", e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(CoreError::Auth("mal: invalid token".into()));
        }
        if !status.is_success() {
            return Err(CoreError::Remote {
                service: "mal".into(),
                message: format!("HTTP {status}"),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl Tracker for MalTracker {
    fn name(&self) -> &str {
        "mal"
    }

    async fn update_progress(&self, ids: &IdSet, episode: u32) -> CoreResult<()> {
        self.update(ids, &[("num_watched_episodes", episode.to_string())])
            .await
    }

    async fn set_status(&self, ids: &IdSet, status: ListStatus) -> CoreResult<()> {
        self.update(ids, &status_form(status)).await
    }

    async fn rate(&self, ids: &IdSet, score: f32) -> CoreResult<()> {
        // MAL scores are integers 0–10.
        let clamped = score.round().clamp(0.0, 10.0) as u32;
        self.update(ids, &[("score", clamped.to_string())]).await
    }
}

fn status_str(status: ListStatus) -> &'static str {
    match status {
        ListStatus::Watching | ListStatus::Repeating => "watching",
        ListStatus::Completed => "completed",
        ListStatus::Planning => "plan_to_watch",
        ListStatus::Paused => "on_hold",
        ListStatus::Dropped => "dropped",
    }
}

/// Build the form fields for a `my_list_status` update.
///
/// MAL has no distinct "rewatching" status; the v2 API models a rewatch as
/// `status=watching` plus `is_rewatching=true`. Set the flag so a repeat is not
/// silently collapsed into a first watch.
fn status_form(status: ListStatus) -> Vec<(&'static str, String)> {
    let mut form = vec![("status", status_str(status).to_string())];
    if matches!(status, ListStatus::Repeating) {
        form.push(("is_rewatching", "true".to_string()));
    }
    form
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_status_to_mal_strings() {
        assert_eq!(status_str(ListStatus::Watching), "watching");
        assert_eq!(status_str(ListStatus::Planning), "plan_to_watch");
        assert_eq!(status_str(ListStatus::Paused), "on_hold");
    }

    #[test]
    fn repeating_sets_is_rewatching_flag() {
        let form = status_form(ListStatus::Repeating);
        assert_eq!(form[0], ("status", "watching".to_string()));
        assert!(
            form.iter()
                .any(|(k, v)| *k == "is_rewatching" && v == "true"),
            "repeating status must set is_rewatching=true, got {form:?}"
        );
    }

    #[test]
    fn plain_watching_does_not_set_is_rewatching() {
        let form = status_form(ListStatus::Watching);
        assert_eq!(form[0], ("status", "watching".to_string()));
        assert!(
            !form.iter().any(|(k, _)| *k == "is_rewatching"),
            "plain watching must not set is_rewatching, got {form:?}"
        );
    }
}
