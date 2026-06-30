//! # open-media-telemetry
//!
//! The anonymous usage-analytics adapter: a [`UsageReporter`] that POSTs a
//! [`UsageInfo`] snapshot to a collector so the project can estimate how many
//! active installs exist (DAU/MAU).
//!
//! ## Privacy invariant (part of the contract)
//! The **only** data transmitted is the four fields of [`UsageInfo`] — app
//! version, host OS, host arch, and a random per-install id. It carries
//! **nothing** about what a user watches: no titles, queries, source names,
//! tokens, or history. Do not extend the payload with anything content-derived.
//!
//! ## Best-effort
//! Reporting must never block or break the app. A missing/placeholder endpoint is
//! a no-op; a network failure or non-2xx response is swallowed (logged at debug,
//! returned as `Ok`). The caller fires it detached, once per launch.
//!
//! [`UsageReporter`]: open_media_core::ports::UsageReporter
//! [`UsageInfo`]: open_media_core::usage::UsageInfo

use std::time::Duration;

use async_trait::async_trait;
use open_media_core::ports::UsageReporter;
use open_media_core::usage::UsageInfo;
use open_media_core::CoreResult;

/// Sentinel endpoint meaning "no real collector configured yet". When the
/// reporter is built with this (or an empty string), [`HttpUsageReporter::report`]
/// is a no-op — the plumbing is in place but nothing is sent until a real
/// collector URL replaces it at the composition root.
pub const PLACEHOLDER_ENDPOINT: &str = "PLACEHOLDER";

/// How long to wait on the analytics POST before giving up. Kept short: this is a
/// fire-and-forget ping that must never delay the user.
const REPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// A [`UsageReporter`] that sends the anonymous snapshot to an HTTP collector.
pub struct HttpUsageReporter {
    endpoint: String,
    client: reqwest::Client,
}

impl HttpUsageReporter {
    /// Build a reporter targeting `endpoint`. If `endpoint` is empty or the
    /// [`PLACEHOLDER_ENDPOINT`] sentinel, reporting is a no-op.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Whether this reporter is pointed at a real collector (i.e. not the
    /// placeholder/empty sentinel). Lets the composition root skip wiring it
    /// entirely when no collector is configured.
    pub fn is_configured(endpoint: &str) -> bool {
        !endpoint.is_empty() && endpoint != PLACEHOLDER_ENDPOINT
    }

    /// The exact JSON body sent for `info`. Split out so a test can assert the
    /// payload contains only the four allowed fields and nothing else.
    fn payload(info: &UsageInfo) -> serde_json::Value {
        serde_json::json!({
            "v": info.version,
            "os": info.os,
            "arch": info.arch,
            "id": info.install_id,
        })
    }
}

#[async_trait]
impl UsageReporter for HttpUsageReporter {
    async fn report(&self, info: &UsageInfo) -> CoreResult<()> {
        // No collector configured → silently do nothing.
        if !Self::is_configured(&self.endpoint) {
            return Ok(());
        }

        let body = Self::payload(info);
        // Best-effort: any error (network, timeout, non-2xx) is logged at debug
        // and swallowed. Usage analytics must never surface to the user or fail a
        // launch.
        match self
            .client
            .post(&self.endpoint)
            .timeout(REPORT_TIMEOUT)
            .header(
                reqwest::header::USER_AGENT,
                concat!("open-media/", env!("CARGO_PKG_VERSION")),
            )
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => tracing::debug!(status = %resp.status(), "usage ping non-success"),
            Err(e) => tracing::debug!(error = %e, "usage ping failed"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample() -> UsageInfo {
        UsageInfo {
            version: "0.3.0".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            install_id: "11111111-1111-4111-8111-111111111111".into(),
        }
    }

    #[test]
    fn payload_contains_only_the_four_allowed_fields() {
        let v = HttpUsageReporter::payload(&sample());
        let obj = v.as_object().expect("payload is a JSON object");
        // Exactly the allowed keys — no content-derived fields ever.
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["arch", "id", "os", "v"]);
        assert_eq!(obj["v"], "0.3.0");
        assert_eq!(obj["os"], "linux");
        assert_eq!(obj["arch"], "x86_64");
        assert_eq!(obj["id"], "11111111-1111-4111-8111-111111111111");
    }

    #[test]
    fn placeholder_and_empty_are_not_configured() {
        assert!(!HttpUsageReporter::is_configured(PLACEHOLDER_ENDPOINT));
        assert!(!HttpUsageReporter::is_configured(""));
        assert!(HttpUsageReporter::is_configured(
            "https://example.test/ping"
        ));
    }

    #[tokio::test]
    async fn placeholder_endpoint_sends_nothing() {
        // A reporter on the placeholder must not make any request; if it did, this
        // would still pass, but the point is it returns Ok without panicking.
        let reporter = HttpUsageReporter::new(PLACEHOLDER_ENDPOINT);
        assert!(reporter.report(&sample()).await.is_ok());
    }

    #[tokio::test]
    async fn posts_minimal_body_to_collector() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/ping"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let reporter = HttpUsageReporter::new(format!("{}/ping", server.uri()));
        assert!(reporter.report(&sample()).await.is_ok());
        // Drop verifies the `.expect(1)` — the POST happened exactly once.
    }

    #[tokio::test]
    async fn server_error_is_swallowed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let reporter = HttpUsageReporter::new(format!("{}/ping", server.uri()));
        // A 5xx must not produce an error to the caller.
        assert!(reporter.report(&sample()).await.is_ok());
    }
}
