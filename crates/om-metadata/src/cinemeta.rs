//! Cinemeta (Stremio) metadata adapter — keyless movies & series.
//!
//! Cinemeta is Stremio's official catalog addon. It needs **no API key** and is
//! **IMDB-native** (every item is a `tt…` id), which is exactly what the source
//! layer (Torrentio/Comet) requires — so it is the default discovery path for
//! movies and live-action series when the user has not configured a TMDB key.
//!
//! Endpoints used (all GET, JSON):
//! - search:  `/catalog/{movie|series}/top/search={query}.json` → `{ metas: [...] }`
//! - details: `/meta/{movie|series}/{imdbId}.json` → `{ meta: {...} }`
//!
//! ## API quirks worth knowing
//! - The **series** meta endpoint cleanly returns `meta: null` for an id it does
//!   not have as a series; the **movie** endpoint instead 307-redirects and can
//!   echo back an *unrelated* item for a series id. So [`details`] queries
//!   `series` first and only falls back to `movie` when series misses — never the
//!   other way around — to avoid that poisoned response.
//! - Anime is intentionally **not** served here: AniList owns anime discovery (it
//!   carries the MAL/AniList ids AniSkip and the trackers need). Cinemeta stays on
//!   movie/series, mirroring how `AniListProvider` stays out of movie/series.
//!
//! [`details`]: CinemetaProvider::details

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::model::{Episode, IdSet, Media, MediaKind, Season};
use om_core::ports::MetadataProvider;
use reqwest::Client;
use serde::Deserialize;

use crate::map_net;

const DEFAULT_BASE: &str = "https://v3-cinemeta.strem.io";

/// Cinemeta-backed metadata provider (keyless).
pub struct CinemetaProvider {
    client: Client,
    base_url: String,
}

impl Default for CinemetaProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl CinemetaProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: DEFAULT_BASE.to_string(),
        }
    }

    /// Construct against a custom base URL (used by integration tests).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
        }
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> CoreResult<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| map_net("cinemeta", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(CoreError::Remote {
                service: "cinemeta".into(),
                message: format!("HTTP {status}"),
            });
        }
        resp.json::<T>().await.map_err(|e| CoreError::Parse {
            what: "cinemeta response".into(),
            message: e.to_string(),
        })
    }

    /// Search one catalog (`movie` or `series`) and tag results with `kind`.
    async fn search_catalog(&self, catalog: &str, query: &str, kind: MediaKind) -> Vec<Media> {
        let enc = urlencoding::encode(query);
        let path = format!("/catalog/{catalog}/top/search={enc}.json");
        match self.get_json::<CatalogResponse>(&path).await {
            Ok(resp) => resp
                .metas
                .into_iter()
                .filter_map(|m| m.into_media(kind))
                .collect(),
            Err(e) => {
                tracing::warn!(service = "cinemeta", catalog, error = %e, "catalog search failed");
                Vec::new()
            }
        }
    }

    /// Fetch a meta document, returning `Ok(None)` when the item is absent for
    /// that type (Cinemeta sends `meta: null`).
    async fn get_meta(&self, kind_path: &str, imdb: &str) -> CoreResult<Option<MetaDetail>> {
        let path = format!("/meta/{kind_path}/{imdb}.json");
        let resp: MetaResponse = self.get_json(&path).await?;
        Ok(resp.meta.filter(|m| m.name.is_some()))
    }

    /// Resolve the series meta for an episodic id (shared by seasons/episodes).
    async fn series_meta(&self, ids: &IdSet) -> CoreResult<MetaDetail> {
        let imdb = ids
            .imdb
            .as_deref()
            .ok_or_else(|| CoreError::NotFound("cinemeta requires an IMDB id".into()))?;
        self.get_meta("series", imdb)
            .await?
            .ok_or_else(|| CoreError::NotFound(format!("cinemeta: no series meta for {imdb}")))
    }
}

#[async_trait]
impl MetadataProvider for CinemetaProvider {
    fn name(&self) -> &str {
        "cinemeta"
    }

    async fn search(&self, query: &str, kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        let out = match kind {
            Some(MediaKind::Movie) => self.search_catalog("movie", query, MediaKind::Movie).await,
            Some(MediaKind::Series) => {
                self.search_catalog("series", query, MediaKind::Series)
                    .await
            }
            // Anime is AniList's domain (it carries the MAL/AniList ids); stay out.
            Some(MediaKind::Anime) => Vec::new(),
            None => {
                let mut movies = self.search_catalog("movie", query, MediaKind::Movie).await;
                let mut series = self
                    .search_catalog("series", query, MediaKind::Series)
                    .await;
                movies.append(&mut series);
                movies
            }
        };
        Ok(out)
    }

