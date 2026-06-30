//! # om-net
//!
//! Shared HTTP plumbing for the network adapters. Two concerns live here so that
//! `open-media-core` can stay strictly I/O-free (it must not name `reqwest`):
//!
//! - [`client`] / [`client_with`] — a [`reqwest::Client`] factory with sane
//!   connect + overall timeouts and a stable user-agent. Every adapter builds its
//!   client through this instead of `reqwest::Client::new()` (which has *no*
//!   timeout — a stalled DNS/TLS handshake would hang a request forever).
//! - [`retry`] — a bounded exponential-backoff retry wrapper for idempotent
//!   requests. It re-runs a closure only while the returned [`CoreError`] reports
//!   itself [retryable](open_media_core::error::CoreError::is_retryable) (network
//!   / timeout), so a 404 or auth failure fails fast.
//!
//! This is a leaf I/O helper crate: adapters depend on it, it depends only on
//! `reqwest` + `open-media-core` (for the error type the retry predicate keys off of).

use std::time::Duration;

use open_media_core::error::CoreResult;

/// User-agent sent on every request. A stable, identifiable UA is courteous to
/// the public APIs we hit (TMDB, AniList, nyaa, …) and helps them attribute /
/// rate-limit traffic to the app rather than treating us as an anonymous bot.
pub const USER_AGENT: &str = concat!("open-media/", env!("CARGO_PKG_VERSION"));

/// Default time to establish a TCP+TLS connection before giving up.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default ceiling on a whole request (connect + send + response). Bounds the
/// worst case for a hung peer that accepts the connection but never replies.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default number of attempts [`retry`] makes (the first try plus retries).
pub const DEFAULT_RETRIES: u32 = 3;

/// Base unit for the backoff schedule: sleep `BACKOFF_BASE * 2^attempt` between
/// tries (attempt 0 → 200ms, 1 → 400ms, 2 → 800ms, …).
const BACKOFF_BASE: Duration = Duration::from_millis(200);

/// Build a [`reqwest::Client`] with the default connect/request timeouts and the
/// shared [`USER_AGENT`].
///
/// Use this in place of `reqwest::Client::new()` everywhere. The builder cannot
/// fail with these inputs, but if the platform TLS backend ever refuses to
/// initialize we fall back to a bare client rather than panicking — a request
/// with no timeout is still far better than aborting startup.
pub fn client() -> reqwest::Client {
    client_with(DEFAULT_CONNECT_TIMEOUT, DEFAULT_REQUEST_TIMEOUT)
}

/// Build a [`reqwest::Client`] with explicit connect and overall-request
/// timeouts (and the shared [`USER_AGENT`]). For the rare caller that needs a
/// different ceiling than the defaults.
pub fn client_with(connect_timeout: Duration, request_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()
        .unwrap_or_else(|e| {
            // Practically unreachable with the inputs above; degrade rather than
            // abort so a TLS-init quirk can't take the whole app down.
            tracing::warn!(error = %e, "HTTP client builder failed; using untimed default");
            reqwest::Client::new()
        })
}

/// Run `op` up to [`DEFAULT_RETRIES`] times, retrying only on a
/// [retryable](CoreError::is_retryable) error, with exponential backoff between
/// attempts. See [`retry_with`] for the configurable form.
///
/// Intended to wrap a single idempotent request (a GET fetch). Wrap the *outer*
/// request call, not deep internals, so a retry re-issues the whole request.
pub async fn retry<T, F, Fut>(op: F) -> CoreResult<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = CoreResult<T>>,
{
    retry_with(DEFAULT_RETRIES, op).await
}

/// Like [`retry`] but with an explicit attempt budget. `attempts` is the total
/// number of tries (clamped to at least 1); the closure is awaited fresh each
/// time. A non-retryable error short-circuits immediately and is returned as-is;
/// the last error is returned once the budget is exhausted.
pub async fn retry_with<T, F, Fut>(attempts: u32, mut op: F) -> CoreResult<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = CoreResult<T>>,
{
    let attempts = attempts.max(1);
    let mut attempt = 0;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                let last = attempt + 1 >= attempts;
                if last || !e.is_retryable() {
                    return Err(e);
                }
                let backoff = BACKOFF_BASE * 2u32.pow(attempt);
                tracing::debug!(
                    attempt = attempt + 1,
                    backoff_ms = backoff.as_millis() as u64,
                    error = %e,
                    "retrying transient request after backoff"
                );
                tokio::time::sleep(backoff).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_media_core::error::CoreError;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn client_builds_with_user_agent() {
        // Smoke: the factory yields a usable client without panicking.
        let _ = client();
        let _ = client_with(Duration::from_secs(1), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn retries_then_succeeds_on_transient_errors() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls2 = calls.clone();
        // Fail twice with a retryable error, then succeed on the third try.
        let result = retry(move || {
            let calls = calls2.clone();
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(CoreError::Network("boom".into()))
                } else {
                    Ok(n)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 2);
        // Exactly 3 attempts: the two failures plus the success.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn exhausts_budget_and_returns_last_error() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls2 = calls.clone();
        let result: CoreResult<()> = retry(move || {
            let calls = calls2.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(CoreError::Timeout("always".into()))
            }
        })
        .await;

        assert!(matches!(result, Err(CoreError::Timeout(_))));
        // Default budget is exactly DEFAULT_RETRIES attempts, no more.
        assert_eq!(calls.load(Ordering::SeqCst), DEFAULT_RETRIES);
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_retry_non_retryable_errors() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls2 = calls.clone();
        let result: CoreResult<()> = retry(move || {
            let calls = calls2.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                // NotFound is not transient — must fail fast on the first try.
                Err(CoreError::NotFound("nope".into()))
            }
        })
        .await;

        assert!(matches!(result, Err(CoreError::NotFound(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
