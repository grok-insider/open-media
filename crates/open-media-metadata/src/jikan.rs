//! Jikan (unofficial MyAnimeList API) per-episode title source.
//!
//! AniList's `streamingEpisodes` is sparse, so most anime episodes have no
//! title. Jikan exposes `/v4/anime/{mal_id}/episodes` keyed by **MAL id**
//! (bridged from AniList via the media's `idMal`), which carries a real title
//! per episode number.
//!
//! This client mirrors the page-loop shape of open-media-track's aniskip enricher: a
//! shared [`reqwest::Client`], an injectable base URL (tests point it at a mock
//! server), a small page cap, and a pause between pages to respect Jikan's
//! ~3 req/s limit. Every lookup is **best-effort** — any failure yields an empty
//! map rather than failing the caller's `episodes()`.

use std::collections::HashMap;
use std::time::Duration;

use open_media_core::error::CoreError;
use reqwest::Client;
use serde::Deserialize;

const DEFAULT_BASE: &str = "https://api.jikan.moe";

/// Upper bound on Jikan episode pages fetched per title, so a very long series
/// can't fan out into many round-trips. Jikan pages 100 episodes each, so six
/// pages covers ~600 episodes — ample for essentially all anime.
const PAGE_CAP: u32 = 6;

/// Pause between Jikan page requests. Jikan rate-limits ~3 req/s; 350 ms keeps
/// the page loop comfortably under that ceiling (matches the aniskip enricher).
const PAGE_DELAY: Duration = Duration::from_millis(350);

/// Jikan-backed per-episode title source (MAL id keyed).
pub(crate) struct JikanTitles {
    client: Client,
    base_url: String,
}

impl JikanTitles {
    pub(crate) fn new() -> Self {
        Self {
            client: open_media_net::client(),
            base_url: DEFAULT_BASE.to_string(),
        }
    }

    /// Override the base URL (tests point it at a mock server).
    #[cfg(test)]
    pub(crate) fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: open_media_net::client(),
            base_url: base_url.into(),
        }
    }

    /// Fetch a map of `episode number -> title` for a MAL id.
    ///
    /// Best-effort: on **any** failure (network, non-2xx, parse) this returns an
    /// empty map. Callers treat a missing entry as "no Jikan title" and fall
    /// back to their existing source, so episode listing never fails because of
    /// Jikan.
    pub(crate) async fn episode_titles(&self, mal_id: i32) -> HashMap<u32, String> {
        match self.try_episode_titles(mal_id).await {
            Ok(map) => map,
            Err(e) => {
                tracing::debug!(error = %e, mal_id, "jikan episode titles unavailable; continuing without");
                HashMap::new()
            }
        }
    }

    /// Inner fetch that surfaces errors, so [`episode_titles`] can log them
    /// before degrading to an empty map.
    ///
    /// [`episode_titles`]: Self::episode_titles
    async fn try_episode_titles(&self, mal_id: i32) -> Result<HashMap<u32, String>, CoreError> {
        let mut titles = HashMap::new();
        let mut page = 1u32;
        loop {
            let url = format!("{}/v4/anime/{mal_id}/episodes?page={page}", self.base_url);
            let resp = self
                .client
                .get(&url)
                .send()
                .await
                .map_err(|e| crate::map_net("jikan", e))?;
            if !resp.status().is_success() {
                return Err(CoreError::Remote {
                    service: "jikan".into(),
                    message: format!("HTTP {}", resp.status()),
                });
            }
            let body: JikanEpisodes = resp.json().await.map_err(|e| CoreError::Parse {
                what: "jikan response".into(),
                message: e.to_string(),
            })?;
            for ep in body.data {
                // Keep only entries that carry both a numbered slot and a
                // non-empty title; the `mal_id` here is Jikan's per-anime
                // episode number (1-based), which is what we key episodes on.
                if let Some(title) = non_empty(ep.title) {
                    titles.entry(ep.mal_id).or_insert(title);
                }
            }
            let has_next = body.pagination.map(|p| p.has_next_page).unwrap_or(false);
            if !has_next || page >= PAGE_CAP {
                break;
            }
            page += 1;
            tokio::time::sleep(PAGE_DELAY).await;
        }
        Ok(titles)
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

#[derive(Debug, Deserialize)]
struct JikanEpisodes {
    #[serde(default)]
    data: Vec<JikanEpisode>,
    #[serde(default)]
    pagination: Option<JikanPagination>,
}

/// One Jikan episode. Unlike open-media-track's `JikanEpisode` (which keeps only
/// `mal_id`/`filler`), this keeps `title` — the whole point of this client.
#[derive(Debug, Deserialize)]
struct JikanEpisode {
    /// Jikan's per-anime episode number (1-based), used as the map key.
    mal_id: u32,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JikanPagination {
    #[serde(default)]
    has_next_page: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A Jikan `/episodes` page with the given episode entries and `has_next`.
    fn jikan_page(eps: &[(u32, &str)], has_next: bool) -> serde_json::Value {
        let data: Vec<_> = eps
            .iter()
            .map(|(n, t)| serde_json::json!({ "mal_id": n, "title": t }))
            .collect();
        serde_json::json!({
            "pagination": { "has_next_page": has_next },
            "data": data,
        })
    }

    #[tokio::test]
    async fn returns_episode_number_to_title_map() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/v4/anime/\d+/episodes$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(jikan_page(
                &[(1, "The First Step"), (2, "Onward"), (3, "")],
                false,
            )))
            .mount(&server)
            .await;

        let jikan = JikanTitles::with_base_url(server.uri());
        let titles = jikan.episode_titles(52991).await;

        assert_eq!(titles.get(&1).map(String::as_str), Some("The First Step"));
        assert_eq!(titles.get(&2).map(String::as_str), Some("Onward"));
        // Empty title is dropped, not stored as an empty string.
        assert_eq!(titles.get(&3), None);
    }

    #[tokio::test]
    async fn http_error_yields_empty_map() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let jikan = JikanTitles::with_base_url(server.uri());
        // Best-effort: a 5xx must degrade to an empty map, never an error.
        assert!(jikan.episode_titles(1).await.is_empty());
    }
}
