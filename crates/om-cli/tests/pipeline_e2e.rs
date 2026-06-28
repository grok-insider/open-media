//! Composition-root end-to-end test.
//!
//! Wires the *real* adapters (TMDB, Torrentio, Real-Debrid + the hybrid resolver)
//! into the *real* `Engine` and drives the whole discovery→resolve pipeline
//! against three mock servers — no live services, no mpv. This is the closest
//! thing to "watch a movie" we can assert in CI: search → details (IMDB) →
//! sources (ranked, cache-aware) → resolve (both the cached-direct and the
//! uncached-via-Real-Debrid branches).

use std::sync::Arc;
use std::time::Duration;

use om_app::{Engine, PlayRequest};
use om_core::model::MediaKind;
use om_core::scoring::ScoringPrefs;
use om_core::stream::{CacheState, PlaybackOrigin};
use om_debrid::RealDebrid;
use om_metadata::TmdbProvider;
use om_sources::TorrentioSource;
use om_stream::{HybridResolver, P2pEngine};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn mock_tmdb() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                { "id": 157336, "title": "Interstellar", "release_date": "2014-11-05",
                  "overview": "A team of explorers travel through a wormhole.", "vote_average": 8.4 }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/movie/157336"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 157336, "title": "Interstellar", "release_date": "2014-11-05",
            "status": "Released", "imdb_id": "tt0816692",
            "genres": [{ "id": 878, "name": "Science Fiction" }]
        })))
        .mount(&server)
        .await;
    server
}

async fn mock_torrentio() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/testcfg/stream/movie/tt0816692.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "streams": [
                {
                    "name": "[RD+] 1337x",
                    "title": "Interstellar.2014.1080p.BluRay.x264\n👤 500 💾 2.3 GB",
                    "url": "https://rd-cdn.example/cached/interstellar-1080p.mkv"
                },
                {
                    "name": "[RD download] torrentgalaxy",
                    "title": "Interstellar.2014.2160p.WEB.HEVC\n👤 40 💾 18.0 GB",
                    "infoHash": "abcdef0123456789abcdef0123456789abcdef01",
                    "fileIdx": 0
                }
            ]
        })))
        .mount(&server)
        .await;
    server
}

async fn mock_realdebrid() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/torrents/addMagnet"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "T1", "uri": "x" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/torrents/info/T1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T1", "filename": "Interstellar.2014.2160p.mkv", "status": "downloaded",
            "files": [{ "id": 1, "path": "/Interstellar.2014.2160p.mkv", "bytes": 18000000000_u64 }],
            "links": ["https://real-debrid.com/d/RESTRICTED"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/unrestrict/link"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "filename": "Interstellar.2014.2160p.mkv",
            "download": "https://cdn.real-debrid.com/d/UNRESTRICTED/interstellar-2160p.mkv"
        })))
        .mount(&server)
        .await;
    server
}

fn build_engine(tmdb: &MockServer, torrentio: &MockServer, rd: &MockServer) -> Engine {
    let debrid = Arc::new(
        RealDebrid::with_base_url("rd_token", rd.uri())
            .with_poll_interval(Duration::from_millis(5)),
    );
    let p2p = Arc::new(P2pEngine::new(3131, true));
    Engine::builder()
        .add_metadata(Arc::new(TmdbProvider::with_base_url(
            "tmdb_key",
            tmdb.uri(),
        )))
        .add_source(Arc::new(TorrentioSource::with_base_url(
            "testcfg",
            torrentio.uri(),
        )))
        .resolver(Arc::new(HybridResolver::new(Some(debrid), p2p)))
        .scoring_prefs(ScoringPrefs::default())
        .build()
}

#[tokio::test]
async fn full_pipeline_search_details_sources_resolve() {
    let tmdb = mock_tmdb().await;
    let torrentio = mock_torrentio().await;
    let rd = mock_realdebrid().await;
    let engine = build_engine(&tmdb, &torrentio, &rd);

    // 1. Search.
    let results = engine
        .search("interstellar", Some(MediaKind::Movie))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].ids.tmdb, Some(157336));
    assert!(
        results[0].ids.imdb.is_none(),
        "search shouldn't fetch imdb yet"
    );

    // 2. Details hydrate the IMDB id that Torrentio needs.
    let media = engine.details(&results[0].ids).await.unwrap();
    assert_eq!(media.ids.imdb.as_deref(), Some("tt0816692"));

    // 3. Find + rank sources. prefer_cached ⇒ the cached candidate ranks first
    //    even though the uncached one is higher resolution.
    let req = PlayRequest {
        media,
        season: None,
        episode: None,
        episode_title: None,
        episode_runtime_minutes: None,
        include_uncached: true,
    };
    let candidates = engine.find_sources(&req).await.unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].cache, CacheState::Cached);
    assert!(candidates[0].direct_url.is_some());

    // 4a. Resolve the cached pick → the addon's direct URL, no Real-Debrid call.
    let cached_playback = engine.resolve(&candidates[0]).await.unwrap();
    assert_eq!(cached_playback.origin, PlaybackOrigin::Debrid);
    assert_eq!(
        cached_playback.url,
        "https://rd-cdn.example/cached/interstellar-1080p.mkv"
    );

    // 4b. Resolve the uncached pick → full Real-Debrid add→info→unrestrict flow.
    let uncached = candidates
        .iter()
        .find(|c| c.cache == CacheState::Uncached)
        .expect("an uncached candidate exists");
    let rd_playback = engine.resolve(uncached).await.unwrap();
    assert_eq!(
        rd_playback.url,
        "https://cdn.real-debrid.com/d/UNRESTRICTED/interstellar-2160p.mkv"
    );
}

#[tokio::test]
async fn search_survives_a_failing_provider() {
    // TMDB up, but point a second metadata provider at a dead server: search
    // must still return TMDB's results (failures are logged, not fatal).
    let tmdb = mock_tmdb().await;
    let dead = TmdbProvider::with_base_url("k", "http://127.0.0.1:1");
    let engine = Engine::builder()
        .add_metadata(Arc::new(TmdbProvider::with_base_url("k", tmdb.uri())))
        .add_metadata(Arc::new(dead))
        .build();

    let results = engine
        .search("interstellar", Some(MediaKind::Movie))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
}
