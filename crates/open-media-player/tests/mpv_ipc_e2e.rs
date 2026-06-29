//! End-to-end test for the mpv JSON-IPC control plane.
//!
//! Stands up a *fake mpv* unix-socket server that speaks mpv's IPC protocol
//! (`{"command":[...]}\n` → `{error,data}`) and drives the real [`MpvControl`]
//! against it — covering the codec, event-line skipping, and the typed helpers
//! without needing a real mpv or a display.
//!
//! Unix-only: the fake server uses `UnixListener`. The control plane itself is
//! cross-platform (named pipes on Windows); that path is exercised on the
//! Windows CI runner against real mpv rather than a hand-rolled pipe server.
#![cfg(unix)]

use std::path::PathBuf;

use open_media_core::ports::{Chapter, PlaybackControl};
use open_media_player::MpvControl;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

async fn spawn_fake_mpv() -> (PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("mpv.sock");
    let listener = UnixListener::bind(&socket).unwrap();

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(handle_conn(stream));
        }
    });

    (socket, dir)
}

async fn handle_conn(stream: tokio::net::UnixStream) {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    // mpv emits an unsolicited event on connect; our client must skip it.
    let _ = write_half.write_all(b"{\"event\":\"file-loaded\"}\n").await;

    while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        let cmd = v["command"].as_array().cloned().unwrap_or_default();
        let name = cmd.first().and_then(Value::as_str).unwrap_or("");
        let reply = match name {
            "get_property" => match cmd.get(1).and_then(Value::as_str).unwrap_or("") {
                "time-pos" => json!({ "error": "success", "data": 123.7 }),
                "duration" => json!({ "error": "success", "data": 1440.0 }),
                "pause" => json!({ "error": "success", "data": false }),
                _ => json!({ "error": "property unavailable" }),
            },
            "seek" | "set_property" | "quit" => json!({ "error": "success" }),
            _ => json!({ "error": "success" }),
        };
        write_half
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
        line.clear();
    }
}

#[tokio::test]
async fn mpv_ipc_roundtrip() {
    let (socket, _guard) = spawn_fake_mpv().await;
    let control = MpvControl::new(socket);

    // Skips the unsolicited event line, parses the typed replies.
    assert_eq!(control.position().await.unwrap(), Some(123)); // truncated from 123.7
    assert_eq!(control.duration().await.unwrap(), Some(1440));
    assert_eq!(control.is_paused().await.unwrap(), Some(false));

    // Commands that only need to succeed.
    control.seek_absolute(90).await.unwrap();
    control
        .set_chapters(&[
            Chapter {
                title: "Opening".into(),
                time_secs: 0,
            },
            Chapter {
                title: "Main".into(),
                time_secs: 90,
            },
        ])
        .await
        .unwrap();
    control.quit().await.unwrap();
}

#[tokio::test]
async fn mpv_ipc_connect_failure_is_an_error() {
    let control = MpvControl::new(PathBuf::from("/nonexistent/om-mpv-not-there.sock"));
    assert!(control.position().await.is_err());
}
