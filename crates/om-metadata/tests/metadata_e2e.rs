//! End-to-end integration tests for the metadata adapters.
//!
//! These exercise the real reqwest client + JSON parsing against an in-process
//! mock HTTP server (`wiremock`), so the full request/response path is covered
//! without touching the live TMDB/AniList services.

use om_core::model::{IdSet, MediaKind};
use om_core::ports::MetadataProvider;
use om_metadata::{AniListProvider, CinemetaProvider, TmdbProvider};
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn tmdb_search_multi_maps_results_and_skips_person() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/search/multi"))
        .and(query_param("query", "frieren"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {
                    "id": 209867,
                    "media_type": "tv",
                    "name": "Frieren: Beyond Journey's End",
                    "first_air_date": "2023-09-29",
                    "overview": "After the party of heroes defeated the Demon King...",
                    "poster_path": "/dqZENchTd7lp5zht7BQpqQkutwc.jpg",
                    "vote_average": 8.7
                },
                {
                    "id": 5,
                    "media_type": "person",
                    "name": "Some Voice Actor"
                }
            ]
        })))
        .mount(&server)
        .await;

    let provider = TmdbProvider::with_base_url("test_key", server.uri());
    let results = provider.search("frieren", None).await.unwrap();

    assert_eq!(results.len(), 1, "person result must be filtered out");
    let m = &results[0];
    assert_eq!(m.kind, MediaKind::Series);
    assert_eq!(m.ids.tmdb, Some(209867));
    assert_eq!(m.title, "Frieren: Beyond Journey's End");
    assert_eq!(m.year, Some(2023));
    assert!(m
        .poster
        .as_deref()
        .unwrap()
        .starts_with("https://image.tmdb.org"));
}

#[tokio::test]
async fn tmdb_details_falls_back_from_movie_to_tv_and_extracts_imdb() {
    let server = MockServer::start().await;

    // /movie/{id} 404s (it's actually a TV show) → adapter must fall back to /tv.
    Mock::given(method("GET"))
        .and(path("/movie/209867"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "status_code": 34, "status_message": "Not found"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/tv/209867"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 209867,
            "name": "Frieren: Beyond Journey's End",
            "first_air_date": "2023-09-29",
            "number_of_seasons": 1,
            "number_of_episodes": 28,
            "status": "Returning Series",
            "genres": [{ "id": 1, "name": "Animation" }],
            "external_ids": { "imdb_id": "tt22248376" }
        })))
        .mount(&server)
        .await;

    let provider = TmdbProvider::with_base_url("test_key", server.uri());
    let media = provider
        .details(&IdSet::default().with_tmdb(209867))
        .await
        .unwrap();

    assert_eq!(media.kind, MediaKind::Series);
    assert_eq!(media.ids.imdb.as_deref(), Some("tt22248376"));
    assert_eq!(media.season_count, Some(1));
    assert_eq!(media.episode_count, Some(28));
    assert_eq!(media.genres, vec!["Animation".to_string()]);
}

#[tokio::test]
async fn tmdb_episodes_lists_a_season() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/tv/209867/season/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "episodes": [
                { "episode_number": 1, "name": "The Journey's End", "air_date": "2023-09-29", "runtime": 24, "vote_average": 8.5 },
                { "episode_number": 2, "name": "It Didn't Have to Be Magic", "air_date": "2023-09-29" }
            ]
        })))
        .mount(&server)
        .await;

    let provider = TmdbProvider::with_base_url("test_key", server.uri());
    let eps = provider
        .episodes(&IdSet::default().with_tmdb(209867), 1)
        .await
        .unwrap();

    assert_eq!(eps.len(), 2);
    assert_eq!(eps[0].number, 1);
    assert_eq!(eps[0].title.as_deref(), Some("The Journey's End"));
    assert_eq!(eps[0].runtime_minutes, Some(24));
    assert_eq!(eps[0].coordinate(), "S01E01");
}

