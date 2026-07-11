//! # open-media-config
//!
//! The on-disk configuration schema and its load/save logic.
//!
//! ## Secrets policy
//! API tokens (debrid, TMDB, tracker OAuth) live in this file under the user's
//! XDG config dir (`~/.config/open-media/config.toml`), **never** in the source
//! tree, environment-baked binaries, or the Nix store. `open-media-config` owns the file
//! so the rest of the app never reads tokens from anywhere else. Environment
//! variables (`OPEN_MEDIA_*`) may override at runtime for ephemeral/CI use.
//!
//! The schema is intentionally flat and `#[serde(default)]`-heavy so a minimal
//! file (even an empty one) deserializes — there are no required keys; search
//! works keyless via Cinemeta + AniList. Matches miru's ergonomics.

use std::path::PathBuf;

use open_media_core::error::{CoreError, CoreResult};
use serde::{Deserialize, Serialize};

/// Root configuration document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub credentials: Credentials,
    #[serde(default)]
    pub providers: Providers,
    #[serde(default)]
    pub player: Player,
    #[serde(default)]
    pub streaming: Streaming,
    #[serde(default)]
    pub behavior: Behavior,
    #[serde(default)]
    pub subtitles: Subtitles,
    #[serde(default)]
    pub ui: Ui,
    #[serde(default)]
    pub telemetry: Telemetry,
}

/// All secret material. The only place tokens live.
///
/// `Default` is implemented by hand (not derived) so it matches the
/// `#[serde(default = ...)]` field defaults — otherwise `Config::default()` and a
/// freshly-deserialized empty document would disagree on `debrid_provider`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// TMDB v3 API key — optional; enriches movie/series search. Cinemeta
    /// provides keyless movie/series discovery when this is empty.
    #[serde(default)]
    pub tmdb_api_key: String,
    /// Active debrid backend: `"real-debrid"` (default) or `"torbox"`; later
    /// `"alldebrid"`, `"premiumize"`.
    #[serde(default = "default_debrid")]
    pub debrid_provider: String,
    /// Real-Debrid API token (optional — empty means no RD; P2P only if
    /// `streaming.allow_p2p`).
    #[serde(default)]
    pub real_debrid_token: String,
    /// TorBox API key (optional — used when `debrid_provider = "torbox"`).
    #[serde(default)]
    pub torbox_token: String,
    /// AniList OAuth access token (optional — enables anime tracking).
    #[serde(default)]
    pub anilist_token: String,
    /// MyAnimeList OAuth access token (optional).
    #[serde(default)]
    pub mal_token: String,
    /// MyAnimeList API client id, required for `open-media login mal`. Public
    /// (not a secret) — register one at <https://myanimelist.net/apiconfig>.
    #[serde(default)]
    pub mal_client_id: String,
    /// MyAnimeList API client secret. Only set when the registered app type is
    /// "web" (public app types like "other" have no secret).
    #[serde(default)]
    pub mal_client_secret: String,
    /// MyAnimeList OAuth refresh token, persisted by `open-media login mal` and
    /// rotated automatically when the access token nears expiry.
    #[serde(default)]
    pub mal_refresh_token: String,
    /// Unix timestamp (seconds) when `mal_token` expires; `0` = unknown (e.g. a
    /// manually provisioned token), which disables auto-refresh.
    #[serde(default)]
    pub mal_token_expires_at: i64,
}

impl Default for Credentials {
    fn default() -> Self {
        Self {
            tmdb_api_key: String::new(),
            debrid_provider: default_debrid(),
            real_debrid_token: String::new(),
            torbox_token: String::new(),
            anilist_token: String::new(),
            mal_token: String::new(),
            mal_client_id: String::new(),
            mal_client_secret: String::new(),
            mal_refresh_token: String::new(),
            mal_token_expires_at: 0,
        }
    }
}

