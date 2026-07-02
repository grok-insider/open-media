//! End-to-end integration tests for the TorBox resolve flow.
//!
//! Drives the full state machine (`createtorrent` → poll `mylist` → `requestdl`)
//! against a mock server with a tiny poll interval, plus the envelope error
//! paths and the (real, unlike Real-Debrid) bulk cache check.

use std::time::Duration;

use open_media_core::error::CoreError;
use open_media_core::ports::DebridProvider;
use open_media_core::stream::{CacheState, PlaybackOrigin, Quality, ReleaseTags, SourceCandidate};
use open_media_debrid::Torbox;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn candidate_with_hash(hash: &str) -> SourceCandidate {
    SourceCandidate {
        provider: "torrentio".into(),
        title: "Interstellar 2014 1080p".into(),
        quality: Quality::P1080,
        size_bytes: 2_400_000_000,
        seeders: Some(50),
        info_hash: Some(hash.into()),
        magnet: None,
        direct_url: None,
        file_index: None,
        cache: CacheState::Uncached,
        tags: ReleaseTags::default(),
    }
}

#[tokio::test]
async fn resolves_torrent_end_to_end() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/torrents/createtorrent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "error": null,
            "detail": "Torrent created.",
            "data": { "torrent_id": 42, "name": "Interstellar.2014.1080p", "hash": "abc" }
        })))
        .mount(&server)
        .await;

    // First poll: still downloading (single-use).
    Mock::given(method("GET"))
        .and(path("/torrents/mylist"))
        .and(query_param("id", "42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "error": null,
            "detail": "ok",
            "data": {
                "id": 42,
                "download_state": "downloading",
                "download_finished": false,
                "download_present": false,
                "files": []
            }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // Subsequent polls: ready, with files.
    Mock::given(method("GET"))
        .and(path("/torrents/mylist"))
        .and(query_param("id", "42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "error": null,
            "detail": "ok",
            "data": {
                "id": 42,
                "download_state": "uploading",
                "download_finished": true,
                "download_present": true,
                "files": [
                    { "id": 7, "name": "Interstellar.2014.1080p/Interstellar.2014.1080p.mkv",
                      "short_name": "Interstellar.2014.1080p.mkv", "size": 2400000000_u64 },
                    { "id": 8, "name": "Interstellar.2014.1080p/sample.nfo",
                      "short_name": "sample.nfo", "size": 100 }
                ]
            }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/torrents/requestdl"))
        .and(query_param("torrent_id", "42"))
        .and(query_param("file_id", "7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "error": null,
            "detail": "ok",
            "data": "https://store-01.torbox.app/dl/XYZ/Interstellar.mkv"
        })))
        .mount(&server)
        .await;

    let tb = Torbox::with_base_url("test_token", server.uri())
        .with_poll_interval(Duration::from_millis(10));

    let candidate = candidate_with_hash("abcdef0123456789abcdef0123456789abcdef01");
    let playback = tb.resolve_playback(&candidate).await.unwrap();

    assert_eq!(playback.origin, PlaybackOrigin::Debrid);
    assert_eq!(
        playback.url,
        "https://store-01.torbox.app/dl/XYZ/Interstellar.mkv"
    );
    assert_eq!(playback.file_name, "Interstellar.2014.1080p.mkv");
}

#[tokio::test]
async fn season_pack_file_index_targets_requested_episode() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/torrents/createtorrent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "error": null, "detail": "ok",
            "data": { "torrent_id": 5, "name": "Show S01", "hash": "def" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/torrents/mylist"))
        .and(query_param("id", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "error": null, "detail": "ok",
            "data": {
                "id": 5,
                "download_state": "cached",
                "download_finished": true,
                "download_present": true,
                "files": [
                    { "id": 1, "name": "Show/S01E01.mkv", "short_name": "S01E01.mkv", "size": 100 },
                    { "id": 2, "name": "Show/S01E02.mkv", "short_name": "S01E02.mkv", "size": 100 },
                    { "id": 3, "name": "Show/S01E03.mkv", "short_name": "S01E03.mkv", "size": 100 }
                ]
            }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/torrents/requestdl"))
        .and(query_param("torrent_id", "5"))
        .and(query_param("file_id", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "error": null, "detail": "ok",
            "data": "https://store-01.torbox.app/dl/E02"
        })))
        .mount(&server)
        .await;

    let tb = Torbox::with_base_url("test_token", server.uri())
        .with_poll_interval(Duration::from_millis(10));

    let mut candidate = candidate_with_hash("1111111111111111111111111111111111111111");
    candidate.file_index = Some(1); // E02 at full-list position 1

    let playback = tb.resolve_playback(&candidate).await.unwrap();
    assert_eq!(playback.url, "https://store-01.torbox.app/dl/E02");
    assert_eq!(playback.file_name, "S01E02.mkv");
}

#[tokio::test]
async fn check_cached_maps_hits_and_misses() {
    let server = MockServer::start().await;

    // TorBox returns only the cached hashes (lowercase) in the object map.
    Mock::given(method("GET"))
        .and(path("/torrents/checkcached"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "error": null,
            "detail": "ok",
            "data": {
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa":
                    { "name": "Cached Movie", "size": 1000, "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
            }
        })))
        .mount(&server)
        .await;

    let tb = Torbox::with_base_url("test_token", server.uri());
    let cached = tb
        .check_cached(&[
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        ])
        .await
        .unwrap();

    assert_eq!(
        cached.get("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        Some(&true),
        "uppercase input hash matches torbox's lowercase key"
    );
    assert_eq!(
        cached.get("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        Some(&false)
    );
}

#[tokio::test]
async fn account_summary_reports_plan() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "error": null,
            "detail": "ok",
            "data": {
                "email": "watcher@example.com",
                "plan": 2,
                "premium_expires_at": "2026-09-01T00:00:00Z"
            }
        })))
        .mount(&server)
        .await;

    let tb = Torbox::with_base_url("test_token", server.uri());
    let summary = tb.account_summary().await.unwrap();
    assert!(summary.contains("watcher@example.com"));
    assert!(summary.contains("pro"));
}