#[tokio::test]
async fn anilist_search_maps_anime_with_mal_bridge() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(body_string_contains("media(search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "Page": { "media": [
                {
                    "id": 154587,
                    "idMal": 52991,
                    "title": { "romaji": "Sousou no Frieren", "english": "Frieren", "native": "葬送のフリーレン" },
                    "seasonYear": 2023,
                    "episodes": 28,
                    "averageScore": 89,
                    "description": "After the party of heroes...",
                    "coverImage": { "large": "https://img/frieren.jpg" },
                    "status": "FINISHED",
                    "genres": ["Adventure", "Fantasy"]
                }
            ] } }
        })))
        .mount(&server)
        .await;

    let provider = AniListProvider::with_base_url(server.uri());
    let results = provider
        .search("frieren", Some(MediaKind::Anime))
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    let m = &results[0];
    assert_eq!(m.kind, MediaKind::Anime);
    assert_eq!(m.ids.anilist, Some(154587));
    assert_eq!(m.ids.mal, Some(52991), "MAL bridge is needed for AniSkip");
    assert_eq!(m.title, "Frieren");
    assert_eq!(m.score, Some(8.9));
}

/// Absolute-numbering offset: a season-2 entry whose relation graph has a TV
/// `PREQUEL` (S1, 20 episodes) yields an offset of 20, walking the chain until a
/// season with no further TV prequel.
#[tokio::test]
async fn anilist_episode_offset_sums_tv_prequels() {
    let server = MockServer::start().await;

    // DETAIL_QUERY for the S2 entry (id 2): its PREQUEL is S1 (id 1, 20 eps, TV).
    // A non-TV ADAPTATION edge and a SEQUEL edge must be ignored.
    Mock::given(method("POST"))
        .and(body_string_contains(r#"{"id":2}"#))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "Media": {
                "id": 2,
                "title": { "romaji": "Eminence in Shadow 2nd Season" },
                "episodes": 12,
                "format": "TV",
                "relations": { "edges": [
                    { "relationType": "PREQUEL", "node": { "id": 1, "format": "TV", "episodes": 20 } },
                    { "relationType": "SEQUEL",  "node": { "id": 3, "format": "TV", "episodes": 12 } },
                    { "relationType": "ADAPTATION", "node": { "id": 9, "format": "MANGA", "episodes": null } }
                ] }
            } }
        })))
        .mount(&server)
        .await;

    // RELATIONS_QUERY for S1 (id 1): no TV prequel → the walk stops here.
    Mock::given(method("POST"))
        .and(body_string_contains(r#"{"id":1}"#))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "Media": {
                "id": 1,
                "episodes": 20,
                "format": "TV",
                "relations": { "edges": [
                    { "relationType": "SEQUEL", "node": { "id": 2, "format": "TV", "episodes": 12 } }
                ] }
            } }
        })))
        .mount(&server)
        .await;

    let provider = AniListProvider::with_base_url(server.uri());
    let offset = provider
        .episode_offset(&IdSet::default().with_anilist(2))
        .await
        .unwrap();
    assert_eq!(offset, Some(20), "S1's 20 episodes are the S2 offset");
}

/// A true season 1 (no TV prequel in its relation graph) has no offset, so
/// absolute matching stays disabled (`None`, not `Some(0)`).
#[tokio::test]
async fn anilist_episode_offset_none_without_prequel() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "Media": {
                "id": 1,
                "title": { "romaji": "Standalone Show" },
                "episodes": 12,
                "format": "TV",
                "relations": { "edges": [
                    { "relationType": "ADAPTATION", "node": { "id": 9, "format": "MANGA", "episodes": null } }
                ] }
            } }
        })))
        .mount(&server)
        .await;

    let provider = AniListProvider::with_base_url(server.uri());
    let offset = provider
        .episode_offset(&IdSet::default().with_anilist(1))
        .await
        .unwrap();
    assert_eq!(offset, None);
}

#[tokio::test]
async fn anilist_stays_out_of_movie_searches() {
    // No mock needed: AniList must not even hit the network for a movie filter.
    let provider = AniListProvider::with_base_url("http://127.0.0.1:1/should-not-be-called");
    let results = provider
        .search("dune", Some(MediaKind::Movie))
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn anilist_propagates_graphql_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "errors": [{ "message": "Invalid token" }]
        })))
        .mount(&server)
        .await;

    let provider = AniListProvider::with_base_url(server.uri());
    let err = provider.search("x", None).await.unwrap_err();
    assert!(err.to_string().contains("Invalid token"));
}

// --- Cinemeta (keyless) ---

