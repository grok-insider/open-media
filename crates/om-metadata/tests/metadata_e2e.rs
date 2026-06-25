//! End-to-end integration tests for the metadata adapters.
//!
//! These exercise the real reqwest client + JSON parsing against an in-process
//! mock HTTP server (`wiremock`), so the full request/response path is covered
//! without touching the live TMDB/AniList services.

use om_core::model::{IdSet, MediaKind};
use om_core::ports::MetadataProvider;
use om_metadata::{AniListProvider, TmdbProvider};
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
