//! Cross-platform async transport for the Discord IPC channel.
//!
//! Discord's local IPC endpoint is a **Unix domain socket**
//! (`$XDG_RUNTIME_DIR/discord-ipc-N`) on Unix and a **named pipe**
//! (`\\.\pipe\discord-ipc-N`) on Windows. This module hides that difference
//! behind a single [`IpcStream`] type + [`connect`] fn so the rich-presence
//! framing in [`crate::discord`] stays platform-agnostic: both backends
//! implement `AsyncRead + AsyncWrite + Unpin`.
//!
//! (Deliberately a per-crate twin of `open-media-player`'s `ipc` module. Adapter crates
//! don't depend on each other, and the seam is too small to justify a shared
//! crate.)

use std::io;
use std::path::Path;

#[cfg(unix)]
pub use tokio::net::UnixStream as IpcStream;

/// Connect to the Discord IPC endpoint at `path` (a Unix socket).
#[cfg(unix)]
pub async fn connect(path: &Path) -> io::Result<IpcStream> {
    tokio::net::UnixStream::connect(path).await
}

#[cfg(windows)]
pub use tokio::net::windows::named_pipe::NamedPipeClient as IpcStream;

/// Connect to the Discord IPC endpoint at `path` (a `\\.\pipe\discord-ipc-N`
/// named pipe).
#[cfg(windows)]
pub async fn connect(path: &Path) -> io::Result<IpcStream> {
    tokio::net::windows::named_pipe::ClientOptions::new().open(path.as_os_str())
}
