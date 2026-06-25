//! End-to-end integration tests for the source adapters against a mock server.

use om_core::model::{IdSet, Media, MediaKind};
use om_core::ports::{SourceProvider, SourceQuery};
use om_core::stream::{CacheState, Quality};
use om_sources::{NyaaSource, TorrentioSource};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn media(kind: MediaKind, ids: IdSet, title: &str) -> Media {
    Media {
        kind,
        ids,
        title: title.to_string(),
        original_title: None,
        year: None,
        score: None,
        overview: None,
        poster: None,
        genres: vec![],
        status: None,
        episode_count: None,
        season_count: None,
    }
}

#[tokio::test]
async fn torrentio_movie_parses_cached_and_uncached() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/testcfg/stream/movie/tt0816692.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "streams": [
                {
                    "name": "[RD+] 1337x",
                    "title": "Interstellar.2014.2160p.BluRay.REMUX.HEVC.DTS-HD.MA.5.1\n👤 320 💾 54.2 GB",
                    "url": "https://rd.example/abc/stream.mkv"
                },
                {
                    "name": "[RD download] torrentgalaxy",
                    "title": "Interstellar.2014.1080p.WEB.x264\n👤 12 💾 2.3 GB",
                    "infoHash": "abcdef0123456789abcdef0123456789abcdef01",
                    "fileIdx": 0
                }
            ]
        })))
        .mount(&server)
        .await;

    let src = TorrentioSource::with_base_url("testcfg", server.uri());
    let q = SourceQuery {
        media: media(
            MediaKind::Movie,
            IdSet::default().with_imdb("tt0816692"),
            "Interstellar",
        ),
        season: None,
        episode: None,
        include_uncached: true,
    };
    let candidates = src.find(&q).await.unwrap();

    assert_eq!(candidates.len(), 2);

    let cached = &candidates[0];
    assert_eq!(cached.cache, CacheState::Cached);
    assert_eq!(cached.quality, Quality::P2160);
    assert!(cached.direct_url.is_some());
    assert_eq!(cached.seeders, Some(320));
    assert_eq!(cached.tags.video_codec.as_deref(), Some("HEVC"));

    let uncached = &candidates[1];
    assert_eq!(uncached.cache, CacheState::Uncached);
    assert_eq!(uncached.quality, Quality::P1080);
    assert!(uncached.info_hash.is_some());
    assert!(uncached
        .magnet
        .as_deref()
        .unwrap()
        .starts_with("magnet:?xt=urn:btih:"));
}

#[tokio::test]
async fn torrentio_series_uses_season_episode_path() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/testcfg/stream/series/tt22248376:1:1.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "streams": [
                { "name": "[RD+] nyaasi", "title": "Frieren S01E01 1080p\n👤 99 💾 1.3 GB", "url": "https://rd/x.mkv" }
            ]
        })))
        .mount(&server)
        .await;

    let src = TorrentioSource::with_base_url("testcfg", server.uri());
    let q = SourceQuery {
        media: media(
            MediaKind::Anime,
            IdSet::default().with_imdb("tt22248376"),
            "Frieren",
        ),
        season: Some(1),
        episode: Some(1),
        include_uncached: false,
    };
    let candidates = src.find(&q).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].provider, "nyaasi");
}

#[tokio::test]
async fn torrentio_without_imdb_returns_empty() {
    // An AniList-only anime (no IMDB id) makes Torrentio a clean no-op, not an
    // error — nyaa serves anime. The base URL is unreachable to prove no request
    // is even attempted.
    let src = TorrentioSource::with_base_url("testcfg", "http://127.0.0.1:1");
    let q = SourceQuery {
        media: media(
            MediaKind::Anime,
            IdSet::default().with_anilist(1),
            "AniList only",
        ),
        season: Some(1),
        episode: Some(1),
        include_uncached: false,
    };
    let out = src.find(&q).await.unwrap();
    assert!(out.is_empty());
}

#[tokio::test]
async fn nyaa_rss_search_returns_candidates() {
    let server = MockServer::start().await;

    let rss = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[SubsPlease] Frieren - 01 (1080p) [ABCD].mkv</title>
      <nyaa:seeders>2500</nyaa:seeders>
      <nyaa:size>1.4 GiB</nyaa:size>
      <nyaa:infoHash>1111111111111111111111111111111111111111</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .and(path("/"))
        .and(query_param("page", "rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss))
        .mount(&server)
        .await;

    let src = NyaaSource::with_base_url(server.uri());
    let q = SourceQuery {
        media: media(
            MediaKind::Anime,
            IdSet::default().with_mal(52991),
            "Frieren",
        ),
        season: Some(1),
        episode: Some(1),
        include_uncached: false,
    };
    let candidates = src.find(&q).await.unwrap();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].provider, "nyaa");
    assert_eq!(candidates[0].quality, Quality::P1080);
    assert_eq!(candidates[0].seeders, Some(2500));
    assert!(candidates[0].is_resolvable());
}

#[tokio::test]
async fn nyaa_does_not_support_movies() {
    let src = NyaaSource::new();
    assert!(!src.supports(MediaKind::Movie));
    assert!(src.supports(MediaKind::Anime));
}
