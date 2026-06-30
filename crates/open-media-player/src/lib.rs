//! # open-media-player
//!
//! [`Player`] adapters and the mpv IPC control plane.
//!
//! - [`MpvPlayer`] — launches mpv with `--input-ipc-server` and returns a session
//!   whose [`PlaybackControl`] (via [`mpv::MpvControl`]) talks line-delimited JSON
//!   over the socket. This single channel powers resume (seek), progress
//!   (time-pos), presence (pause), and AniSkip (seek + chapters).
//! - [`VlcPlayer`] — launch-only; its session returns `None` from
//!   [`PlaySession::control`], so resume/auto-skip are unavailable for vlc.
//!
//! [`Player`]: open_media_core::ports::Player
//! [`PlaybackControl`]: open_media_core::ports::PlaybackControl
//! [`PlaySession::control`]: open_media_core::ports::PlaySession::control

mod ipc;
pub mod mpv;
pub mod vlc;

pub use mpv::{MpvControl, MpvPlayer, MpvSession};
pub use vlc::VlcPlayer;
