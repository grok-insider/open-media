//! Tests for the librqbit-backed P2P engine.
//!
//! `p2p_http_server_starts_and_serves` is hermetic: it boots the engine's HTTP
//! stream server on a free localhost port and confirms librqbit's API responds —
//! no peers/torrents needed, so it validates our server wiring in CI.
//!
//! `live_stream_magnet` is gated behind `--ignored` + `OM_P2P_MAGNET` because it
//! needs real peers/DHT (often blocked in sandboxes).

use std::time::Duration;

use om_stream::P2pEngine;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn p2p_http_server_starts_and_serves() {
    let port = free_port();
    let engine = P2pEngine::new(port, true);
    engine.start().await.expect("engine starts");

    // Give the spawned axum server a moment to bind.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/torrents"))
        .await
        .expect("librqbit API reachable");
    assert!(resp.status().is_success(), "status {}", resp.status());
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("torrents"),
        "expected a torrents listing, got: {body}"
    );
}

#[tokio::test]
#[ignore = "live: needs peers/DHT; set OM_P2P_MAGNET to a well-seeded video magnet"]
async fn live_stream_magnet() {
    let Ok(magnet) = std::env::var("OM_P2P_MAGNET") else {
        eprintln!("OM_P2P_MAGNET unset — skipping");
        return;
    };
    let port = free_port();
    let engine = P2pEngine::new(port, true);

    let playback = engine.stream_magnet(&magnet).await.expect("stream magnet");
    println!("streaming: {} ({})", playback.url, playback.file_name);
    assert!(playback
        .url
        .starts_with(&format!("http://127.0.0.1:{port}/torrents/")));

    // The stream endpoint should answer a ranged request with partial content.
    let client = reqwest::Client::new();
    let resp = client
        .get(&playback.url)
        .header("Range", "bytes=0-1023")
        .send()
        .await
        .expect("stream endpoint reachable");
    assert!(
        resp.status().is_success() || resp.status().as_u16() == 206,
        "unexpected status {}",
        resp.status()
    );

    engine.cleanup().await;
}
