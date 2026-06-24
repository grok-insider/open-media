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
//! file (just the two required keys) deserializes, matching miru's ergonomics.

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
    /// TMDB v3 API key — required for movie/series search.
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
    /// Torrentio sub-providers, highest priority first.
    #[serde(default = "default_torrentio_providers")]
    pub torrentio_providers: Vec<String>,
    /// Also query nyaa.si directly (anime), in addition to Torrentio's `nyaasi`.
    #[serde(default = "default_true")]
    pub nyaa_direct: bool,
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
            torrentio_providers: default_torrentio_providers(),
            nyaa_direct: true,
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
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            theme: default_theme(),
        }
    }
}

impl Config {
    /// True once the minimum required key (TMDB) is present.
    pub fn is_usable(&self) -> bool {
        !self.credentials.tmdb_api_key.is_empty()
    }

    /// Whether a debrid token is configured (else P2P streaming is used).
    pub fn has_debrid(&self) -> bool {
        !self.credentials.real_debrid_token.is_empty()
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
    toml::from_str(&text).map_err(|e| CoreError::Config(e.to_string()))
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
        assert!(cfg.behavior.skip_intro_outro);
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
