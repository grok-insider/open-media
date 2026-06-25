//! TMDB (The Movie Database) v3 metadata adapter.

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::model::{Episode, IdSet, Media, MediaKind, Season};
use om_core::ports::MetadataProvider;
use reqwest::Client;
use serde::Deserialize;

use crate::map_net;

const DEFAULT_BASE: &str = "https://api.themoviedb.org/3";
const IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w500";

/// TMDB-backed metadata provider.
pub struct TmdbProvider {
    client: Client,
    api_key: String,
    base_url: String,
}

impl TmdbProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            base_url: DEFAULT_BASE.to_string(),
        }
    }

    /// Construct against a custom base URL (used by integration tests).
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            base_url: base_url.into(),
        }
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> CoreResult<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self
            .client
            .get(&url)
            .query(&[("api_key", self.api_key.as_str())]);
        if !query.is_empty() {
            req = req.query(query);
        }
        let resp = req.send().await.map_err(|e| map_net("tmdb", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(CoreError::Remote {
                service: "tmdb".into(),
                message: format!("HTTP {status}"),
            });
        }
        resp.json::<T>().await.map_err(|e| CoreError::Parse {
            what: "tmdb response".into(),
            message: e.to_string(),
        })
    }

    async fn get_movie(&self, id: i32) -> CoreResult<Media> {
        let raw: TmdbDetail = self
            .get_json(
                &format!("/movie/{id}"),
                &[("append_to_response", "external_ids")],
            )
            .await?;
        Ok(raw.into_media(MediaKind::Movie))
    }

    async fn get_tv(&self, id: i32) -> CoreResult<Media> {
        let raw: TmdbDetail = self
            .get_json(
                &format!("/tv/{id}"),
                &[("append_to_response", "external_ids")],
            )
            .await?;
        Ok(raw.into_media(MediaKind::Series))
    }
}

#[async_trait]
impl MetadataProvider for TmdbProvider {
    fn name(&self) -> &str {
        "tmdb"
    }

    async fn search(&self, query: &str, kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        let path = match kind {
            Some(MediaKind::Movie) => "/search/movie",
            Some(MediaKind::Series) | Some(MediaKind::Anime) => "/search/tv",
            None => "/search/multi",
        };
        let resp: SearchResponse = self
            .get_json(path, &[("query", query), ("include_adult", "false")])
            .await?;

        let forced_kind = match kind {
            Some(MediaKind::Movie) => Some(MediaKind::Movie),
            Some(MediaKind::Series) | Some(MediaKind::Anime) => Some(MediaKind::Series),
            None => None,
        };

        Ok(resp
            .results
            .into_iter()
            .filter_map(|r| r.into_media(forced_kind))
            .collect())
    }

    async fn details(&self, ids: &IdSet) -> CoreResult<Media> {
        let id = ids
            .tmdb
            .ok_or_else(|| CoreError::NotFound("tmdb id required for details".into()))?;
        // We don't carry kind in IdSet, so try movie then fall back to tv.
        match self.get_movie(id).await {
            Ok(m) => Ok(m),
            Err(CoreError::Remote { .. }) | Err(CoreError::NotFound(_)) => self.get_tv(id).await,
            Err(e) => Err(e),
        }
    }

    async fn seasons(&self, ids: &IdSet) -> CoreResult<Vec<Season>> {
        let id = ids
            .tmdb
            .ok_or_else(|| CoreError::NotFound("tmdb id required for seasons".into()))?;
        let raw: TmdbDetail = self.get_json(&format!("/tv/{id}"), &[]).await?;
        Ok(raw
            .seasons
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.season_number > 0)
            .map(|s| Season {
                number: s.season_number,
                episode_count: s.episode_count.unwrap_or(0),
                name: s.name,
            })
            .collect())
    }

    async fn episodes(&self, ids: &IdSet, season: u32) -> CoreResult<Vec<Episode>> {
        let id = ids
            .tmdb
            .ok_or_else(|| CoreError::NotFound("tmdb id required for episodes".into()))?;
        let raw: SeasonDetail = self
            .get_json(&format!("/tv/{id}/season/{season}"), &[])
            .await?;
        Ok(raw
            .episodes
            .into_iter()
            .map(|e| Episode {
                season,
                number: e.episode_number,
                title: e.name,
                air_date: e.air_date,
                overview: non_empty(e.overview),
                runtime_minutes: e.runtime,
                rating: e.vote_average,
            })
            .collect())
    }
}

// --- TMDB response shapes ---

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    id: i32,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    original_title: Option<String>,
    #[serde(default)]
    original_name: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    vote_average: Option<f32>,
}

