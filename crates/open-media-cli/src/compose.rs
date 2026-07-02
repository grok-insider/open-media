//! Composition root.
//!
//! This is the **only** module in the workspace that names concrete adapters.
//! It reads the user's [`Config`] and assembles an [`Engine`] of `Arc<dyn Port>`
//! trait objects. Everything downstream depends on ports, never on the types
//! constructed here — so adding a debrid backend or a tracker is a change *only*
//! to this file plus the new adapter crate (OCP).

use std::sync::Arc;

use open_media_app::{Engine, EngineBuilder};
use open_media_config::Config;
use open_media_core::scoring::ScoringPrefs;
use open_media_core::stream::Quality;

use open_media_debrid::{RealDebrid, Torbox};
use open_media_history::SqliteHistory;
use open_media_metadata::{AniListProvider, CinemetaProvider, FribbIdBridge, TmdbProvider};
use open_media_player::{MpvPlayer, VlcPlayer};
use open_media_sources::{NyaaSource, TorrentioSource};
use open_media_stream::{HybridResolver, P2pEngine};
use open_media_subs::OpenSubtitleAdapter;
use open_media_track::{
    AniListTracker, AniSkipEnricher, CompositeTracker, DiscordPresence, MalTracker,
};

/// Build the fully-wired [`Engine`] for the given config.
pub fn build_engine(cfg: &Config) -> Engine {
    let mut builder = EngineBuilder::default()
        .scoring_prefs(scoring_prefs(cfg))
        .complete_threshold(cfg.behavior.complete_threshold)
        .skip_filler(cfg.behavior.skip_filler)
        .autoplay_next(cfg.behavior.autoplay_next)
        .resume(cfg.behavior.resume);

    // --- Metadata providers ---
    // TMDB first (richest) when a key is configured; its IMDB ids win on dedup.
    if !cfg.credentials.tmdb_api_key.is_empty() {
        builder = builder.add_metadata(Arc::new(TmdbProvider::new(&cfg.credentials.tmdb_api_key)));
    }
    // Cinemeta: keyless, IMDB-native movie/series discovery — the default so the
    // app works with no TMDB key. Results dedup against TMDB by IMDB id.
    if cfg.providers.cinemeta {
        builder = builder.add_metadata(Arc::new(CinemetaProvider::new()));
    }
    // AniList needs no key for read/search; it enriches anime discovery.
    builder = builder.add_metadata(Arc::new(AniListProvider::new()));

    // --- Source providers ---
    builder = builder.add_source(Arc::new(TorrentioSource::new(torrentio_config_string(cfg))));
    if cfg.providers.nyaa_direct {
        builder = builder.add_source(Arc::new(NyaaSource::with_category(
            cfg.providers.nyaa_category.clone(),
        )));
    }

    // --- AniList/MAL → IMDB bridge (keyless; fetch-and-cache) ---
    // Always wired: anime are discovered by AniList (no IMDB id), which makes the
    // IMDB-keyed providers (Torrentio → debrid) short-circuit for them. The
    // bridge fills `ids.imdb` for anime that have an IMDB mapping so those
    // providers can serve them; titles without a mapping keep their nyaa sources.
    builder = builder.id_bridge(Arc::new(FribbIdBridge::new()));

    // --- Debrid + resolver (debrid optional → P2P fallback) ---
    let debrid: Option<Arc<dyn open_media_core::ports::DebridProvider>> = if cfg.has_real_debrid() {
        Some(Arc::new(RealDebrid::new(
            &cfg.credentials.real_debrid_token,
        )))
    } else if cfg.has_torbox() {
        Some(Arc::new(Torbox::new(&cfg.credentials.torbox_token)))
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
            cfg.player.thumbnail_previews,
        )));
    }

    // --- Trackers (composite of whatever tokens exist) ---
    let mut members: Vec<Box<dyn open_media_core::ports::Tracker>> = Vec::new();
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

    // --- History + local library ---
    if let Ok(history) = SqliteHistory::open(SqliteHistory::default_path()) {
        let history = Arc::new(history);
        builder = builder.history(history.clone()).library(history);
    }

    // --- Presence ---
    if cfg.behavior.discord_presence {
        builder = builder.presence(Arc::new(DiscordPresence::new(DISCORD_CLIENT_ID)));
    }

    // --- Subtitles (open-subtitle, opt-in) ---
    // Building the engine can fail on misconfiguration; like the history store,
    // a failure disables the feature rather than aborting startup. The preferred
    // languages are also threaded to the engine for the SubtitleQuery.
    if cfg.subtitles.enabled {
        match OpenSubtitleAdapter::new(cfg.subtitles.languages.clone()) {
            Ok(adapter) => {
                builder = builder
                    .subtitles(Arc::new(adapter))
                    .subtitle_languages(cfg.subtitles.languages.clone());
            }
            Err(e) => {
                tracing::warn!(error = %e, "subtitle provider disabled (engine build failed)");
            }
        }
    }

    builder.build()
}

/// open-media's registered Discord application id. Public by design — it is sent
/// in the rich-presence IPC handshake and is not a secret (unlike a bot token).
/// Forks wanting their own app name on the "Watching" card register their own app
/// and swap this constant.
const DISCORD_CLIENT_ID: &str = "1495240340636958811";

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
    // Only inject a debrid param for the *active* backend — matches the
    // `DebridProvider` wiring gate so the two never disagree.
    if cfg.has_real_debrid() {
        if !cfg.providers.show_uncached {
            parts.push("debridoptions=nodownloadlinks".to_string());
        }
        parts.push(format!("realdebrid={}", cfg.credentials.real_debrid_token));
    } else if cfg.has_torbox() {
        if !cfg.providers.show_uncached {
            parts.push("debridoptions=nodownloadlinks".to_string());
        }
        parts.push(format!("torbox={}", cfg.credentials.torbox_token));
    }
    parts.join("|")
}
