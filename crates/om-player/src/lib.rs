//! # om-player
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
//! [`Player`]: om_core::ports::Player
//! [`PlaybackControl`]: om_core::ports::PlaybackControl
//! [`PlaySession::control`]: om_core::ports::PlaySession::control

pub mod mpv;
pub mod vlc;

pub use mpv::{MpvControl, MpvPlayer, MpvSession};
pub use vlc::VlcPlayer;
