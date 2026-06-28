//! # om-config
//!
//! The on-disk configuration schema and its load/save logic.
//!
//! ## Secrets policy
//! API tokens (debrid, TMDB, tracker OAuth) live in this file under the user's
//! XDG config dir (`~/.config/open-media/config.toml`), **never** in the source
//! tree, environment-baked binaries, or the Nix store. `om-config` owns the file
//! so the rest of the app never reads tokens from anywhere else. Environment
//! variables (`OPEN_MEDIA_*`) may override at runtime for ephemeral/CI use.
//!
//! The schema is intentionally flat and `#[serde(default)]`-heavy so a minimal
//! file (even an empty one) deserializes — there are no required keys; search
//! works keyless via Cinemeta + AniList. Matches miru's ergonomics.

use std::path::PathBuf;

use om_core::error::{CoreError, CoreResult};
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
    pub ui: Ui,
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
    /// Active debrid backend: `"real-debrid"` (default), later `"alldebrid"`,
    /// `"torbox"`, `"premiumize"`.
    #[serde(default = "default_debrid")]
    pub debrid_provider: String,
    /// Real-Debrid API token (optional — empty falls back to P2P streaming).
    #[serde(default)]
    pub real_debrid_token: String,
    /// AniList OAuth access token (optional — enables anime tracking).
    #[serde(default)]
    pub anilist_token: String,
    /// MyAnimeList OAuth access token (optional).
    #[serde(default)]
    pub mal_token: String,
}

impl Default for Credentials {
    fn default() -> Self {
        Self {
            tmdb_api_key: String::new(),
            debrid_provider: default_debrid(),
            real_debrid_token: String::new(),
            anilist_token: String::new(),
            mal_token: String::new(),
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
    /// Torrentio sub-providers, highest priority first.
    #[serde(default = "default_torrentio_providers")]
    pub torrentio_providers: Vec<String>,
    /// Also query nyaa.si directly (anime), in addition to Torrentio's `nyaasi`.
    #[serde(default = "default_true")]
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
}

impl Default for Providers {
    fn default() -> Self {
        Self {
            cinemeta: true,
            torrentio_providers: default_torrentio_providers(),
            nyaa_direct: true,
            nyaa_category: default_nyaa_category(),
            quality: default_quality(),
            show_uncached: false,
        }
    }
}

/// External media player configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    /// Player binary: `"mpv"` (default, enables IPC features) or `"vlc"`.
    #[serde(default = "default_player")]
    pub command: String,
    /// Extra args appended to the player invocation.
    #[serde(default = "default_player_args")]
    pub args: Vec<String>,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            command: default_player(),
            args: default_player_args(),
        }
    }
}

/// Local P2P streaming engine knobs (used when a source is played without debrid).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Streaming {
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
            resume: true,
            complete_threshold: default_complete_threshold(),
            discord_presence: false,
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
/// `om-config` stays free of the TUI's enums; the TUI maps them on load/save.
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

impl Config {
    /// Whether search can return movies/series. Keyless via Cinemeta by default,
    /// or via a configured TMDB key. (Anime always works through AniList.)
    pub fn is_usable(&self) -> bool {
        self.providers.cinemeta || !self.credentials.tmdb_api_key.is_empty()
    }

    /// Whether a debrid token is configured (else P2P streaming is used).
    pub fn has_debrid(&self) -> bool {
        !self.credentials.real_debrid_token.is_empty()
    }

    /// Whether Real-Debrid specifically is the active, configured backend. Gates
    /// both the `DebridProvider` wiring and the `realdebrid=` Torrentio param so
    /// the two never disagree.
    pub fn has_real_debrid(&self) -> bool {
        self.has_debrid() && self.credentials.debrid_provider == "real-debrid"
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
    Ok(cfg)
}

/// Apply `OPEN_MEDIA_*` credential overrides from the environment onto a parsed
/// config. An env var only overrides when present **and** non-empty, so an
/// accidentally-exported empty var never blanks a configured token. Table-driven
/// so adding a key is one row. Never logs a value.
fn apply_env_overrides(cfg: &mut Config) {
    let overrides: [(&str, &mut String); 4] = [
        ("OPEN_MEDIA_TMDB_API_KEY", &mut cfg.credentials.tmdb_api_key),
        (
            "OPEN_MEDIA_REAL_DEBRID_TOKEN",
            &mut cfg.credentials.real_debrid_token,
        ),
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
        assert!(cfg.providers.nyaa_direct);
        assert!(cfg.providers.cinemeta);
        assert!(cfg.behavior.skip_intro_outro);
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
    fn roundtrips() {
        let mut cfg = Config::default();
        cfg.credentials.tmdb_api_key = "k".into();
        cfg.credentials.real_debrid_token = "t".into();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert!(back.has_debrid());
    }
}
