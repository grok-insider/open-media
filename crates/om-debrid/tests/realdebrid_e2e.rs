//! End-to-end integration test for the Real-Debrid resolve flow.
//!
//! Drives the full state machine (add → poll `waiting_files_selection` → select →
//! poll `downloaded` → unrestrict) against a mock server, with a tiny poll
//! interval so it runs in milliseconds.

use std::time::Duration;

use om_core::ports::DebridProvider;
use om_core::stream::{CacheState, PlaybackOrigin, Quality, ReleaseTags, SourceCandidate};
use om_debrid::RealDebrid;
use serde_json::json;
use wiremock::matchers::{method, path};
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
async fn resolves_uncached_torrent_end_to_end() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/torrents/addMagnet"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "T1", "uri": "https://real-debrid.com/torrents/T1"
        })))
        .mount(&server)
        .await;

    // First info poll: waiting for file selection (mounted first, single-use).
    Mock::given(method("GET"))
        .and(path("/torrents/info/T1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T1",
            "filename": "Interstellar.2014.1080p.mkv",
            "status": "waiting_files_selection",
            "files": [
                { "id": 1, "path": "/Interstellar.2014.1080p.mkv", "bytes": 2400000000_u64, "selected": 0 },
                { "id": 2, "path": "/sample.nfo", "bytes": 100, "selected": 0 }
            ],
            "links": []
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // selectFiles → 204 No Content.
    Mock::given(method("POST"))
        .and(path("/torrents/selectFiles/T1"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    // Subsequent info polls: downloaded, with a restricted link.
    Mock::given(method("GET"))
        .and(path("/torrents/info/T1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T1",
            "filename": "Interstellar.2014.1080p.mkv",
            "status": "downloaded",
            "files": [
                { "id": 1, "path": "/Interstellar.2014.1080p.mkv", "bytes": 2400000000_u64, "selected": 1 }
            ],
            "links": ["https://real-debrid.com/d/RESTRICTED1"]
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/unrestrict/link"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "filename": "Interstellar.2014.1080p.mkv",
            "download": "https://sgp1.download.real-debrid.com/d/XYZ/Interstellar.mkv"
        })))
        .mount(&server)
        .await;

    let rd = RealDebrid::with_base_url("test_token", server.uri())
        .with_poll_interval(Duration::from_millis(10));

    let candidate = candidate_with_hash("abcdef0123456789abcdef0123456789abcdef01");
    let playback = rd.resolve_playback(&candidate).await.unwrap();

    assert_eq!(playback.origin, PlaybackOrigin::Debrid);
    assert_eq!(
        playback.url,
        "https://sgp1.download.real-debrid.com/d/XYZ/Interstellar.mkv"
    );
    assert_eq!(playback.file_name, "Interstellar.2014.1080p.mkv");
}

#[tokio::test]
async fn account_summary_reports_premium() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "username": "watcher",
            "type": "premium",
            "expiration": "2026-09-01T00:00:00.000Z"
        })))
        .mount(&server)
        .await;

    let rd = RealDebrid::with_base_url("test_token", server.uri());
    let summary = rd.account_summary().await.unwrap();
    assert!(summary.contains("watcher"));
    assert!(summary.contains("premium"));
}

#[tokio::test]
async fn resolve_surfaces_torrent_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/torrents/addMagnet"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "T2", "uri": "x" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/torrents/info/T2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T2", "status": "magnet_error", "files": [], "links": []
        })))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/torrents/delete/T2"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let rd = RealDebrid::with_base_url("test_token", server.uri())
        .with_poll_interval(Duration::from_millis(10));
    let err = rd
        .resolve_playback(&candidate_with_hash(
            "0000000000000000000000000000000000000000",
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("magnet_error"));
}

#[tokio::test]
async fn auth_failure_maps_to_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "bad_token", "error_code": 8
        })))
        .mount(&server)
        .await;

    let rd = RealDebrid::with_base_url("bad", server.uri());
    let err = rd.account_summary().await.unwrap_err();
    assert!(matches!(err, om_core::error::CoreError::Auth(_)));
}
