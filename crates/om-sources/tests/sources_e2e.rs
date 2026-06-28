//! End-to-end integration tests for the source adapters against a mock server.

use om_core::model::{IdSet, Media, MediaKind};
use om_core::ports::{SourceProvider, SourceQuery};
use om_core::stream::{CacheState, Quality};
use om_sources::{NyaaSource, TorrentioSource};
use serde_json::json;
use wiremock::matchers::{method, path, query_param, query_param_contains};
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
        absolute_episode: None,
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
        absolute_episode: None,
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
        absolute_episode: None,
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
        absolute_episode: None,
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
async fn nyaa_filters_out_wrong_season_releases() {
    let server = MockServer::start().await;

    // A mixed feed: S1 single, S1 batch (no marker), S2 single, S2 batch.
    let rss = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[SubsPlease] Kage no Jitsuryokusha ni Naritakute! - 01 (1080p) [AAAA].mkv</title>
      <nyaa:seeders>100</nyaa:seeders><nyaa:size>1.3 GiB</nyaa:size>
      <nyaa:infoHash>1111111111111111111111111111111111111111</nyaa:infoHash>
    </item>
    <item>
      <title>[Erai-raws] Kage no Jitsuryokusha ni Naritakute! - 01 ~ 20 [1080p][Batch]</title>
      <nyaa:seeders>90</nyaa:seeders><nyaa:size>7.0 GiB</nyaa:size>
      <nyaa:infoHash>2222222222222222222222222222222222222222</nyaa:infoHash>
    </item>
    <item>
      <title>[SubsPlease] Kage no Jitsuryokusha ni Naritakute! S2 - 01 (1080p) [BBBB].mkv</title>
      <nyaa:seeders>80</nyaa:seeders><nyaa:size>1.3 GiB</nyaa:size>
      <nyaa:infoHash>3333333333333333333333333333333333333333</nyaa:infoHash>
    </item>
    <item>
      <title>[Erai-raws] Kage no Jitsuryokusha ni Naritakute! 2nd Season - 01 ~ 12 [1080p][Batch]</title>
      <nyaa:seeders>70</nyaa:seeders><nyaa:size>5.0 GiB</nyaa:size>
      <nyaa:infoHash>4444444444444444444444444444444444444444</nyaa:infoHash>
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
    let mut m = media(
        MediaKind::Anime,
        IdSet::default().with_anilist(1),
        "The Eminence in Shadow",
    );
    // nyaa keys off the romaji original title.
    m.original_title = Some("Kage no Jitsuryokusha ni Naritakute!".into());
    let q = SourceQuery {
        media: m,
        season: Some(1),
        episode: Some(1),
        absolute_episode: None,
        include_uncached: false,
    };

    let candidates = src.find(&q).await.unwrap();

    // Only the two S1 releases (single + batch) survive; both S2 ones are dropped.
    assert_eq!(candidates.len(), 2);
    assert!(candidates
        .iter()
        .all(|c| !c.title.contains("S2") && !c.title.contains("2nd Season")));
}

