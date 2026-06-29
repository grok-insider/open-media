//! Startup usage analytics: the first-run notice and the once-per-launch ping.
//!
//! This is the composition root for the [`UsageReporter`] port. Telemetry is
//! **on by default (opt-out)** and intentionally minimal: a single anonymous ping
//! per launch (app version, OS, arch, a random install id) so the project can
//! estimate active installs. It never carries anything about what is watched.
//!
//! The collector endpoint is a [`PLACEHOLDER_ENDPOINT`] sentinel until a real one
//! is configured — while it is the placeholder, [`fire`] builds and "sends"
//! nothing (the adapter no-ops), so default-on does not POST to a dead host.
//!
//! [`UsageReporter`]: open_media_core::ports::UsageReporter

use open_media_config::Config;
use open_media_core::ports::UsageReporter;
use open_media_core::usage::UsageInfo;
use open_media_telemetry::{HttpUsageReporter, PLACEHOLDER_ENDPOINT};

/// The usage-analytics collector. Replace [`PLACEHOLDER_ENDPOINT`] with a real
/// URL (e.g. a self-hosted Aptabase/Plausible/Umami endpoint) to start counting
/// active installs. Until then telemetry is wired but inert.
const TELEMETRY_ENDPOINT: &str = PLACEHOLDER_ENDPOINT;

/// Show the one-time telemetry notice if it has not been shown yet, then persist
/// that it has. Idempotent across launches via `config.telemetry.notified`.
///
/// Called on the first run (and any later run that has not yet seen the notice)
/// so an opt-out user is informed in plain language how to disable it.
pub fn maybe_notify(cfg: &mut Config) {
    if cfg.telemetry.notified {
        return;
    }
    println!("open-media sends an anonymous usage ping (OS, architecture, app");
    println!("version, and a random id) once per launch so we can count active");
    println!("installs. It never includes anything about what you watch.");
    println!("It is ON by default — disable any time with:");
    println!("    om config set telemetry=false");
    println!();
    cfg.telemetry.notified = true;
    // Best-effort persist; if it fails the notice simply shows again next run.
    let _ = open_media_config::save(cfg);
}

/// Fire the anonymous usage ping for this launch, detached, when telemetry is
/// enabled and a real collector is configured. Never blocks or fails the caller:
/// the spawned task swallows all errors.
pub fn fire(cfg: &Config) {
    if !cfg.telemetry.enabled || !HttpUsageReporter::is_configured(TELEMETRY_ENDPOINT) {
        return;
    }
    let info = UsageInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        install_id: cfg.telemetry.install_id.clone(),
    };
    tokio::spawn(async move {
        let reporter = HttpUsageReporter::new(TELEMETRY_ENDPOINT);
        let _ = reporter.report(&info).await;
    });
}

/// Convenience: run the notice then fire the ping for a loaded config. Mutates
/// `cfg` (sets `notified`); callers that already hold `&mut Config` use this.
pub fn startup(cfg: &mut Config) {
    maybe_notify(cfg);
    fire(cfg);
}