#[tokio::test]
async fn cinemeta_search_movie_maps_imdb_id_and_year() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/catalog/movie/top/search=interstellar.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metas": [
                {
                    "id": "tt0816692",
                    "imdb_id": "tt0816692",
                    "type": "movie",
                    "name": "Interstellar",
                    "poster": "https://img/p.jpg",
                    "releaseInfo": "2014"
                }
            ]
        })))
        .mount(&server)
        .await;

    let provider = CinemetaProvider::with_base_url(server.uri());
    let results = provider
        .search("interstellar", Some(MediaKind::Movie))
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    let m = &results[0];
    assert_eq!(m.kind, MediaKind::Movie);
    assert_eq!(m.ids.imdb.as_deref(), Some("tt0816692"));
    assert_eq!(m.title, "Interstellar");
    assert_eq!(m.year, Some(2014));
}

#[tokio::test]
async fn cinemeta_search_none_queries_both_catalogs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/catalog/movie/top/search=breaking%20bad.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "metas": [] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/catalog/series/top/search=breaking%20bad.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metas": [
                { "id": "tt0903747", "type": "series", "name": "Breaking Bad", "releaseInfo": "2008–2013" }
            ]
        })))
        .mount(&server)
        .await;

    let provider = CinemetaProvider::with_base_url(server.uri());
    let results = provider.search("breaking bad", None).await.unwrap();

    assert_eq!(results.len(), 1, "series catalog result must appear");
    assert_eq!(results[0].kind, MediaKind::Series);
    assert_eq!(results[0].ids.imdb.as_deref(), Some("tt0903747"));
    assert_eq!(results[0].year, Some(2008));
}

#[tokio::test]
async fn cinemeta_skips_anime_without_network() {
    // Anime is AniList's domain; Cinemeta must not hit the network for it.
    let provider = CinemetaProvider::with_base_url("http://127.0.0.1:1/should-not-be-called");
    let results = provider
        .search("frieren", Some(MediaKind::Anime))
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn cinemeta_details_tries_series_first_then_movie() {
    let server = MockServer::start().await;
    // Series miss → meta: null (Cinemeta's clean-miss shape).
    Mock::given(method("GET"))
        .and(path("/meta/series/tt0816692.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "meta": null })))
        .mount(&server)
        .await;
    // Movie hit.
    Mock::given(method("GET"))
        .and(path("/meta/movie/tt0816692.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "meta": {
                "id": "tt0816692",
                "imdb_id": "tt0816692",
                "type": "movie",
                "name": "Interstellar",
                "releaseInfo": "2014",
                "imdbRating": "8.7",
                "genres": ["Adventure", "Drama", "Sci-Fi"]
            }
        })))
        .mount(&server)
        .await;

    let provider = CinemetaProvider::with_base_url(server.uri());
    let ids = IdSet::default().with_imdb("tt0816692");
    let m = provider.details(&ids).await.unwrap();
    assert_eq!(m.kind, MediaKind::Movie);
    assert_eq!(m.title, "Interstellar");
    assert_eq!(m.score, Some(8.7));
    assert_eq!(m.genres, vec!["Adventure", "Drama", "Sci-Fi"]);
}

#[tokio::test]
async fn cinemeta_episodes_parsed_from_videos() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/meta/series/tt0903747.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "meta": {
                "id": "tt0903747",
                "imdb_id": "tt0903747",
                "type": "series",
                "name": "Breaking Bad",
                "videos": [
                    { "season": 0, "number": 1, "name": "Special", "released": "2009-02-17T05:00:00.000Z" },
                    { "season": 1, "number": 1, "episode": 1, "name": "Pilot", "released": "2008-01-21T05:00:00.000Z", "rating": "7.7" },
                    { "season": 1, "number": 2, "episode": 2, "name": "Cat's in the Bag...", "released": "2008-01-28T05:00:00.000Z" }
                ]
            }
        })))
        .mount(&server)
        .await;

    let provider = CinemetaProvider::with_base_url(server.uri());
    let ids = IdSet::default().with_imdb("tt0903747");

    let seasons = provider.seasons(&ids).await.unwrap();
    assert_eq!(seasons.len(), 1, "S0 specials excluded");
    assert_eq!(seasons[0].number, 1);
    assert_eq!(seasons[0].episode_count, 2);

    let eps = provider.episodes(&ids, 1).await.unwrap();
    assert_eq!(eps.len(), 2);
    assert_eq!(eps[0].number, 1);
    assert_eq!(eps[0].title.as_deref(), Some("Pilot"));
    assert_eq!(eps[0].air_date.as_deref(), Some("2008-01-21"));
    assert_eq!(eps[0].rating, Some(7.7));
}