/// A continuously-numbered sequel: S2E01 is published on nyaa only as `… - 21`
/// (S1 had 20 episodes), with no season marker. The engine supplies
/// `absolute_episode = 21`; nyaa must issue a second search for `… 21` and accept
/// the marker-less `- 21` release as the requested S2 episode.
#[tokio::test]
async fn nyaa_matches_absolute_numbered_sequel_episode() {
    let server = MockServer::start().await;

    // The relative-episode search (`… 01`) only returns the S1 premiere.
    let rss_ep01 = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[SubsPlease] Kage no Jitsuryokusha ni Naritakute! - 01 (1080p) [S1EP1].mkv</title>
      <nyaa:seeders>500</nyaa:seeders><nyaa:size>1.3 GiB</nyaa:size>
      <nyaa:infoHash>1111111111111111111111111111111111111111</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    // The absolute-episode search (`… 21`) returns the continuously-numbered S2
    // premiere — no "S2"/"2nd Season" marker anywhere in the title.
    let rss_ep21 = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[SubsPlease] Kage no Jitsuryokusha ni Naritakute! - 21 (1080p) [ABSOLUTE].mkv</title>
      <nyaa:seeders>400</nyaa:seeders><nyaa:size>1.3 GiB</nyaa:size>
      <nyaa:infoHash>2121212121212121212121212121212121212121</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .and(query_param("page", "rss"))
        .and(query_param_contains("q", "21"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss_ep21))
        .mount(&server)
        .await;
    // Fallback for the `… 01` search (and any other query): the S1-only feed.
    Mock::given(method("GET"))
        .and(query_param("page", "rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss_ep01))
        .mount(&server)
        .await;

    let src = NyaaSource::with_base_url(server.uri());
    let mut m = media(
        MediaKind::Anime,
        IdSet::default().with_anilist(2),
        "The Eminence in Shadow",
    );
    // S2's AniList entry carries the "2nd Season" romaji title → ordinal 2.
    m.original_title = Some("Kage no Jitsuryokusha ni Naritakute! 2nd Season".into());
    let q = SourceQuery {
        media: m,
        season: Some(2),
        episode: Some(1),
        // offset (S1 = 20 eps) + episode 1 = absolute 21.
        absolute_episode: Some(21),
        include_uncached: false,
    };

    let candidates = src.find(&q).await.unwrap();

    // The marker-less `- 21` release is recognized as the requested S2 episode…
    assert!(
        candidates.iter().any(|c| c.title.contains("- 21")),
        "absolute-numbered S2 release should be matched, got: {:?}",
        candidates.iter().map(|c| &c.title).collect::<Vec<_>>()
    );
    // …and the S1 `- 01` premiere is NOT (it's season 1, not the requested S2).
    assert!(
        !candidates.iter().any(|c| c.title.contains("- 01")),
        "S1 premiere must not leak into the S2 result"
    );
}

/// The mirror of the above: a *season 1* search must not pull in a `… - 21`
/// release. A true S1 (no prequel → `absolute_episode` is `None`) issues only the
/// relative `… 01` search and never the second absolute `… 21` fetch, so the
/// `- 21` file — served only on the `q=…21` query — never reaches the results.
#[tokio::test]
async fn nyaa_season_one_does_not_fetch_or_match_absolute_release() {
    let server = MockServer::start().await;

    // The `… 21` query is the ONLY place the absolute release is served. If S1
    // wrongly issued the second fetch, this `- 21` item would appear.
    let rss_ep21 = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[SubsPlease] Kage no Jitsuryokusha ni Naritakute! - 21 (1080p) [ABSOLUTE].mkv</title>
      <nyaa:seeders>400</nyaa:seeders><nyaa:size>1.3 GiB</nyaa:size>
      <nyaa:infoHash>2121212121212121212121212121212121212121</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    // Every other query (the `… 01` relative search) gets the S1 premiere only.
    let rss_ep01 = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[SubsPlease] Kage no Jitsuryokusha ni Naritakute! - 01 (1080p) [S1EP1].mkv</title>
      <nyaa:seeders>500</nyaa:seeders><nyaa:size>1.3 GiB</nyaa:size>
      <nyaa:infoHash>1111111111111111111111111111111111111111</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .and(query_param("page", "rss"))
        .and(query_param_contains("q", "21"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss_ep21))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("page", "rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss_ep01))
        .mount(&server)
        .await;

    let src = NyaaSource::with_base_url(server.uri());
    let mut m = media(
        MediaKind::Anime,
        IdSet::default().with_anilist(1),
        "The Eminence in Shadow",
    );
    m.original_title = Some("Kage no Jitsuryokusha ni Naritakute!".into());
    let q = SourceQuery {
        media: m,
        season: Some(1),
        episode: Some(1),
        // True season 1: no prior-seasons offset, so no absolute number.
        absolute_episode: None,
        include_uncached: false,
    };

    let candidates = src.find(&q).await.unwrap();

    // The S1 premiere is returned, and the `- 21` absolute release is absent —
    // S1 never issued the second fetch nor matched the marker-less `- 21`.
    assert!(
        candidates.iter().any(|c| c.title.contains("- 01")),
        "S1 premiere should be present"
    );
    assert!(
        !candidates.iter().any(|c| c.title.contains("- 21")),
        "S1 search must not pick up the absolute-numbered `- 21` release"
    );
}

#[tokio::test]
async fn nyaa_does_not_support_movies() {
    let src = NyaaSource::new();
    assert!(!src.supports(MediaKind::Movie));
    assert!(src.supports(MediaKind::Anime));
}