/// Source/metadata provider knobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Providers {
    /// Use Stremio Cinemeta for keyless movie/series discovery. On by default so
    /// the app works with no TMDB key; disable to rely solely on TMDB.
    #[serde(default = "default_true")]
    pub cinemeta: bool,
    /// Wire the Torrentio source adapter. **Off by default** (dual-use / opt-in
    /// posture — see `docs/LEGAL.md`).
    #[serde(default)]
    pub torrentio: bool,
    /// Torrentio sub-providers, highest priority first (used when `torrentio`).
    #[serde(default = "default_torrentio_providers")]
    pub torrentio_providers: Vec<String>,
    /// Query nyaa.si directly (anime). **Off by default** (opt-in; see LEGAL.md).
    #[serde(default)]
    pub nyaa_direct: bool,
    /// nyaa.si category for direct queries (the `c=` RSS parameter). Defaults to
    /// `"1_2"` (English-translated anime); `"1_3"` is raw/untranslated.
    #[serde(default = "default_nyaa_category")]
    pub nyaa_category: String,
    /// Quality preference: `"best" | "2160p" | "1080p" | "720p" | "480p"`.
    #[serde(default = "default_quality")]
    pub quality: String,
    /// Show uncached candidates in source lists (slower to start).
    #[serde(default)]
    pub show_uncached: bool,
    /// User has acknowledged `docs/LEGAL.md` before enabling torrent sources.
    /// Soft flag set via config/init; not a hard runtime gate.
    #[serde(default)]
    pub sources_acknowledged: bool,
}

impl Default for Providers {
    fn default() -> Self {
        Self {
            cinemeta: true,
            torrentio: false,
            torrentio_providers: default_torrentio_providers(),
            nyaa_direct: false,
            nyaa_category: default_nyaa_category(),
            quality: default_quality(),
            show_uncached: false,
            sources_acknowledged: false,
        }
    }
}

/// External media player configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    /// Player binary: `"mpv"` (default, enables IPC features) or `"vlc"`.
    #[serde(default = "default_player")]
    pub command: String,
    /// Enable best-effort mpv seekbar thumbnail script compatibility. Requires
    /// user-installed mpv scripts such as thumbfast plus uosc or another
    /// thumbfast-compatible OSC; open-media does not bundle scripts.
    #[serde(default)]
    pub thumbnail_previews: bool,
    /// Extra args appended to the player invocation.
    #[serde(default = "default_player_args")]
    pub args: Vec<String>,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            command: default_player(),
            thumbnail_previews: false,
            args: default_player_args(),
        }
    }
}

/// Local P2P streaming engine knobs (used when a source is played without debrid).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Streaming {
    /// Allow the local BitTorrent (librqbit) path. **Off by default** — P2P may
    /// upload pieces to peers (see `docs/LEGAL.md`). When false, only direct
    /// URLs and debrid resolution can produce playback.
    #[serde(default)]
    pub allow_p2p: bool,
    /// Port for the local librqbit HTTP stream server.
    #[serde(default = "default_stream_port")]
    pub http_port: u16,
    /// Delete the downloaded torrent data after playback.
    #[serde(default = "default_true")]
    pub cleanup_after_playback: bool,
}

impl Default for Streaming {
    fn default() -> Self {
        Self {
            allow_p2p: false,
            http_port: default_stream_port(),
            cleanup_after_playback: true,
        }
    }
}

/// Behavioral toggles for the playback session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Behavior {
    /// Auto-skip openings/endings using AniSkip data (mpv only).
    #[serde(default = "default_true")]
    pub skip_intro_outro: bool,
    /// Skip filler/recap episodes when bingeing (anime, via Jikan).
    #[serde(default)]
    pub skip_filler: bool,
    /// After an episode completes, automatically advance to the next one and keep
    /// playing (binge mode). Off by default; only triggers for episodic content
    /// and only when the finished episode crossed `complete_threshold`.
    #[serde(default)]
    pub autoplay_next: bool,
    /// Keep one player process per episodic session and pre-append the next
    /// episode to its playlist so the player's own Next button works (mpv).
    /// Independent of `autoplay_next`, which controls whether the player
    /// advances *by itself* at the end of an episode. On by default.
    #[serde(default = "default_true")]
    pub playlist_next: bool,
    /// Resume from the last saved position.
    #[serde(default = "default_true")]
    pub resume: bool,
    /// Fraction watched at which an episode is marked complete (e.g. 0.85).
    #[serde(default = "default_complete_threshold")]
    pub complete_threshold: f32,
    /// Publish a Discord rich-presence "now watching" status.
    #[serde(default)]
    pub discord_presence: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            skip_intro_outro: true,
            skip_filler: false,
            autoplay_next: false,
            playlist_next: true,
            resume: true,
            complete_threshold: default_complete_threshold(),
            discord_presence: false,
        }
    }
}

