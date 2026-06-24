//! # om-player
//!
//! [`Player`] adapters and the mpv IPC control plane.
//!
//! - [`MpvPlayer`] — launches mpv with `--input-ipc-server=<socket>` and returns
//!   a session whose [`PlaybackControl`] talks line-delimited JSON over the
//!   socket (`{"command":[...]}\n` → `{error,data}`). This single channel powers
//!   resume (seek), progress (time-pos), presence (pause), and AniSkip
//!   (seek + chapters) — ported from curd's IPC client.
//! - [`VlcPlayer`] — launch-only (`--play-and-exit`); no IPC, so resume/auto-skip
//!   are unavailable. Implements [`Player`] but its session returns `None` from
//!   [`PlaySession::control`].
//!
//! Scaffold stub: [`Player`] is implemented; `play` returns
//! [`CoreError::NotImplemented`] until Phase 5 (see `docs/ROADMAP.md`). `which`
//! gives a real [`Player::is_available`] today.
//!
//! [`Player`]: om_core::ports::Player
//! [`PlaybackControl`]: om_core::ports::PlaybackControl
//! [`PlaySession::control`]: om_core::ports::PlaySession::control
//! [`Player::is_available`]: om_core::ports::Player::is_available
//! [`CoreError::NotImplemented`]: om_core::error::CoreError::NotImplemented

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::ports::{PlayOptions, PlaySession, Player};
use om_core::stream::Playback;

/// mpv with JSON IPC control.
pub struct MpvPlayer {
    command: String,
    args: Vec<String>,
}

impl MpvPlayer {
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
        }
    }
}

#[async_trait]
impl Player for MpvPlayer {
    fn name(&self) -> &str {
        "mpv"
    }

    fn is_available(&self) -> bool {
        which::which(&self.command).is_ok()
    }

    async fn play(
        &self,
        _playback: &Playback,
        _opts: &PlayOptions,
    ) -> CoreResult<Box<dyn PlaySession>> {
        let _ = &self.args;
        Err(CoreError::NotImplemented("mpv.play"))
    }
}

/// vlc, launch-only (no IPC control).
pub struct VlcPlayer {
    command: String,
    args: Vec<String>,
}

impl VlcPlayer {
    pub fn new(args: Vec<String>) -> Self {
        Self {
            command: "vlc".to_string(),
            args,
        }
    }
}

#[async_trait]
impl Player for VlcPlayer {
    fn name(&self) -> &str {
        "vlc"
    }

    fn is_available(&self) -> bool {
        which::which(&self.command).is_ok()
    }

    async fn play(
        &self,
        _playback: &Playback,
        _opts: &PlayOptions,
    ) -> CoreResult<Box<dyn PlaySession>> {
        let _ = &self.args;
        Err(CoreError::NotImplemented("vlc.play"))
    }
}