impl SearchResult {
    fn into_media(self, forced_kind: Option<MediaKind>) -> Option<Media> {
        let kind = match forced_kind {
            Some(k) => k,
            None => match self.media_type.as_deref() {
                Some("movie") => MediaKind::Movie,
                Some("tv") => MediaKind::Series,
                _ => return None, // skip "person" and unknown
            },
        };
        let title = self.title.or(self.name)?;
        let original = self.original_title.or(self.original_name);
        let date = self.release_date.or(self.first_air_date);
        Some(Media {
            kind,
            ids: IdSet::default().with_tmdb(self.id),
            title,
            original_title: original,
            year: parse_year(date.as_deref()),
            score: self.vote_average,
            overview: non_empty(self.overview),
            poster: self.poster_path.map(|p| format!("{IMAGE_BASE}{p}")),
            genres: Vec::new(),
            status: None,
            episode_count: None,
            season_count: None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct TmdbDetail {
    id: i32,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    original_title: Option<String>,
    #[serde(default)]
    original_name: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    vote_average: Option<f32>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    imdb_id: Option<String>,
    #[serde(default)]
    external_ids: Option<ExternalIds>,
    #[serde(default)]
    number_of_episodes: Option<u32>,
    #[serde(default)]
    number_of_seasons: Option<u32>,
    #[serde(default)]
    seasons: Option<Vec<SeasonSummary>>,
    #[serde(default)]
    genres: Option<Vec<Genre>>,
}

#[derive(Debug, Deserialize)]
struct ExternalIds {
    #[serde(default)]
    imdb_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Genre {
    name: String,
}

#[derive(Debug, Deserialize)]
struct SeasonSummary {
    season_number: u32,
    #[serde(default)]
    episode_count: Option<u32>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SeasonDetail {
    #[serde(default)]
    episodes: Vec<EpisodeDetail>,
}

#[derive(Debug, Deserialize)]
struct EpisodeDetail {
    episode_number: u32,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    air_date: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    runtime: Option<u32>,
    #[serde(default)]
    vote_average: Option<f32>,
}

impl TmdbDetail {
    fn into_media(self, kind: MediaKind) -> Media {
        let imdb = self
            .imdb_id
            .clone()
            .or_else(|| self.external_ids.as_ref().and_then(|e| e.imdb_id.clone()))
            .filter(|s| !s.is_empty());
        let mut ids = IdSet::default().with_tmdb(self.id);
        if let Some(imdb) = imdb {
            ids = ids.with_imdb(imdb);
        }
        let title = self.title.or(self.name).unwrap_or_default();
        let original = self.original_title.or(self.original_name);
        let date = self.release_date.or(self.first_air_date);
        Media {
            kind,
            ids,
            title,
            original_title: original,
            year: parse_year(date.as_deref()),
            score: self.vote_average,
            overview: non_empty(self.overview),
            poster: self.poster_path.map(|p| format!("{IMAGE_BASE}{p}")),
            genres: self
                .genres
                .unwrap_or_default()
                .into_iter()
                .map(|g| g.name)
                .collect(),
            status: self.status,
            episode_count: self.number_of_episodes,
            season_count: self.number_of_seasons,
        }
    }
}

fn parse_year(date: Option<&str>) -> Option<i32> {
    date.and_then(|d| d.get(0..4)).and_then(|y| y.parse().ok())
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_year_extracts_from_date() {
        assert_eq!(parse_year(Some("2023-09-29")), Some(2023));
        assert_eq!(parse_year(Some("")), None);
        assert_eq!(parse_year(None), None);
    }

    #[test]
    fn search_result_skips_person() {
        let person = SearchResult {
            id: 1,
            media_type: Some("person".into()),
            title: None,
            name: Some("Some Actor".into()),
            original_title: None,
            original_name: None,
            release_date: None,
            first_air_date: None,
            overview: None,
            poster_path: None,
            vote_average: None,
        };
        assert!(person.into_media(None).is_none());
    }

    #[test]
    fn detail_prefers_top_level_imdb_then_external() {
        let json = serde_json::json!({
            "id": 42,
            "name": "Frieren",
            "first_air_date": "2023-09-29",
            "external_ids": { "imdb_id": "tt22248376" },
            "number_of_seasons": 1,
            "number_of_episodes": 28
        });
        let detail: TmdbDetail = serde_json::from_value(json).unwrap();
        let media = detail.into_media(MediaKind::Series);
        assert_eq!(media.ids.tmdb, Some(42));
        assert_eq!(media.ids.imdb.as_deref(), Some("tt22248376"));
        assert_eq!(media.year, Some(2023));
        assert_eq!(media.season_count, Some(1));
        assert_eq!(media.episode_count, Some(28));
    }
}
