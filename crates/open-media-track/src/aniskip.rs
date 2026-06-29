//! Enricher: opening/ending skip windows (AniSkip) and filler episodes (Jikan).
//!
//! Both APIs are keyed by **MAL id** (bridged from AniList via the media's
//! `idMal`). Base URLs are injectable for tests.

use async_trait::async_trait;
use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::IdSet;
use open_media_core::ports::Enricher;
use open_media_core::tracking::{Interval, SkipTimes};
use reqwest::Client;
use serde::Deserialize;

use crate::map_net;

const ANISKIP_BASE: &str = "https://api.aniskip.com";
const JIKAN_BASE: &str = "https://api.jikan.moe";
const JIKAN_PAGE_CAP: u32 = 6;

/// AniSkip (intro/outro) + Jikan (filler) enricher.
pub struct AniSkipEnricher {
    client: Client,
    aniskip_base: String,
    jikan_base: String,
}

impl AniSkipEnricher {
    pub fn new() -> Self {
        Self {
            client: open_media_net::client(),
            aniskip_base: ANISKIP_BASE.to_string(),
            jikan_base: JIKAN_BASE.to_string(),
        }
    }

    /// Override both base URLs (tests point them at one mock server).
    pub fn with_bases(aniskip_base: impl Into<String>, jikan_base: impl Into<String>) -> Self {
        Self {
            client: open_media_net::client(),
            aniskip_base: aniskip_base.into(),
            jikan_base: jikan_base.into(),
        }
    }
}

impl Default for AniSkipEnricher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Enricher for AniSkipEnricher {
    async fn skip_times(
        &self,
        ids: &IdSet,
        episode: u32,
        episode_length_secs: Option<u32>,
    ) -> CoreResult<SkipTimes> {
        let mal = ids
            .mal
            .ok_or_else(|| CoreError::NotFound("MAL id required for AniSkip".into()))?;
        // AniSkip validates returned intervals against `episodeLength` (seconds);
        // `0` is its sentinel for "unknown — skip that validation". Pass the real
        // runtime when we have it so out-of-range intervals are filtered server-side.
        let episode_length = episode_length_secs.unwrap_or(0);
        let url = format!(
            "{}/v1/skip-times/{mal}/{episode}?types=op&types=ed&episodeLength={episode_length}",
            self.aniskip_base
        );
        let resp = open_media_net::retry(|| async {
            self.client
                .get(&url)
                .send()
                .await
                .map_err(|e| map_net("aniskip", e))
        })
        .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(SkipTimes::default());
        }
        if !resp.status().is_success() {
            return Err(CoreError::Remote {
                service: "aniskip".into(),
                message: format!("HTTP {}", resp.status()),
            });
        }
        let body: AniSkipResponse = resp.json().await.map_err(|e| CoreError::Parse {
            what: "aniskip response".into(),
            message: e.to_string(),
        })?;

        if !body.found {
            return Ok(SkipTimes::default());
        }
        let mut skip = SkipTimes::default();
        for r in body.results {
            let interval = Interval {
                start: r.interval.start_time.round() as u32,
                end: r.interval.end_time.round() as u32,
            };
            match r.skip_type.as_str() {
                "op" => skip.opening = Some(interval),
                "ed" => skip.ending = Some(interval),
                _ => {}
            }
        }
        Ok(skip)
    }

    async fn filler_episodes(&self, ids: &IdSet) -> CoreResult<Vec<u32>> {
        let mal = ids
            .mal
            .ok_or_else(|| CoreError::NotFound("MAL id required for Jikan filler".into()))?;

        let mut filler = Vec::new();
        let mut page = 1u32;
        loop {
            let url = format!("{}/v4/anime/{mal}/episodes?page={page}", self.jikan_base);
            let resp = open_media_net::retry(|| async {
                self.client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| map_net("jikan", e))
            })
            .await?;
            if !resp.status().is_success() {
                break;
            }
            let body: JikanEpisodes = resp.json().await.map_err(|e| CoreError::Parse {
                what: "jikan response".into(),
                message: e.to_string(),
            })?;
            for ep in &body.data {
                if ep.filler {
                    filler.push(ep.mal_id);
                }
            }
            let has_next = body.pagination.map(|p| p.has_next_page).unwrap_or(false);
            if !has_next || page >= JIKAN_PAGE_CAP {
                break;
            }
            page += 1;
            // Jikan rate-limits ~3 req/s.
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        }
        Ok(filler)
    }
}

#[derive(Debug, Deserialize)]
struct AniSkipResponse {
    #[serde(default)]
    found: bool,
    #[serde(default)]
    results: Vec<AniSkipResult>,
}

#[derive(Debug, Deserialize)]
struct AniSkipResult {
    interval: AniSkipInterval,
    #[serde(rename = "skipType")]
    skip_type: String,
}

#[derive(Debug, Deserialize)]
struct AniSkipInterval {
    #[serde(rename = "startTime")]
    start_time: f64,
    #[serde(rename = "endTime")]
    end_time: f64,
}

#[derive(Debug, Deserialize)]
struct JikanEpisodes {
    #[serde(default)]
    data: Vec<JikanEpisode>,
    #[serde(default)]
    pagination: Option<JikanPagination>,
}

#[derive(Debug, Deserialize)]
struct JikanEpisode {
    mal_id: u32,
    #[serde(default)]
    filler: bool,
}

#[derive(Debug, Deserialize)]
struct JikanPagination {
    #[serde(default)]
    has_next_page: bool,
}
