//! End-to-end tests for the Fribb anime-id bridge: fetch → tolerant parse →
//! cache → lookup, against a mock server serving the **current** upstream
//! schema (array `imdb_id`, object `themoviedb_id`), plus the failure/retry
//! path. A gated live test verifies the real dataset still parses.

use open_media_core::model::IdSet;
use open_media_core::ports::IdBridge;
use open_media_metadata::FribbIdBridge;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The live-schema shape recorded from upstream (2026-07): `imdb_id` arrays and
/// per-season entries sharing one series id.
fn live_schema_body() -> serde_json::Value {
    json!([
        {
            "type": "TV", "anilist_id": 130298, "mal_id": 48316,
            "imdb_id": ["tt14115938"], "kitsu_id": 44107,
            "themoviedb_id": { "tv": 119495 }, "season": { "tvdb": 1, "tmdb": 1 }
        },
        {
            "type": "TV", "anilist_id": 161964, "mal_id": 54595,
            "imdb_id": ["tt14115938"], "kitsu_id": 47099,
            "themoviedb_id": { "tv": 119495 }, "season": { "tvdb": 2, "tmdb": 2 }
        },
        { "type": "TV", "anilist_id": 154587, "mal_id": 52991 }
    ])
}

#[tokio::test]
async fn fetches_parses_array_schema_and_caches() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(live_schema_body()))
        .expect(1) // second lookup must come from memory, not the network
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("anime-id-map.json");
    let bridge = FribbIdBridge::with_url_and_cache(server.uri(), cache.clone());

    let imdb = bridge
        .imdb_for(&IdSet::default().with_anilist(130298))
        .await
        .unwrap();
    assert_eq!(imdb.as_deref(), Some("tt14115938"));

    // The raw dataset is persisted only after a successful parse — the exact
    // behavior that silently broke when the schema changed under the old code.
    assert!(cache.exists(), "cache must be written on successful parse");

    // Second lookup (a different key) is served from the memoized map.
    let none = bridge
        .imdb_for(&IdSet::default().with_anilist(154587))
        .await
        .unwrap();
    assert_eq!(none, None);
}

#[tokio::test]
async fn cold_cache_reuse_needs_no_network() {
    // Pre-seed the cache file, point the bridge at a server that would fail —
    // a fresh cache must win without any request.
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("anime-id-map.json");
    std::fs::write(&cache, live_schema_body().to_string()).unwrap();

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let bridge = FribbIdBridge::with_url_and_cache(server.uri(), cache);
    let imdb = bridge
        .imdb_for(&IdSet::default().with_mal(54595))
        .await
        .unwrap();
    assert_eq!(imdb.as_deref(), Some("tt14115938"));
}

#[tokio::test]
async fn failed_load_degrades_then_retries_after_cooldown() {
    let server = MockServer::start().await;
    // First attempt: upstream down.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Recovery: healthy payload.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(live_schema_body()))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let bridge =
        FribbIdBridge::with_url_and_cache(server.uri(), dir.path().join("anime-id-map.json"));
    let ids = IdSet::default().with_anilist(130298);

    // Failure degrades to no-enrichment, never an error.
    assert_eq!(bridge.imdb_for(&ids).await.unwrap(), None);
    // Within the cooldown the failure is not retried.
    assert_eq!(bridge.imdb_for(&ids).await.unwrap(), None);

    // After the cooldown the next lookup retries and succeeds — a startup
    // hiccup must not disable the bridge for the whole session.
    bridge.expire_retry_cooldown().await;
    let imdb = bridge.imdb_for(&ids).await.unwrap();
    assert_eq!(imdb.as_deref(), Some("tt14115938"));
}

/// Gated live test: the real upstream dataset must parse into a non-trivial
/// map. Run with `cargo test -p open-media-metadata --test bridge_e2e -- --ignored`.
#[tokio::test]
#[ignore = "network: fetches the real Fribb dataset"]
async fn live_fribb_dataset_parses_and_maps() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = FribbIdBridge::with_url_and_cache(
        "https://raw.githubusercontent.com/Fribb/anime-lists/master/anime-list-full.json",
        dir.path().join("anime-id-map.json"),
    );
    // The Eminence in Shadow (AniList 130298) → tt14115938.
    let imdb = bridge
        .imdb_for(&IdSet::default().with_anilist(130298))
        .await
        .unwrap();
    assert_eq!(imdb.as_deref(), Some("tt14115938"));
}
