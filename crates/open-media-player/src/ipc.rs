//! Cross-platform async transport for mpv's JSON IPC channel.
//!
//! mpv exposes `--input-ipc-server` as a **Unix domain socket** on Unix and a
//! **named pipe** (`\\.\pipe\<name>`) on Windows. This module hides that one
//! difference behind a single [`IpcStream`] type + [`connect`] fn, so the JSON
//! framing in [`crate::mpv`] stays platform-agnostic: both backends implement
//! `AsyncRead + AsyncWrite + Unpin`, which is all the control plane needs.
//!
//! (Deliberately duplicated, ~per-crate, in `om-track` — see that crate's
//! `ipc` module. Adapter crates don't depend on each other, so a shared
//! transport would have to live in a new crate; the seam is too small to
//! warrant one.)

use std::io;
use std::path::Path;

#[cfg(unix)]
pub use tokio::net::UnixStream as IpcStream;

/// Connect to the mpv IPC endpoint at `path` (a Unix socket).
#[cfg(unix)]
pub async fn connect(path: &Path) -> io::Result<IpcStream> {
    tokio::net::UnixStream::connect(path).await
}

#[cfg(windows)]
pub use tokio::net::windows::named_pipe::NamedPipeClient as IpcStream;

/// Connect to the mpv IPC endpoint at `path` (a `\\.\pipe\<name>` named pipe).
///
/// `ERROR_PIPE_BUSY` surfaces as an `Err` here; callers already retry connects
/// (startup probe + per-request retry), so a momentarily busy pipe is handled
/// by the layer above without special-casing it.
#[cfg(windows)]
pub async fn connect(path: &Path) -> io::Result<IpcStream> {
    tokio::net::windows::named_pipe::ClientOptions::new().open(path.as_os_str())
}
