//! Real-mpv integration test (gated behind `--ignored`).
//!
//! Launches an actual mpv on a synthetic `lavfi` source — headless
//! (`--vo=null --ao=null --no-config`) so it needs no display, audio device, or
//! the user's mpv.conf — and drives the real IPC: query duration, seek, then let
//! it run to completion and confirm the session ends.
//!
//! Run with: `cargo test -p om-player --test real_mpv -- --ignored`

use std::time::Duration;

use om_core::ports::{PlayOptions, Player};
use om_core::stream::{Playback, PlaybackOrigin};
use om_player::MpvPlayer;

#[tokio::test]
#[ignore = "requires a real mpv binary"]
async fn real_mpv_launch_ipc_and_exit() {
    let player = MpvPlayer::new(
        "mpv",
        vec!["--vo=null".into(), "--ao=null".into(), "--no-config".into()],
    );
    if !player.is_available() {
        eprintln!("mpv not on PATH — skipping");
        return;
    }

    // 6 seconds of synthetic video, no window/audio needed.
    let playback = Playback {
        url: "av://lavfi:testsrc=duration=6:size=320x240:rate=5".into(),
        origin: PlaybackOrigin::LocalP2p,
        file_name: "testsrc".into(),
    };

    let mut session = player
        .play(&playback, &PlayOptions::default())
        .await
        .expect("mpv launches");
    let control = session.control().expect("mpv exposes IPC control");

    // Let mpv load the source.
    tokio::time::sleep(Duration::from_millis(1200)).await;

    // Real IPC round-trips against a live mpv: a readable duration proves the
    // get_property path works end-to-end (exact value depends on the lavfi
    // source and mpv's estimate, so we only require it to be present).
    let duration = control.duration().await.expect("duration query");
    assert!(
        matches!(duration, Some(d) if d >= 1),
        "expected a positive duration, got {duration:?}"
    );

    control.seek_absolute(1).await.expect("seek");
    let pos = control.position().await.expect("position query");
    assert!(pos.is_some(), "position should be readable after seek");

    // The source ends on its own; the session must resolve.
    tokio::time::timeout(Duration::from_secs(15), session.wait())
        .await
        .expect("mpv exits within 15s")
        .expect("clean wait");
}
