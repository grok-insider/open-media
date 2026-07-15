//! End-to-end integration tests for the source adapters against a mock server.

use open_media_core::model::{IdSet, Media, MediaKind};
use open_media_core::ports::{SourceProvider, SourceQuery};
use open_media_core::stream::{CacheState, Quality};
use open_media_sources::{NyaaSource, TorrentioSource};
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
        kitsu: None,
        imdb_season: None,
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
        kitsu: None,
        imdb_season: None,
        include_uncached: false,
    };
    let candidates = src.find(&q).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].provider, "nyaasi");
}

#[tokio::test]
async fn torrentio_without_imdb_returns_empty() {
    // An AniList-only anime (no IMDB and no kitsu id) makes Torrentio a clean
    // no-op, not an error — nyaa serves anime. The base URL is unreachable to
    // prove no request is even attempted.
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
        kitsu: None,
        imdb_season: None,
        include_uncached: false,
    };
    let out = src.find(&q).await.unwrap();
    assert!(out.is_empty());
}

#[tokio::test]
async fn torrentio_anime_prefers_native_kitsu_addressing() {
    // A bridged anime with a kitsu id is addressed as kitsu:{id}:{ep} — kitsu
    // mirrors AniList's per-entry numbering, so no IMDB season math is needed.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/testcfg/stream/series/kitsu:47099:5.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "streams": [
                { "name": "[RD+] nyaasi", "title": "Eminence S02E05 1080p\n👤 80 💾 1.4 GB", "url": "https://rd/e5.mkv" }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let src = TorrentioSource::with_base_url("testcfg", server.uri());
    let q = SourceQuery {
        media: media(
            MediaKind::Anime,
            IdSet::default()
                .with_anilist(161964)
                .with_imdb("tt14115938"),
            "The Eminence in Shadow Season 2",
        ),
        season: Some(1), // AniList's flat numbering
        episode: Some(5),
        absolute_episode: None,
        kitsu: Some(47099),
        imdb_season: Some(2),
        include_uncached: false,
    };
    let candidates = src.find(&q).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].direct_url.is_some());
}

#[tokio::test]
async fn torrentio_falls_back_to_imdb_at_bridged_season_when_kitsu_is_empty() {
    // kitsu addressing knows nothing for this entry → fall back to the IMDB id,
    // and crucially at imdb_season (2), not AniList's flat season (1).
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/testcfg/stream/series/kitsu:47099:5.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "streams": [] })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/testcfg/stream/series/tt14115938:2:5.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "streams": [
                { "name": "[RD+] subsplease", "title": "Eminence S02E05 1080p\n👤 55 💾 1.4 GB", "url": "https://rd/imdb-e5.mkv" }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let src = TorrentioSource::with_base_url("testcfg", server.uri());
    let q = SourceQuery {
        media: media(
            MediaKind::Anime,
            IdSet::default()
                .with_anilist(161964)
                .with_imdb("tt14115938"),
            "The Eminence in Shadow Season 2",
        ),
        season: Some(1),
        episode: Some(5),
        absolute_episode: None,
        kitsu: Some(47099),
        imdb_season: Some(2),
        include_uncached: false,
    };
    let candidates = src.find(&q).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].direct_url.as_deref(),
        Some("https://rd/imdb-e5.mkv")
    );
}

#[tokio::test]
async fn torrentio_anime_movie_uses_kitsu_movie_path_without_imdb() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/testcfg/stream/movie/kitsu:48346.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "streams": [
                { "name": "[RD+] nyaasi", "title": "Lost Echoes 1080p\n👤 40 💾 3.1 GB", "url": "https://rd/movie.mkv" }
            ]
        })))
        .mount(&server)
        .await;

    let src = TorrentioSource::with_base_url("testcfg", server.uri());
    let q = SourceQuery {
        media: media(
            MediaKind::Movie,
            IdSet::default().with_anilist(171952),
            "The Eminence in Shadow: Lost Echoes",
        ),
        season: None,
        episode: None,
        absolute_episode: None,
        kitsu: Some(48346),
        imdb_season: None,
        include_uncached: false,
    };
    let candidates = src.find(&q).await.unwrap();
    assert_eq!(candidates.len(), 1);
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
        kitsu: None,
        imdb_season: None,
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
        kitsu: None,
        imdb_season: None,
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
        kitsu: None,
        imdb_season: None,
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
        kitsu: None,
        imdb_season: None,
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