    async fn details(&self, ids: &IdSet) -> CoreResult<Media> {
        let imdb = ids
            .imdb
            .as_deref()
            .ok_or_else(|| CoreError::NotFound("cinemeta requires an IMDB id".into()))?;
        // Series first: that endpoint is a clean miss (null) for non-series ids,
        // whereas the movie endpoint can echo an unrelated item for a series id.
        if let Some(meta) = self.get_meta("series", imdb).await? {
            return Ok(meta.into_media(MediaKind::Series));
        }
        if let Some(meta) = self.get_meta("movie", imdb).await? {
            return Ok(meta.into_media(MediaKind::Movie));
        }
        Err(CoreError::NotFound(format!("cinemeta: no meta for {imdb}")))
    }

    async fn seasons(&self, ids: &IdSet) -> CoreResult<Vec<Season>> {
        let meta = self.series_meta(ids).await?;
        let mut counts: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
        for v in &meta.videos {
            if let Some(s) = v.season {
                if s > 0 {
                    *counts.entry(s).or_default() += 1;
                }
            }
        }
        Ok(counts
            .into_iter()
            .map(|(number, episode_count)| Season {
                number,
                episode_count,
                name: None,
            })
            .collect())
    }

    async fn episodes(&self, ids: &IdSet, season: u32) -> CoreResult<Vec<Episode>> {
        let meta = self.series_meta(ids).await?;
        let mut episodes: Vec<Episode> = meta
            .videos
            .into_iter()
            .filter(|v| v.season == Some(season))
            .map(|v| Episode {
                season,
                number: v.number.or(v.episode).unwrap_or(0),
                title: non_empty(v.name),
                air_date: v
                    .released
                    .or(v.first_aired)
                    .and_then(|d| d.get(0..10).map(str::to_string)),
                overview: non_empty(v.overview.or(v.description)),
                runtime_minutes: None,
                rating: v.rating.as_deref().and_then(|r| r.parse().ok()),
                still: non_empty(v.thumbnail),
            })
            .collect();
        // Cinemeta's `videos` can arrive slightly out of order; present them by
        // episode number.
        episodes.sort_by_key(|e| e.number);
        Ok(episodes)
    }
}

// --- Cinemeta response shapes ---

#[derive(Debug, Deserialize)]
struct CatalogResponse {
    #[serde(default)]
    metas: Vec<CatalogMeta>,
}

#[derive(Debug, Deserialize)]
struct CatalogMeta {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    imdb_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    poster: Option<String>,
    #[serde(default, rename = "releaseInfo")]
    release_info: Option<String>,
}