/// External subtitle auto-fetch (open-subtitle). Off by default — it adds a
/// network round-trip before playback, so it is opt-in.
///
/// `Default` is implemented by hand (not derived) so it matches the
/// `#[serde(default = ...)]` field defaults; a test asserts the two agree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subtitles {
    /// Fetch external subtitles before launching the player and pass them in.
    #[serde(default)]
    pub enabled: bool,
    /// Preferred subtitle language tags, most-wanted first (e.g. `["en", "ja"]`).
    #[serde(default = "default_subtitle_languages")]
    pub languages: Vec<String>,
}

impl Default for Subtitles {
    fn default() -> Self {
        Self {
            enabled: false,
            languages: default_subtitle_languages(),
        }
    }
}

/// Terminal UI preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ui {
    /// `"auto" | "dark" | "light"`.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Persisted defaults for the Sources screen's filter/sort panel.
    #[serde(default)]
    pub sources: SourcesUi,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            sources: SourcesUi::default(),
        }
    }
}

/// Persisted Sources-screen filter/sort selections. Stored as strings/bools so
/// `open-media-config` stays free of the TUI's enums; the TUI maps them on load/save.
/// `"all"` means no filter. All five are remembered across sessions and applied
/// literally; if a remembered filter hides everything on a later search, the
/// panel shows `0 of N` and `Clear` resets it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcesUi {
    /// `"relevance" | "seeders" | "quality" | "size"`.
    #[serde(default = "default_sort")]
    pub sort: String,
    /// `"all" | "2160p" | "1080p" | "720p" | "480p" | "360p"`.
    #[serde(default = "default_all")]
    pub quality: String,
    /// Language name (e.g. `"English"`) or `"all"`.
    #[serde(default = "default_all")]
    pub language: String,
    /// Provider/tracker name (e.g. `"1337x"`) or `"all"`.
    #[serde(default = "default_all")]
    pub provider: String,
    /// Only show debrid-cached candidates.
    #[serde(default)]
    pub cached_only: bool,
}

impl Default for SourcesUi {
    fn default() -> Self {
        Self {
            sort: default_sort(),
            quality: default_all(),
            language: default_all(),
            provider: default_all(),
            cached_only: false,
        }
    }
}

/// Anonymous usage analytics. **On by default** (opt-out) so the project can
/// estimate how many active installs exist (DAU/MAU). The only data ever sent is
/// the app version, host OS/arch, and the random [`install_id`](Telemetry::install_id);
/// **never** anything about what is watched (no titles, queries, tokens, or
/// history). Disable any time with `open-media config set telemetry=false`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Telemetry {
    /// Send the anonymous usage ping once per launch. Default `true` (opt-out).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Random per-install id (UUID v4). Empty until generated once via
    /// [`Config::ensure_install_id`]; persisted so the count is by install, not by
    /// launch. Carries no PII.
    #[serde(default)]
    pub install_id: String,
    /// Set once the first-run telemetry notice has been shown, so it prints only
    /// the first time. Default `false`.
    #[serde(default)]
    pub notified: bool,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            enabled: true,
            install_id: String::new(),
            notified: false,
        }
    }
}

impl Config {
    /// Whether search can return movies/series. Keyless via Cinemeta by default,
    /// or via a configured TMDB key. (Anime always works through AniList.)
    pub fn is_usable(&self) -> bool {
        self.providers.cinemeta || !self.credentials.tmdb_api_key.is_empty()
    }

    /// Whether the *active* debrid backend has a token configured.
    /// Without debrid, magnet playback needs `streaming.allow_p2p`.
    pub fn has_debrid(&self) -> bool {
        match self.credentials.debrid_provider.as_str() {
            "torbox" => !self.credentials.torbox_token.is_empty(),
            _ => !self.credentials.real_debrid_token.is_empty(),
        }
    }

    /// Whether Real-Debrid specifically is the active, configured backend. Gates
    /// both the `DebridProvider` wiring and the `realdebrid=` Torrentio param so
    /// the two never disagree.
    pub fn has_real_debrid(&self) -> bool {
        self.has_debrid() && self.credentials.debrid_provider == "real-debrid"
    }

    /// Whether TorBox specifically is the active, configured backend. Gates both
    /// the `DebridProvider` wiring and the `torbox=` Torrentio param so the two
    /// never disagree.
    pub fn has_torbox(&self) -> bool {
        self.has_debrid() && self.credentials.debrid_provider == "torbox"
    }

