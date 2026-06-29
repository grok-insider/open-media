//! Anonymous usage value type for the [`UsageReporter`] port.
//!
//! [`UsageInfo`] is the **entire** payload open-media will ever emit for usage
//! analytics. The privacy invariant is part of the contract: it carries only the
//! app version, the host OS/arch, and a random per-install id — and **never**
//! anything about *what* a user watches (no titles, queries, source names, tokens,
//! or history). Any adapter implementing [`UsageReporter`] must honor that.
//!
//! [`UsageReporter`]: crate::ports::UsageReporter

/// The minimal, non-identifying usage snapshot sent once per process launch so
/// the project can estimate active installs (DAU/MAU). Constructed from
/// compile-time/`std` constants plus a random install id; no machine probing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageInfo {
    /// App version (`CARGO_PKG_VERSION`), e.g. `"0.3.0"`.
    pub version: String,
    /// Host OS (`std::env::consts::OS`), e.g. `"linux"` / `"macos"` / `"windows"`.
    pub os: String,
    /// Host architecture (`std::env::consts::ARCH`), e.g. `"x86_64"` / `"aarch64"`.
    pub arch: String,
    /// Random per-install id (UUID v4), generated once and persisted in config.
    /// Lets the server count *unique* active installs without any PII; it is not
    /// tied to a person and carries no identifying information on its own.
    pub install_id: String,
}