/// Multi-season TMDB/Cinemeta franchise: plain title (no "Season 3" suffix) with
/// `season = 3` on the query. Must keep S3-tagged releases and drop S1 packs —
/// not treat the unmarked title as season 1.
#[tokio::test]
async fn nyaa_uses_query_season_for_multi_season_franchise_title() {
    let server = MockServer::start().await;

    // Bare `{base} 01` feed is dominated by S1 (and a stray S2 pack).
    let rss_bare = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[EMBER] Mushoku Tensei - Jobless Reincarnation (Season 1) [BD 1080p]</title>
      <nyaa:seeders>600</nyaa:seeders><nyaa:size>17.0 GiB</nyaa:size>
      <nyaa:infoHash>1111111111111111111111111111111111111111</nyaa:infoHash>
    </item>
    <item>
      <title>[SubsPlease] Mushoku Tensei - 01 (1080p) [S1EP1].mkv</title>
      <nyaa:seeders>500</nyaa:seeders><nyaa:size>1.4 GiB</nyaa:size>
      <nyaa:infoHash>2222222222222222222222222222222222222222</nyaa:infoHash>
    </item>
    <item>
      <title>[SubsPlease] Mushoku Tensei II - 01 (1080p) [S2].mkv</title>
      <nyaa:seeders>200</nyaa:seeders><nyaa:size>1.4 GiB</nyaa:size>
      <nyaa:infoHash>3333333333333333333333333333333333333333</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    // Season-tagged search surfaces the real S3 premiere.
    let rss_s3 = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[SubsPlease] Mushoku Tensei S3 - 01 (1080p) [S3EP1].mkv</title>
      <nyaa:seeders>150</nyaa:seeders><nyaa:size>1.5 GiB</nyaa:size>
      <nyaa:infoHash>4444444444444444444444444444444444444444</nyaa:infoHash>
    </item>
    <item>
      <title>[Erai-raws] Mushoku Tensei Season 3 - 01 [1080p][Multiple Subtitle]</title>
      <nyaa:seeders>120</nyaa:seeders><nyaa:size>1.4 GiB</nyaa:size>
      <nyaa:infoHash>5555555555555555555555555555555555555555</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .and(query_param("page", "rss"))
        .and(query_param_contains("q", "S3"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss_s3))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("page", "rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss_bare))
        .mount(&server)
        .await;

    let src = NyaaSource::with_base_url(server.uri());
    // TMDB-style: English franchise title only — no season in the name.
    let m = media(
        MediaKind::Anime,
        IdSet::default().with_imdb("tt13293588"),
        "Mushoku Tensei: Jobless Reincarnation",
    );
    let q = SourceQuery {
        media: m,
        season: Some(3),
        episode: Some(1),
        absolute_episode: None,
        kitsu: None,
        imdb_season: None,
        include_uncached: true,
    };

    let candidates = src.find(&q).await.unwrap();

    assert!(
        !candidates.is_empty(),
        "expected S3 releases, got empty list"
    );
    assert!(
        candidates.iter().all(|c| {
            let t = c.title.to_ascii_lowercase();
            t.contains("s3") || t.contains("season 3")
        }),
        "only S3-tagged releases should remain, got: {:?}",
        candidates.iter().map(|c| &c.title).collect::<Vec<_>>()
    );
    assert!(
        !candidates.iter().any(|c| {
            let t = c.title.to_ascii_lowercase();
            t.contains("season 1") || t.contains(" s1") || t.contains("ii -")
        }),
        "S1/S2 packs must not leak into an S3 result"
    );
}

/// When ordinal > 1 and nothing matches the season filter, do **not** fall back
/// to the unfiltered S1-dominated set (that reintroduced wrong-season packs).
#[tokio::test]
async fn nyaa_later_season_does_not_fall_back_to_unfiltered_s1() {
    let server = MockServer::start().await;

    let rss_s1_only = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[EMBER] Mushoku Tensei - Jobless Reincarnation (Season 1) [BD 1080p]</title>
      <nyaa:seeders>600</nyaa:seeders><nyaa:size>17.0 GiB</nyaa:size>
      <nyaa:infoHash>1111111111111111111111111111111111111111</nyaa:infoHash>
    </item>
    <item>
      <title>[SubsPlease] Mushoku Tensei - 01 (1080p).mkv</title>
      <nyaa:seeders>500</nyaa:seeders><nyaa:size>1.4 GiB</nyaa:size>
      <nyaa:infoHash>2222222222222222222222222222222222222222</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .and(query_param("page", "rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss_s1_only))
        .mount(&server)
        .await;

    let src = NyaaSource::with_base_url(server.uri());
    let m = media(
        MediaKind::Anime,
        IdSet::default().with_imdb("tt13293588"),
        "Mushoku Tensei: Jobless Reincarnation",
    );
    let q = SourceQuery {
        media: m,
        season: Some(3),
        episode: Some(1),
        absolute_episode: None,
        kitsu: None,
        imdb_season: None,
        include_uncached: true,
    };

    let candidates = src.find(&q).await.unwrap();
    assert!(
        candidates.is_empty(),
        "S3 with only S1 hits must return empty, not unfiltered S1; got: {:?}",
        candidates.iter().map(|c| &c.title).collect::<Vec<_>>()
    );
}

/// S1 episode play must drop later-season roman markers and multi-season packs.
#[tokio::test]
async fn nyaa_s1_drops_sequel_roman_and_multi_season_range() {
    let server = MockServer::start().await;

    let rss = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[SubsPlease] Mushoku Tensei - 01 (1080p) [S1].mkv</title>
      <nyaa:seeders>500</nyaa:seeders><nyaa:size>1.4 GiB</nyaa:size>
      <nyaa:infoHash>1111111111111111111111111111111111111111</nyaa:infoHash>
    </item>
    <item>
      <title>[Anime Time] Mushoku Tensei - Jobless Reincarnation II - 01 [1080p]</title>
      <nyaa:seeders>200</nyaa:seeders><nyaa:size>1.4 GiB</nyaa:size>
      <nyaa:infoHash>2222222222222222222222222222222222222222</nyaa:infoHash>
    </item>
    <item>
      <title>[Pack] Mushoku Tensei S01-S03 Complete [1080p]</title>
      <nyaa:seeders>100</nyaa:seeders><nyaa:size>40 GiB</nyaa:size>
      <nyaa:infoHash>3333333333333333333333333333333333333333</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    Mock::given(method("GET"))
        .and(query_param("page", "rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss))
        .mount(&server)
        .await;

    let src = NyaaSource::with_base_url(server.uri());
    let mut m = media(
        MediaKind::Anime,
        IdSet::default().with_imdb("tt13293588"),
        "Mushoku Tensei: Jobless Reincarnation",
    );
    m.original_title = Some("Mushoku Tensei".into());
    let q = SourceQuery {
        media: m,
        season: Some(1),
        episode: Some(1),
        absolute_episode: None,
        kitsu: None,
        imdb_season: None,
        include_uncached: true,
    };

    let candidates = src.find(&q).await.unwrap();
    assert_eq!(candidates.len(), 1, "only bare S1 premiere: {candidates:?}");
    assert!(candidates[0].title.contains("SubsPlease"));
    assert!(!candidates.iter().any(|c| c.title.contains(" II ")));
    assert!(!candidates.iter().any(|c| c.title.contains("S01-S03")));
}

/// HTTP 429 is retried with backoff until a success body is returned.
#[tokio::test]
async fn nyaa_retries_on_http_429() {
    let server = MockServer::start().await;

    let rss = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <item>
      <title>[SubsPlease] Frieren - 01 (1080p) [OK].mkv</title>
      <nyaa:seeders>10</nyaa:seeders><nyaa:size>1.0 GiB</nyaa:size>
      <nyaa:infoHash>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    // First response: rate limited. Second: success.
    Mock::given(method("GET"))
        .and(query_param("page", "rss"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("page", "rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss))
        .mount(&server)
        .await;

    let src = NyaaSource::with_base_url(server.uri());
    let q = SourceQuery {
        media: media(MediaKind::Anime, IdSet::default().with_mal(1), "Frieren"),
        season: Some(1),
        episode: Some(1),
        absolute_episode: None,
        kitsu: None,
        imdb_season: None,
        include_uncached: true,
    };

    let candidates = src.find(&q).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].title.contains("Frieren"));
}

#[tokio::test]
async fn nyaa_does_not_support_movies() {
    let src = NyaaSource::new();
    assert!(!src.supports(MediaKind::Movie));
    assert!(src.supports(MediaKind::Anime));
}