#[tokio::test]
async fn resolve_surfaces_failed_torrents_and_deletes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/torrents/createtorrent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "error": null, "detail": "ok",
            "data": { "torrent_id": 9, "name": "Dead", "hash": "dead" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/torrents/mylist"))
        .and(query_param("id", "9"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "error": null, "detail": "ok",
            "data": {
                "id": 9,
                "download_state": "error",
                "download_finished": false,
                "download_present": false,
                "files": []
            }
        })))
        .mount(&server)
        .await;
    let delete = Mock::given(method("POST"))
        .and(path("/torrents/controltorrent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "error": null, "detail": "deleted", "data": {}
        })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let tb = Torbox::with_base_url("test_token", server.uri())
        .with_poll_interval(Duration::from_millis(10));
    let err = tb
        .resolve_playback(&candidate_with_hash(
            "0000000000000000000000000000000000000000",
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("error"));
    drop(delete); // asserts the failed torrent was cleaned up
}

#[tokio::test]
async fn auth_failure_maps_to_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "success": false,
            "error": "AUTH_ERROR",
            "detail": "Invalid or expired API token.",
            "data": null
        })))
        .mount(&server)
        .await;

    let tb = Torbox::with_base_url("bad", server.uri());
    let err = tb.account_summary().await.unwrap_err();
    assert!(matches!(err, CoreError::Auth(_)));
    assert!(err.to_string().contains("Invalid or expired"));
}

#[tokio::test]
async fn envelope_failure_on_http_200_maps_to_remote_error() {
    let server = MockServer::start().await;
    // TorBox can signal failure inside a 200 response's envelope.
    Mock::given(method("POST"))
        .and(path("/torrents/createtorrent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": false,
            "error": "DOWNLOAD_LIMIT_REACHED",
            "detail": "You have reached your active download limit.",
            "data": null
        })))
        .mount(&server)
        .await;

    let tb = Torbox::with_base_url("test_token", server.uri());
    let err = tb.add_magnet("magnet:?xt=urn:btih:abc").await.unwrap_err();
    assert!(matches!(err, CoreError::Remote { .. }));
    assert!(err.to_string().contains("active download limit"));
}
