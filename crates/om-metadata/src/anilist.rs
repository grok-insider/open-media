//! AniList GraphQL metadata adapter (anime).
//!
//! Search/details are public (no token). The token-gated mutations (progress
//! tracking) live in `om-track`, not here.

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::model::{Episode, IdSet, Media, MediaKind, Season};
use om_core::ports::MetadataProvider;
use reqwest::Client;
use serde::Deserialize;

use crate::map_net;

const DEFAULT_BASE: &str = "https://graphql.anilist.co";

const SEARCH_QUERY: &str = r#"
query ($search: String) {
  Page(perPage: 15) {
    media(search: $search, type: ANIME, sort: SEARCH_MATCH) {
      id idMal
      title { romaji english native }
      seasonYear episodes averageScore
      description(asHtml: false)
      coverImage { large }
      status format genres
    }
  }
}"#;

const DETAIL_QUERY: &str = r#"
query ($id: Int) {
  Media(id: $id, type: ANIME) {
    id idMal
    title { romaji english native }
    seasonYear episodes averageScore
    description(asHtml: false)
    coverImage { large }
    status format genres
  }
}"#;

/// AniList-backed metadata provider (anime only).
pub struct AniListProvider {
    client: Client,
    base_url: String,
}

impl AniListProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: DEFAULT_BASE.to_string(),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
        }
    }

    async fn query(&self, query: &str, variables: serde_json::Value) -> CoreResult<GqlData> {
        let body = serde_json::json!({ "query": query, "variables": variables });
        let resp = self
            .client
            .post(&self.base_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| map_net("anilist", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(CoreError::Remote {
                service: "anilist".into(),
                message: format!("HTTP {status}"),
            });
        }
        let parsed: GqlResponse = resp.json().await.map_err(|e| CoreError::Parse {
            what: "anilist response".into(),
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
        parsed.data.ok_or_else(|| CoreError::Parse {
            what: "anilist response".into(),
            message: "missing data field".into(),
        })
    }
}

impl Default for AniListProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MetadataProvider for AniListProvider {
    fn name(&self) -> &str {
        "anilist"
    }

    async fn search(&self, query: &str, kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        // AniList is an anime-only source: stay out of the way for explicit
        // movie/series searches; contribute for None or Anime.
        if matches!(kind, Some(MediaKind::Movie) | Some(MediaKind::Series)) {
            return Ok(Vec::new());
        }
        let data = self
            .query(SEARCH_QUERY, serde_json::json!({ "search": query }))
            .await?;
        let page = data.page.ok_or_else(|| CoreError::Parse {
            what: "anilist search".into(),
            message: "missing Page".into(),
        })?;
        Ok(page
            .media
            .into_iter()
            .map(AniListMedia::into_media)
            .collect())
    }

    async fn details(&self, ids: &IdSet) -> CoreResult<Media> {
        let id = ids
            .anilist
            .ok_or_else(|| CoreError::NotFound("anilist id required for details".into()))?;
        let data = self
            .query(DETAIL_QUERY, serde_json::json!({ "id": id }))
            .await?;
        let media = data
            .media
            .ok_or_else(|| CoreError::NotFound("anilist media".into()))?;
        Ok(media.into_media())
    }

    async fn seasons(&self, ids: &IdSet) -> CoreResult<Vec<Season>> {
        // AniList anime are flat-numbered; model as a single season.
        let media = self.details(ids).await?;
        Ok(vec![Season {
            number: 1,
            episode_count: media.episode_count.unwrap_or(0),
            name: None,
        }])
    }

    async fn episodes(&self, ids: &IdSet, season: u32) -> CoreResult<Vec<Episode>> {
        let media = self.details(ids).await?;
        let count = media.episode_count.unwrap_or(0);
        Ok((1..=count)
            .map(|n| Episode {
                season,
                number: n,
                title: None,
                air_date: None,
                overview: None,
                runtime_minutes: None,
                rating: None,
            })
            .collect())
    }
}

// --- GraphQL response shapes ---

#[derive(Debug, Deserialize)]
struct GqlResponse {
    #[serde(default)]
    data: Option<GqlData>,
    #[serde(default)]
    errors: Option<Vec<GqlError>>,
}

#[derive(Debug, Deserialize)]
struct GqlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct GqlData {
    #[serde(rename = "Page", default)]
    page: Option<Page>,
    #[serde(rename = "Media", default)]
    media: Option<AniListMedia>,
}

#[derive(Debug, Deserialize)]
struct Page {
    #[serde(default)]
    media: Vec<AniListMedia>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AniListMedia {
    id: i32,
    #[serde(default)]
    id_mal: Option<i32>,
    title: AniListTitle,
    #[serde(default)]
    season_year: Option<i32>,
    #[serde(default)]
    episodes: Option<u32>,
    #[serde(default)]
    average_score: Option<i32>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    cover_image: Option<CoverImage>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AniListTitle {
    #[serde(default)]
    romaji: Option<String>,
    #[serde(default)]
    english: Option<String>,
    #[serde(default)]
    native: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoverImage {
    #[serde(default)]
    large: Option<String>,
}

impl AniListMedia {
    fn into_media(self) -> Media {
        let mut ids = IdSet::default().with_anilist(self.id);
        if let Some(mal) = self.id_mal {
            ids = ids.with_mal(mal);
        }
        let title = self
            .title
            .english
            .clone()
            .or_else(|| self.title.romaji.clone())
            .or_else(|| self.title.native.clone())
            .unwrap_or_default();
        let original = self.title.romaji.or(self.title.native);
        Media {
            kind: MediaKind::Anime,
            ids,
            title,
            original_title: original,
            year: self.season_year,
            score: self.average_score.map(|s| s as f32 / 10.0),
            overview: self.description.filter(|s| !s.is_empty()),
            poster: self.cover_image.and_then(|c| c.large),
            genres: self.genres,
            status: self.status,
            episode_count: self.episodes,
            season_count: Some(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_media_with_mal_bridge_and_score() {
        let json = serde_json::json!({
            "id": 154587,
            "idMal": 52991,
            "title": { "romaji": "Sousou no Frieren", "english": "Frieren", "native": "葬送のフリーレン" },
            "seasonYear": 2023,
            "episodes": 28,
            "averageScore": 89,
            "description": "After the party of heroes...",
            "coverImage": { "large": "https://img/frieren.jpg" },
            "status": "FINISHED",
            "format": "TV",
            "genres": ["Adventure", "Drama", "Fantasy"]
        });
        let media: AniListMedia = serde_json::from_value(json).unwrap();
        let m = media.into_media();
        assert_eq!(m.kind, MediaKind::Anime);
        assert_eq!(m.ids.anilist, Some(154587));
        assert_eq!(m.ids.mal, Some(52991));
        assert_eq!(m.title, "Frieren");
        assert_eq!(m.original_title.as_deref(), Some("Sousou no Frieren"));
        assert_eq!(m.year, Some(2023));
        assert_eq!(m.score, Some(8.9));
        assert_eq!(m.episode_count, Some(28));
    }

    #[test]
    fn title_falls_back_to_romaji_when_no_english() {
        let json = serde_json::json!({
            "id": 1,
            "title": { "romaji": "Romaji Only", "english": null, "native": null }
        });
        let media: AniListMedia = serde_json::from_value(json).unwrap();
        assert_eq!(media.into_media().title, "Romaji Only");
    }
}
