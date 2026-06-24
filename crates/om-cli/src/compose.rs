//! Composition root.
//!
//! This is the **only** module in the workspace that names concrete adapters.
//! It reads the user's [`Config`] and assembles an [`Engine`] of `Arc<dyn Port>`
//! trait objects. Everything downstream depends on ports, never on the types
//! constructed here — so adding a debrid backend or a tracker is a change *only*
//! to this file plus the new adapter crate (OCP).

use std::sync::Arc;

use om_app::{Engine, EngineBuilder};
use om_config::Config;
use om_core::scoring::ScoringPrefs;
use om_core::stream::Quality;

use om_debrid::RealDebrid;
use om_history::SqliteHistory;
use om_metadata::{AniListProvider, TmdbProvider};
use om_player::{MpvPlayer, VlcPlayer};
use om_sources::{NyaaSource, TorrentioSource};
use om_stream::{HybridResolver, P2pEngine};
use om_track::{AniListTracker, AniSkipEnricher, CompositeTracker, DiscordPresence, MalTracker};

/// Build the fully-wired [`Engine`] for the given config.
pub fn build_engine(cfg: &Config) -> Engine {
    let mut builder = EngineBuilder::default().scoring_prefs(scoring_prefs(cfg));

    // --- Metadata providers ---
    if !cfg.credentials.tmdb_api_key.is_empty() {
        builder = builder.add_metadata(Arc::new(TmdbProvider::new(&cfg.credentials.tmdb_api_key)));
    }
    // AniList needs no key for read/search; it enriches anime discovery.
    builder = builder.add_metadata(Arc::new(AniListProvider::new()));

    // --- Source providers ---
    builder = builder.add_source(Arc::new(TorrentioSource::new(torrentio_config_string(cfg))));
    if cfg.providers.nyaa_direct {
        builder = builder.add_source(Arc::new(NyaaSource::new()));
    }

    // --- Debrid + resolver (debrid optional → P2P fallback) ---
    let debrid: Option<Arc<dyn om_core::ports::DebridProvider>> =
        if cfg.has_debrid() && cfg.credentials.debrid_provider == "real-debrid" {
            Some(Arc::new(RealDebrid::new(
                &cfg.credentials.real_debrid_token,
            )))
        } else {
            None
        };
    let p2p = Arc::new(P2pEngine::new(
        cfg.streaming.http_port,
        cfg.streaming.cleanup_after_playback,
    ));
    builder = builder.resolver(Arc::new(HybridResolver::new(debrid, p2p)));

    // --- Player (mpv enables IPC features; vlc is launch-only) ---
    if cfg.player.command == "vlc" {
        builder = builder.player(Arc::new(VlcPlayer::new(cfg.player.args.clone())));
    } else {
        builder = builder.player(Arc::new(MpvPlayer::new(
            cfg.player.command.clone(),
            cfg.player.args.clone(),
        )));
    }

    // --- Trackers (composite of whatever tokens exist) ---
    let mut members: Vec<Box<dyn om_core::ports::Tracker>> = Vec::new();
    if !cfg.credentials.anilist_token.is_empty() {
        members.push(Box::new(AniListTracker::new(
            &cfg.credentials.anilist_token,
        )));
    }
    if !cfg.credentials.mal_token.is_empty() {
        members.push(Box::new(MalTracker::new(&cfg.credentials.mal_token)));
    }
    if !members.is_empty() {
        builder = builder.tracker(Arc::new(CompositeTracker::new(members)));
    }

    // --- Enricher (AniSkip + Jikan) ---
    if cfg.behavior.skip_intro_outro || cfg.behavior.skip_filler {
        builder = builder.enricher(Arc::new(AniSkipEnricher::new()));
    }

    // --- History ---
    if let Ok(history) = SqliteHistory::open(SqliteHistory::default_path()) {
        builder = builder.history(Arc::new(history));
    }

    // --- Presence ---
    if cfg.behavior.discord_presence {
        builder = builder.presence(Arc::new(DiscordPresence::new(DISCORD_CLIENT_ID)));
    }

    builder.build()
}

/// open-media's Discord application id (placeholder until registered in Phase 6).
const DISCORD_CLIENT_ID: &str = "0000000000000000000";

/// Map the config's quality string onto a [`ScoringPrefs`] target.
fn scoring_prefs(cfg: &Config) -> ScoringPrefs {
    let target_quality = match cfg.providers.quality.as_str() {
        "2160p" | "4k" => Some(Quality::P2160),
        "1080p" => Some(Quality::P1080),
        "720p" => Some(Quality::P720),
        "480p" => Some(Quality::P480),
        _ => None, // "best" => no fixed target; rank by raw quality.
    };
    ScoringPrefs {
        prefer_cached: cfg.has_debrid() && !cfg.providers.show_uncached,
        target_quality,
        ..Default::default()
    }
}

/// Build the Torrentio addon config path segment from user config.
///
/// Mirrors miru: `providers=...|sort=qualitysize|qualityfilter=scr,cam` and, when
/// a debrid token is present, `|debridoptions=nodownloadlinks|realdebrid=KEY`
/// (omitting `nodownloadlinks` when uncached results are wanted).
fn torrentio_config_string(cfg: &Config) -> String {
    let providers = cfg.providers.torrentio_providers.join(",");
    let mut parts = vec![
        format!("providers={providers}"),
        "sort=qualitysize".to_string(),
        "qualityfilter=scr,cam".to_string(),
    ];
    if cfg.has_debrid() {
        if !cfg.providers.show_uncached {
            parts.push("debridoptions=nodownloadlinks".to_string());
        }
        parts.push(format!("realdebrid={}", cfg.credentials.real_debrid_token));
    }
    parts.join("|")
}
