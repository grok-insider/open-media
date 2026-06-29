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

impl CoreError {
    /// Whether retrying the failed operation could plausibly succeed.
    ///
    /// Only the *transport-level transient* failures qualify: a dropped
    /// connection, DNS hiccup, or a deadline overrun ([`Self::Network`] /
    /// [`Self::Timeout`]) is worth re-issuing an idempotent request for. Every
    /// other variant is treated as terminal — a `Remote` non-success status, an
    /// auth failure, a parse error, or a missing item won't change on a blind
    /// retry, so callers fail fast instead of hammering the service.
    ///
    /// Note: `Remote` is intentionally *not* retryable here. It carries no HTTP
    /// status (it's a coarse "the service rejected us" signal), so we can't tell
    /// a retryable 503 from a terminal 400/404; the conservative choice is to not
    /// retry. Transient transport failures already surface as `Network`/`Timeout`
    /// (see the adapters' `map_net`), which *are* retried.
    pub fn is_retryable(&self) -> bool {
        matches!(self, CoreError::Network(_) | CoreError::Timeout(_))
    }
}

/// Convenience alias used throughout the core and application layers.
pub type CoreResult<T> = Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::CoreError;

    #[test]
    fn network_and_timeout_are_retryable() {
        assert!(CoreError::Network("reset".into()).is_retryable());
        assert!(CoreError::Timeout("deadline".into()).is_retryable());
    }

    #[test]
    fn other_variants_are_not_retryable() {
        // A representative spread of terminal failures — none should be retried.
        let terminal = [
            CoreError::MissingCredential("key".into()),
            CoreError::Auth("bad token".into()),
            CoreError::Remote {
                service: "tmdb".into(),
                message: "HTTP 404".into(),
            },
            CoreError::Parse {
                what: "body".into(),
                message: "eof".into(),
            },
            CoreError::NotFound("media".into()),
            CoreError::NoSource("none".into()),
            CoreError::Storage("disk".into()),
            CoreError::Player("mpv".into()),
            CoreError::Config("bad".into()),
            CoreError::NotImplemented("stub"),
            CoreError::Other("misc".into()),
        ];
        for e in terminal {
            assert!(!e.is_retryable(), "{e:?} must not be retryable");
        }
    }
}
