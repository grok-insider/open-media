//! Core error type.
//!
//! Ports return [`CoreError`] so the application layer can reason about failure
//! categories without knowing which adapter produced them (DIP: callers depend
//! on this enum, not on `reqwest::Error`, `rusqlite::Error`, etc.). Adapters map
//! their concrete errors into one of these variants at the boundary.

use thiserror::Error;

/// The unified error returned by every port in [`crate::ports`].
#[derive(Debug, Error)]
pub enum CoreError {
    /// A required credential (API key / token) is missing or empty.
    #[error("missing credential: {0}")]
    MissingCredential(String),

    /// Authentication or authorization with a remote service failed.
    #[error("authentication failed: {0}")]
    Auth(String),

    /// The remote service rejected the request or returned a non-success status.
    #[error("remote error from {service}: {message}")]
    Remote { service: String, message: String },

    /// The network transport failed (DNS, TLS, connection reset, ...).
    #[error("network error: {0}")]
    Network(String),

    /// A response could not be parsed into the expected shape.
    #[error("failed to parse {what}: {message}")]
    Parse { what: String, message: String },

    /// The requested item (media, episode, file, torrent) was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// No playable source/stream could be produced for the request.
    #[error("no playable source: {0}")]
    NoSource(String),

    /// An operation exceeded its deadline.
    #[error("timed out: {0}")]
    Timeout(String),

    /// Local persistence (history DB, config file) failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// The external media player could not be launched or controlled.
    #[error("player error: {0}")]
    Player(String),

    /// Configuration was invalid.
    #[error("invalid configuration: {0}")]
    Config(String),

    /// A feature/operation is recognized but not yet implemented.
    ///
    /// Used by scaffold stubs so the binary links and runs end-to-end while
    /// individual adapters are filled in per the roadmap.
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    /// An adapter-specific error that does not fit the categories above.
    #[error("{0}")]
    Other(String),
}

/// Convenience alias used throughout the core and application layers.
pub type CoreResult<T> = Result<T, CoreError>;