impl CatalogMeta {
    fn into_media(self, kind: MediaKind) -> Option<Media> {
        let imdb = self.imdb_id.or(self.id).filter(|s| s.starts_with("tt"))?;
        let title = self.name.filter(|s| !s.is_empty())?;
        Some(Media {
            kind,
            ids: IdSet::default().with_imdb(imdb),
            title,
            original_title: None,
            year: parse_year(self.release_info.as_deref()),
            score: None,
            overview: None,
            poster: non_empty(self.poster),
            genres: Vec::new(),
            status: None,
            episode_count: None,
            season_count: None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct MetaResponse {
    #[serde(default)]
    meta: Option<MetaDetail>,
}

#[derive(Debug, Deserialize)]
struct MetaDetail {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    imdb_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    poster: Option<String>,
    #[serde(default)]
    genres: Option<Vec<String>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "imdbRating")]
    imdb_rating: Option<String>,
    #[serde(default, rename = "releaseInfo")]
    release_info: Option<String>,
    #[serde(default)]
    year: Option<String>,
    #[serde(default)]
    videos: Vec<VideoEntry>,
}

impl MetaDetail {
    fn into_media(self, kind: MediaKind) -> Media {
        let imdb = self.imdb_id.or(self.id);
        let mut ids = IdSet::default();
        if let Some(imdb) = imdb.filter(|s| s.starts_with("tt")) {
            ids = ids.with_imdb(imdb);
        }
        let season_count = self
            .videos
            .iter()
            .filter_map(|v| v.season)
            .filter(|s| *s > 0)
            .collect::<std::collections::BTreeSet<_>>()
            .len() as u32;
        let episode_count = self.videos.iter().filter(|v| v.season != Some(0)).count() as u32;
        let release = self.release_info.or(self.year);
        Media {
            kind,
            ids,
            title: self.name.unwrap_or_default(),
            original_title: None,
            year: parse_year(release.as_deref()),
            score: self.imdb_rating.as_deref().and_then(|r| r.parse().ok()),
            overview: non_empty(self.description),
            poster: non_empty(self.poster),
            genres: self.genres.unwrap_or_default(),
            status: self.status,
            episode_count: (episode_count > 0).then_some(episode_count),
            season_count: (season_count > 0).then_some(season_count),
        }
    }
}

#[derive(Debug, Deserialize)]
struct VideoEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    season: Option<u32>,
    #[serde(default)]
    number: Option<u32>,
    #[serde(default)]
    episode: Option<u32>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    released: Option<String>,
    #[serde(default, rename = "firstAired")]
    first_aired: Option<String>,
    #[serde(default)]
    rating: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
}

/// Parse a leading 4-digit year from a `releaseInfo` like `"2014"` or
/// `"2008–2013"` (en-dash range) or `"2008–"`.
fn parse_year(s: Option<&str>) -> Option<i32> {
    let s = s?;
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() == 4 {
        digits.parse().ok()
    } else {
        None
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_year_handles_movie_and_series_ranges() {
        assert_eq!(parse_year(Some("2014")), Some(2014));
        assert_eq!(parse_year(Some("2008–2013")), Some(2008)); // en-dash
        assert_eq!(parse_year(Some("2008-2013")), Some(2008)); // hyphen
        assert_eq!(parse_year(Some("2008–")), Some(2008));
        assert_eq!(parse_year(Some("")), None);
        assert_eq!(parse_year(None), None);
    }

    #[test]
    fn catalog_meta_maps_imdb_and_skips_non_tt() {
        let good = CatalogMeta {
            id: Some("tt0816692".into()),
            imdb_id: Some("tt0816692".into()),
            name: Some("Interstellar".into()),
            poster: Some("http://img/p.jpg".into()),
            release_info: Some("2014".into()),
        };
        let m = good.into_media(MediaKind::Movie).unwrap();
        assert_eq!(m.ids.imdb.as_deref(), Some("tt0816692"));
        assert_eq!(m.year, Some(2014));
        assert_eq!(m.kind, MediaKind::Movie);

        // Kitsu-style non-tt id is skipped (Torrentio could not use it).
        let bad = CatalogMeta {
            id: Some("kitsu:42".into()),
            imdb_id: None,
            name: Some("Whatever".into()),
            poster: None,
            release_info: None,
        };
        assert!(bad.into_media(MediaKind::Series).is_none());
    }

    #[test]
    fn meta_detail_counts_seasons_and_episodes_ignoring_specials() {
        let meta = MetaDetail {
            id: Some("tt0903747".into()),
            imdb_id: Some("tt0903747".into()),
            name: Some("Breaking Bad".into()),
            description: Some("desc".into()),
            poster: None,
            genres: Some(vec!["Crime".into()]),
            status: None,
            imdb_rating: Some("9.5".into()),
            release_info: Some("2008–2013".into()),
            year: None,
            videos: vec![
                VideoEntry {
                    season: Some(0),
                    number: Some(1),
                    ..blank_video()
                },
                VideoEntry {
                    season: Some(1),
                    number: Some(1),
                    ..blank_video()
                },
                VideoEntry {
                    season: Some(1),
                    number: Some(2),
                    ..blank_video()
                },
                VideoEntry {
                    season: Some(2),
                    number: Some(1),
                    ..blank_video()
                },
            ],
        };
        let m = meta.into_media(MediaKind::Series);
        assert_eq!(m.score, Some(9.5));
        assert_eq!(m.season_count, Some(2)); // S0 specials excluded
        assert_eq!(m.episode_count, Some(3)); // S0 episode excluded
        assert_eq!(m.year, Some(2008));
    }

    fn blank_video() -> VideoEntry {
        VideoEntry {
            name: None,
            season: None,
            number: None,
            episode: None,
            overview: None,
            description: None,
            released: None,
            first_aired: None,
            rating: None,
            thumbnail: None,
        }
    }
}