    /// Ensure a telemetry install id exists, generating a random UUID v4 the first
    /// time. Returns `true` if a new id was generated (i.e. the caller should
    /// persist the config). Idempotent: a non-empty id is left untouched. The id
    /// is generated even when telemetry is disabled — it is not transmitted unless
    /// the user opts in, and pre-generating keeps the file stable.
    pub fn ensure_install_id(&mut self) -> bool {
        if self.telemetry.install_id.is_empty() {
            self.telemetry.install_id = uuid::Uuid::new_v4().to_string();
            true
        } else {
            false
        }
    }
}

/// `~/.config/open-media/config.toml` (respects `XDG_CONFIG_HOME`).
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("open-media")
        .join("config.toml")
}

/// Load and parse the config file. Returns [`CoreError::Config`] on parse error
/// and [`CoreError::NotFound`] when the file does not exist.
pub fn load() -> CoreResult<Config> {
    let path = config_path();
    if !path.exists() {
        return Err(CoreError::NotFound(format!(
            "config file not found at {}",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CoreError::Storage(format!("reading {}: {e}", path.display())))?;
    let mut cfg: Config = toml::from_str(&text).map_err(|e| CoreError::Config(e.to_string()))?;
    apply_env_overrides(&mut cfg);
    // Backfill a stable install id for configs created before telemetry existed,
    // persisting it so the analytics count is by install rather than by launch. A
    // write failure here is non-fatal: the in-memory id still works for this run.
    if cfg.ensure_install_id() {
        let _ = save(&cfg);
    }
    Ok(cfg)
}

/// Apply `OPEN_MEDIA_*` credential overrides from the environment onto a parsed
/// config. An env var only overrides when present **and** non-empty, so an
/// accidentally-exported empty var never blanks a configured token. Table-driven
/// so adding a key is one row. Never logs a value.
fn apply_env_overrides(cfg: &mut Config) {
    let overrides: [(&str, &mut String); 5] = [
        ("OPEN_MEDIA_TMDB_API_KEY", &mut cfg.credentials.tmdb_api_key),
        (
            "OPEN_MEDIA_REAL_DEBRID_TOKEN",
            &mut cfg.credentials.real_debrid_token,
        ),
        ("OPEN_MEDIA_TORBOX_TOKEN", &mut cfg.credentials.torbox_token),
        (
            "OPEN_MEDIA_ANILIST_TOKEN",
            &mut cfg.credentials.anilist_token,
        ),
        ("OPEN_MEDIA_MAL_TOKEN", &mut cfg.credentials.mal_token),
    ];
    for (var, slot) in overrides {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                *slot = val;
            }
        }
    }
}

/// Serialize and write the config file, creating parent dirs as needed.
pub fn save(config: &Config) -> CoreResult<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Storage(format!("creating {}: {e}", parent.display())))?;
    }
    let text = toml::to_string_pretty(config).map_err(|e| CoreError::Config(e.to_string()))?;
    std::fs::write(&path, text)
        .map_err(|e| CoreError::Storage(format!("writing {}: {e}", path.display())))
}

fn default_debrid() -> String {
    "real-debrid".to_string()
}
fn default_true() -> bool {
    true
}
fn default_quality() -> String {
    "best".to_string()
}
fn default_nyaa_category() -> String {
    "1_2".to_string()
}
fn default_player() -> String {
    "mpv".to_string()
}
fn default_player_args() -> Vec<String> {
    vec!["--fullscreen".to_string()]
}
fn default_stream_port() -> u16 {
    3131
}
fn default_complete_threshold() -> f32 {
    0.85
}
fn default_subtitle_languages() -> Vec<String> {
    vec!["en".to_string()]
}
fn default_theme() -> String {
    "auto".to_string()
}
fn default_sort() -> String {
    "relevance".to_string()
}
fn default_all() -> String {
    "all".to_string()
}
fn default_torrentio_providers() -> Vec<String> {
    [
        "yts",
        "eztv",
        "rarbg",
        "1337x",
        "thepiratebay",
        "kickasstorrents",
        "torrentgalaxy",
        "nyaasi",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate process-global env, since `set_var`/`remove_var`
    /// race across the test binary's threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn env_override_wins_when_set() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let mut cfg = Config::default();
        cfg.credentials.tmdb_api_key = "from-file".into();

        // env access is serialized by ENV_LOCK; the var is removed before the lock
        // is released so no other test observes it.
        std::env::set_var("OPEN_MEDIA_TMDB_API_KEY", "from-env");
        apply_env_overrides(&mut cfg);
        std::env::remove_var("OPEN_MEDIA_TMDB_API_KEY");

        assert_eq!(cfg.credentials.tmdb_api_key, "from-env");
    }

    #[test]
    fn empty_env_does_not_blank_configured_token() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let mut cfg = Config::default();
        cfg.credentials.real_debrid_token = "configured".into();

        // serialized by ENV_LOCK; removed before releasing the lock.
        std::env::set_var("OPEN_MEDIA_REAL_DEBRID_TOKEN", "");
        apply_env_overrides(&mut cfg);
        std::env::remove_var("OPEN_MEDIA_REAL_DEBRID_TOKEN");

        assert_eq!(cfg.credentials.real_debrid_token, "configured");
    }

    #[test]
    fn minimal_config_deserializes() {
        let toml = r#"
[credentials]
tmdb_api_key = "abc"
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.is_usable());
        assert!(!cfg.has_debrid());
        assert_eq!(cfg.player.command, "mpv");
        assert!(!cfg.player.thumbnail_previews);
        assert!(!cfg.providers.nyaa_direct);
        assert!(!cfg.providers.torrentio);
        assert!(!cfg.providers.sources_acknowledged);
        assert!(!cfg.streaming.allow_p2p);
        assert!(cfg.providers.cinemeta);
        assert!(cfg.behavior.skip_intro_outro);
    }

    #[test]
    fn dual_use_source_defaults_are_off() {
        let cfg = Config::default();
        assert!(!cfg.providers.torrentio);
        assert!(!cfg.providers.nyaa_direct);
        assert!(!cfg.streaming.allow_p2p);
        assert!(!cfg.providers.sources_acknowledged);

        // Empty document matches Default (opt-in sources stay off).
        let empty: Config = toml::from_str("").unwrap();
        assert!(!empty.providers.torrentio);
        assert!(!empty.providers.nyaa_direct);
        assert!(!empty.streaming.allow_p2p);
    }

    #[test]
    fn opt_in_source_flags_roundtrip() {
        let mut c = Config::default();
        c.providers.torrentio = true;
        c.providers.nyaa_direct = true;
        c.providers.sources_acknowledged = true;
        c.streaming.allow_p2p = true;
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert!(back.providers.torrentio);
        assert!(back.providers.nyaa_direct);
        assert!(back.providers.sources_acknowledged);
        assert!(back.streaming.allow_p2p);
    }

    #[test]
    fn player_thumbnail_previews_default_and_roundtrip() {
        // Off by default on an empty document, matching the manual Default impl.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.player.thumbnail_previews);
        assert_eq!(
            cfg.player.thumbnail_previews,
            Player::default().thumbnail_previews
        );

        // A customized value survives a serialize/deserialize round-trip.
        let mut c = Config::default();
        c.player.thumbnail_previews = true;
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert!(back.player.thumbnail_previews);
    }

    #[test]
    fn keyless_config_is_usable_via_cinemeta() {
        // No TMDB key at all: still usable because Cinemeta is keyless + on.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.credentials.tmdb_api_key.is_empty());
        assert!(cfg.is_usable());
    }

    #[test]
    fn sources_ui_defaults_and_roundtrip() {
        // Defaults on an empty document.
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.ui.sources.sort, "relevance");
        assert_eq!(cfg.ui.sources.quality, "all");
        assert!(!cfg.ui.sources.cached_only);

        // Round-trips a customized panel state.
        let mut c = Config::default();
        c.ui.sources.sort = "seeders".into();
        c.ui.sources.quality = "1080p".into();
        c.ui.sources.language = "English".into();
        c.ui.sources.provider = "1337x".into();
        c.ui.sources.cached_only = true;
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.ui.sources.sort, "seeders");
        assert_eq!(back.ui.sources.quality, "1080p");
        assert_eq!(back.ui.sources.language, "English");
        assert_eq!(back.ui.sources.provider, "1337x");
        assert!(back.ui.sources.cached_only);
    }

    #[test]
    fn nyaa_category_default_and_roundtrip() {
        // Default on an empty document matches the manual Default impl.
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.providers.nyaa_category, "1_2");
        assert_eq!(
            cfg.providers.nyaa_category,
            Providers::default().nyaa_category
        );

        // A customized value survives a serialize/deserialize round-trip.
        let mut c = Config::default();
        c.providers.nyaa_category = "1_3".into();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.providers.nyaa_category, "1_3");
    }

    #[test]
    fn autoplay_next_default_and_roundtrip() {
        // Off by default on an empty document, matching the manual Default impl.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.behavior.autoplay_next);
        assert_eq!(
            cfg.behavior.autoplay_next,
            Behavior::default().autoplay_next
        );

        // A customized value survives a serialize/deserialize round-trip.
        let mut c = Config::default();
        c.behavior.autoplay_next = true;
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert!(back.behavior.autoplay_next);
    }

    #[test]
    fn playlist_next_default_and_roundtrip() {
        // On by default (the player's Next button should just work) — on an
        // empty document and matching the manual Default impl.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.behavior.playlist_next);
        assert_eq!(
            cfg.behavior.playlist_next,
            Behavior::default().playlist_next
        );

        // Opting out survives a round-trip.
        let mut c = Config::default();
        c.behavior.playlist_next = false;
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert!(!back.behavior.playlist_next);
    }

    #[test]
    fn subtitles_default_and_roundtrip() {
        // Defaults on an empty document match the manual Default impl.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.subtitles.enabled);
        assert_eq!(cfg.subtitles.languages, vec!["en".to_string()]);
        assert_eq!(cfg.subtitles.enabled, Subtitles::default().enabled);
        assert_eq!(cfg.subtitles.languages, Subtitles::default().languages);

        // A customized value survives a serialize/deserialize round-trip.
        let mut c = Config::default();
        c.subtitles.enabled = true;
        c.subtitles.languages = vec!["ja".into(), "en".into()];
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert!(back.subtitles.enabled);
        assert_eq!(
            back.subtitles.languages,
            vec!["ja".to_string(), "en".to_string()]
        );
    }

    #[test]
    fn telemetry_default_on_and_roundtrip() {
        // On by default (opt-out) on an empty document, matching the manual Default.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.telemetry.enabled);
        assert!(!cfg.telemetry.notified);
        assert!(cfg.telemetry.install_id.is_empty());
        assert_eq!(cfg.telemetry.enabled, Telemetry::default().enabled);

        // A customized value survives a serialize/deserialize round-trip.
        let mut c = Config::default();
        c.telemetry.enabled = false;
        c.telemetry.notified = true;
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert!(!back.telemetry.enabled);
        assert!(back.telemetry.notified);
    }

    #[test]
    fn ensure_install_id_generates_once_and_is_stable() {
        let mut cfg = Config::default();
        assert!(cfg.telemetry.install_id.is_empty());

        // First call mints a uuid and signals a write is needed.
        assert!(cfg.ensure_install_id());
        let id = cfg.telemetry.install_id.clone();
        assert!(!id.is_empty());
        // It parses as a UUID (v4).
        assert!(uuid::Uuid::parse_str(&id).is_ok());

        // Second call is a no-op: same id, no write needed.
        assert!(!cfg.ensure_install_id());
        assert_eq!(cfg.telemetry.install_id, id);
    }

    #[test]
    fn roundtrips() {
        let mut cfg = Config::default();
        cfg.credentials.tmdb_api_key = "k".into();
        cfg.credentials.real_debrid_token = "t".into();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert!(back.has_debrid());
    }

    #[test]
    fn debrid_gating_follows_active_provider() {
        let mut cfg = Config::default();
        // A torbox token alone does nothing while real-debrid is active…
        cfg.credentials.torbox_token = "tb".into();
        assert!(!cfg.has_debrid());
        assert!(!cfg.has_torbox());
        // …until torbox becomes the active provider.
        cfg.credentials.debrid_provider = "torbox".into();
        assert!(cfg.has_debrid());
        assert!(cfg.has_torbox());
        assert!(!cfg.has_real_debrid());
        // And an RD token doesn't count while torbox is active.
        cfg.credentials.torbox_token.clear();
        cfg.credentials.real_debrid_token = "rd".into();
        assert!(!cfg.has_debrid());
    }
}
